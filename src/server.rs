//! 服务端装配与生命周期（第 1 节）。
//!
//! `ServerContext` 是全局上下文（玩家/房间/会话注册表 + Phira 客户端 + i18n）。
//! 为避免模块间循环引用，提供一个进程级 `OnceLock` 访问点（与 Java 静态注册表语义一致）。

use crate::eventbus::EventBus;
use crate::i18n::I18nService;
use crate::network::connection::{ConnectionHandle, DisconnectReason};
use crate::phira::PhiraFetcher;
use crate::player::{Player, PlayerRegistry};
use crate::room::{Room, RoomRegistry};
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

pub struct ServerContext {
    pub args: ServerArgs,
    pub players: PlayerRegistry,
    pub rooms: RoomRegistry,
    pub sessions: Arc<SessionManager>,
    pub phira: PhiraFetcher,
    pub i18n: I18nService,
    pub events: EventBus,
    pub record_dir: Option<String>,
    /// 实际监听地址（run 中绑定后填入，格式 host:port）。
    pub listen_addr: RwLock<Option<String>>,
    /// server 完全停止后通知（测试用）。
    stopped: Notify,
    shutdown: Notify,
    shutdown_requested: AtomicBool,
    /// 连接 → 房间 索引（current_room_of 用；玩家对象本身不持房间引用）。
    player_rooms: RwLock<std::collections::HashMap<i32, Arc<Room>>>,
}

static GLOBAL_CTX: RwLock<Option<Arc<ServerContext>>> = RwLock::new(None);

pub(crate) fn with_server_ctx<R>(f: impl FnOnce(&Arc<ServerContext>) -> R) -> Option<R> {
    GLOBAL_CTX.read().unwrap().as_ref().map(f)
}

/// 测试辅助：读全局 ctx 的监听地址。
pub fn test_listen_addr() -> Option<String> {
    with_server_ctx(|ctx| ctx.listen_addr.read().unwrap().clone()).flatten()
}

/// 测试辅助：取全局 ctx。
pub fn test_global_ctx() -> Option<Arc<ServerContext>> {
    with_server_ctx(|ctx| ctx.clone())
}

/// 「玩家当前所在房间」索引（Java 版由 handler 位置推断；Rust 版用显式索引）。
pub(crate) fn current_room_of(user_id: i32) -> Option<Arc<Room>> {
    with_server_ctx(|ctx| ctx.player_rooms.read().unwrap().get(&user_id).cloned()).flatten()
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
            listen_addr: RwLock::new(None),
            stopped: Notify::new(),
            shutdown: Notify::new(),
            shutdown_requested: AtomicBool::new(false),
            player_rooms: RwLock::new(std::collections::HashMap::new()),
        });
        *GLOBAL_CTX.write().unwrap() = Some(ctx.clone());
        ctx
    }

    /// 玩家进入房间（建立索引）。
    pub fn enter_room(&self, user_id: i32, room: Arc<Room>) {
        self.player_rooms.write().unwrap().insert(user_id, room);
    }

    pub fn leave_room_index(&self, user_id: i32) {
        self.player_rooms.write().unwrap().remove(&user_id);
    }

    /// 断线清理（5.3/5.4 节）：由连接读循环结束时调用。
    ///
    /// - 若连接已换绑（player.connection().id != conn.id）→ 跳过。
    /// - 未入房（room=None）→ 直接移除注册。
    /// - Monitor → 直接离开房间。
    /// - 普通玩家 → 清理现场后挂起会话。
    pub async fn on_connection_closed(
        &self,
        conn: &ConnectionHandle,
        player: &Arc<Player>,
        room: Option<Arc<Room>>,
        reason: DisconnectReason,
    ) {
        // 已换绑/踢旧：跳过
        if player.connection().id() != conn.id() {
            return;
        }

        self.events.post("player.disconnect", &reason);

        let Some(room) = room else {
            // 未入房：直接移除注册
            self.players.remove_if_bound(player.id(), conn.id());
            return;
        };

        // Monitor 不挂起，直接离开
        if room.is_monitor_user(player.id()) {
            let (_left, broadcasts, _destroyed) = room.leave(player.id());
            for (target, packet) in broadcasts {
                target.send(packet).await;
            }
            self.leave_room_index(player.id());
            self.players.remove_if_bound(player.id(), conn.id());
            return;
        }

        if !room.contains_member(player.id()) {
            self.leave_room_index(player.id());
            self.players.remove_if_bound(player.id(), conn.id());
            return;
        }

        // 清理对局现场（WaitForReady→cancelReady；Playing→abort）
        let broadcasts = room.cleanup_for_suspend(player.id());
        for (target, packet) in broadcasts {
            target.send(packet).await;
        }

        // 挂起会话（5 分钟超时，超时后 forceLeave + 移除注册）
        self.sessions.suspend(player.id(), room, player.clone());
        info!(user_id = player.id(), "session suspended");
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

/// 启动服务端（1.1 节）。
pub async fn run(args: ServerArgs) -> std::io::Result<()> {
    let ctx = ServerContext::new(args);

    // 控制台命令线程
    crate::command::start_command_thread(ctx.clone());

    let listener =
        tokio::net::TcpListener::bind(format!("{}:{}", ctx.args.host, ctx.args.port)).await?;
    *ctx.listen_addr.write().unwrap() = Some(listener.local_addr()?.to_string());
    info!(host = %ctx.args.host, port = ctx.args.port, "phira-mp server started");

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
                            crate::network::handle_connection(stream, ctx, proxy).await;
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

    // 关闭流程（1.3 节）：先离房（保持广播/房主转移语义），再踢+断开
    info!("server stopping: kicking all players");
    for player in ctx.players.all_players() {
        if let Some(room) = crate::server::current_room_of(player.id()) {
            let (_left, broadcasts, _destroyed) = room.leave(player.id());
            for (target, packet) in broadcasts {
                target.send(packet).await;
            }
            ctx.leave_room_index(player.id());
        }
        player.kick();
        player.connection().close().await;
    }
    drop(listener);
    info!("server stopped");
    ctx.stopped.notify_waiters();
    Ok(())
}
