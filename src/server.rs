//! 服务端装配与生命周期（第 1 节）。
//!
//! 职责（对应 Java `Server` 静态装配 + `main`）：
//! - `ServerContext`：纯依赖容器（注册表/事件总线/i18n/HTTP），**不含业务逻辑**。
//! - 断线清理逻辑在 [`crate::network::connection`]（对应 Java PlayerConnection 的 onClose 回调）。
//! - 会话挂起/恢复在 [`crate::session::SessionManager`]（对应 Java LocalSessionManager）。

use crate::eventbus::EventBus;
use crate::i18n::I18nService;
use crate::phira::PhiraFetcher;
use crate::player::PlayerRegistry;
use crate::room::RoomRegistry;
use crate::session::SessionManager;
use clap::Parser;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, RwLock};
use tokio::sync::Notify;
use tracing::info;

/// 命令行参数（1.2 节）。
#[derive(Parser, Debug, Clone)]
#[command(name = "phira-mp", about = "Phira multiplayer server")]
pub struct ServerArgs {
    /// 监听端口
    #[arg(long, default_value_t = 12346, value_parser = clap::value_parser!(u16).range(1..))]
    pub port: u16,

    /// 绑定地址
    #[arg(long, default_value = "0.0.0.0")]
    pub host: String,

    /// 启用 HAProxy PROXY 协议
    #[arg(long, default_value_t = false)]
    pub proxy_protocol: bool,

    /// 默认玩家语言
    #[arg(long, default_value = "zh-CN")]
    pub language: String,

    /// 会话挂起超时（秒）
    #[arg(long, default_value_t = 300)]
    pub session_timeout: u64,

    /// Phira API Base URL
    #[arg(long, default_value = "https://phira.5wyxi.com/")]
    pub phira_api: String,

    /// 对局录制输出目录（不设置则禁用录制）
    #[arg(long)]
    pub record_dir: Option<String>,
}

/// 服务器扩展点（对应 Java 的静态函数替换 / 初始 handler 接管）。
pub struct Extensions {
    /// 初始 handler 工厂：每连接的第一个 PacketHandler（默认 `AuthenticateHandler`）。
    pub initial_handler: RwLock<crate::network::handler::InitialHandlerFactory>,
}

impl Default for Extensions {
    fn default() -> Self {
        Self {
            initial_handler: RwLock::new(Arc::new(|| {
                Box::new(crate::network::authenticate_handler::AuthenticateHandler)
            })),
        }
    }
}

pub struct ServerContext {
    pub args: ServerArgs,
    pub players: PlayerRegistry,
    pub rooms: RoomRegistry,
    pub sessions: Arc<SessionManager>,
    pub phira: PhiraFetcher,
    pub i18n: I18nService,
    pub events: EventBus,
    pub extensions: Extensions,
    pub record_dir: Option<String>,
    /// 实际监听地址（run 中绑定后填入，格式 host:port）。
    pub listen_addr: RwLock<Option<String>>,
    /// server 完全停止后通知（测试用）。
    stopped: Notify,
    shutdown: Notify,
    shutdown_requested: AtomicBool,
}

static GLOBAL_CTX: RwLock<Option<Arc<ServerContext>>> = RwLock::new(None);

pub(crate) fn with_server_ctx<R>(f: impl FnOnce(&Arc<ServerContext>) -> R) -> Option<R> {
    GLOBAL_CTX.read().unwrap().as_ref().map(f)
}

/// 全局上下文访问（发包事件等）。
pub fn global_ctx() -> Option<Arc<ServerContext>> {
    with_server_ctx(|ctx| ctx.clone())
}

/// 测试辅助：读全局 ctx 的监听地址。
pub fn test_listen_addr() -> Option<String> {
    with_server_ctx(|ctx| ctx.listen_addr.read().unwrap().clone()).flatten()
}

/// 测试辅助：取全局 ctx。
pub fn test_global_ctx() -> Option<Arc<ServerContext>> {
    with_server_ctx(|ctx| ctx.clone())
}

impl ServerContext {
    pub fn new(args: ServerArgs) -> Arc<Self> {
        let sessions = Arc::new(SessionManager::new());
        sessions.set_timeout(std::time::Duration::from_secs(args.session_timeout));
        let ctx = Arc::new(Self {
            i18n: I18nService::new(args.language.clone()),
            phira: PhiraFetcher::new(args.phira_api.clone()),
            record_dir: args.record_dir.clone(),
            args,
            players: PlayerRegistry::new(),
            rooms: RoomRegistry::new(),
            sessions,
            events: EventBus::new(),
            extensions: Extensions::default(),
            listen_addr: RwLock::new(None),
            stopped: Notify::new(),
            shutdown: Notify::new(),
            shutdown_requested: AtomicBool::new(false),
        });
        *GLOBAL_CTX.write().unwrap() = Some(ctx.clone());
        ctx
    }

    pub fn request_shutdown(&self) {
        self.shutdown_requested.store(true, Ordering::SeqCst);
        self.shutdown.notify_waiters();
    }

    pub async fn wait_shutdown(&self) {
        self.shutdown.notified().await;
    }

    /// 等待 server 完全停止（测试用）。
    pub async fn wait_stopped(&self) {
        self.stopped.notified().await;
    }
}

/// 启动服务端（1.1 节；对应 Java `Server.main` + 生命周期事件）。
pub async fn run(args: ServerArgs) -> std::io::Result<()> {
    let ctx = ServerContext::new(args);

    // 控制台命令线程（对应 Java CommandService）
    crate::command::start_command_thread(ctx.clone());

    let listener =
        tokio::net::TcpListener::bind(format!("{}:{}", ctx.args.host, ctx.args.port)).await?;
    *ctx.listen_addr.write().unwrap() = Some(listener.local_addr()?.to_string());
    info!(host = %ctx.args.host, port = ctx.args.port, "phira-mp server started");
    ctx.events
        .post(crate::events::SERVER_LIFECYCLE, crate::events::ServerLifecycleEvent {
            phase: crate::events::LifecyclePhase::Started,
        })
        .await;

    // ctrl-c → 优雅关闭
    {
        let ctx = ctx.clone();
        tokio::spawn(async move {
            if tokio::signal::ctrl_c().await.is_ok() {
                ctx.request_shutdown();
            }
        });
    }

    loop {
        tokio::select! {
            accept = listener.accept() => {
                match accept {
                    Ok((stream, _addr)) => {
                        let ctx = ctx.clone();
                        let proxy = ctx.args.proxy_protocol;
                        tokio::spawn(async move {
                            let initial = (ctx.extensions.initial_handler.read().unwrap())();
                            crate::network::connection::spawn_connection(stream, ctx, proxy, initial).await;
                        });
                    }
                    Err(e) => {
                        tracing::warn!("accept error: {e}");
                    }
                }
            }
            _ = ctx.wait_shutdown() => {
                break;
            }
        }
    }

    // 关闭流程（1.3 节）：逐玩家踢出（离房语义由断线清理完成）
    ctx.events
        .post(crate::events::SERVER_LIFECYCLE, crate::events::ServerLifecycleEvent {
            phase: crate::events::LifecyclePhase::Stopping,
        })
        .await;
    info!("server stopping: kicking all players");
    for player in ctx.players.online_players() {
        player.kick();
        player.connection().close().await;
    }
    drop(listener);
    info!("server stopped");
    ctx.events
        .post(crate::events::SERVER_LIFECYCLE, crate::events::ServerLifecycleEvent {
            phase: crate::events::LifecyclePhase::Stopped,
        })
        .await;
    ctx.stopped.notify_waiters();
    Ok(())
}
