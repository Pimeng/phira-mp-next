//! 服务端共享依赖、全局上下文访问与关闭信号。

use super::{Extensions, ServerArgs};
use crate::ban::BanManager;
use crate::eventbus::EventBus;
use crate::i18n::I18nService;
use crate::phira::{PhiraFetcher, PhiraFetcherConfig};
use crate::player::PlayerRegistry;
use crate::room::RoomRegistry;
use crate::session::SessionManager;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, RwLock};
use tokio::sync::Notify;

pub struct ServerContext {
    pub args: ServerArgs,
    pub players: PlayerRegistry,
    pub rooms: RoomRegistry,
    pub sessions: Arc<SessionManager>,
    /// 封禁管理（全局封禁 + 房间封禁；控制台命令数据源）。
    pub bans: BanManager,
    pub phira: PhiraFetcher,
    pub i18n: I18nService,
    pub events: EventBus,
    pub extensions: Extensions,
    pub record_dir: Option<String>,
    /// 实际偷听地址（run 中绑定后填入，格式 host:port）。
    pub listen_addr: RwLock<Option<String>>,
    /// 实际 HTTP 偷听地址（http_port > 0 时填入，格式 host:port）。
    pub http_addr: RwLock<Option<String>>,
    /// 活跃连接数（对应 Java `allChannels`，关闭日志用）。
    pub active_connections: AtomicUsize,
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

/// 测试辅助：读全局 ctx 的偷听地址。
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
            phira: PhiraFetcher::new(PhiraFetcherConfig::from(&args)),
            record_dir: args.record_dir.clone(),
            args,
            players: PlayerRegistry::new(),
            rooms: RoomRegistry::new(),
            sessions,
            bans: BanManager::new(),
            events: EventBus::new(),
            extensions: Extensions::default(),
            listen_addr: RwLock::new(None),
            http_addr: RwLock::new(None),
            active_connections: AtomicUsize::new(0),
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

    /// 是否已请求关闭（控制台 is_running 用）。
    pub fn is_shutdown_requested(&self) -> bool {
        self.shutdown_requested.load(Ordering::SeqCst)
    }

    pub async fn wait_shutdown(&self) {
        self.shutdown.notified().await;
    }

    pub(super) fn notify_stopped(&self) {
        self.stopped.notify_waiters();
    }

    /// 等待 server 完全停止（测试用）。
    pub async fn wait_stopped(&self) {
        self.stopped.notified().await;
    }
}
