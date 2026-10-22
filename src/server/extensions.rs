//! 初始连接处理、断线清理和玩家解析的可替换扩展点。

use super::ServerContext;
use std::sync::{Arc, RwLock};

/// 断线清理覆盖 handler（None = 默认 `network::connection::on_connection_closed` 流程）。
/// 注册后完全接管断线处理（对应 Java `closeBinder` 的框架级槽位）。
pub type DisconnectHandler = Arc<
    dyn Fn(
            Arc<ServerContext>,
            crate::network::connection::ConnectionHandle,
            Arc<dyn crate::player::Player>,
            Option<Arc<dyn crate::room::Room>>,
            bool,
            crate::network::connection::DisconnectReason,
        ) -> futures::future::BoxFuture<'static, ()>
        + Send
        + Sync,
>;

/// 玩家解析覆盖 handler（None = 默认 `PlayerRegistry::resolve_or_resume`）。
/// 自定义玩家类型（如 RemotePlayer）时注册，定义创建/接管语义。
pub type PlayerResolver = Arc<
    dyn Fn(
            Arc<ServerContext>,
            Arc<crate::phira::UserInfo>,
            crate::network::connection::ConnectionHandle,
        ) -> futures::future::BoxFuture<
            'static,
            Result<
                (
                    crate::player::ResolveResult,
                    Option<crate::network::connection::ConnectionHandle>,
                ),
                String,
            >,
        > + Send
        + Sync,
>;

/// 服务器扩展点（对应 Java 的静态函数替换 / 初始 handler 接管）。
pub struct Extensions {
    /// 初始 handler 工厂：每连接的第一个 PacketHandler（默认 `AuthenticateHandler`）。
    pub initial_handler: RwLock<crate::network::handler::InitialHandlerFactory>,
    /// 断线清理覆盖（None = 默认流程）。
    pub disconnect_override: RwLock<Option<DisconnectHandler>>,
    /// 玩家解析覆盖（None = 默认 `resolve_or_resume`）。
    pub player_resolver: RwLock<Option<PlayerResolver>>,
}

impl Default for Extensions {
    fn default() -> Self {
        Self {
            initial_handler: RwLock::new(Arc::new(|| {
                Box::new(crate::network::authenticate_handler::AuthenticateHandler)
            })),
            disconnect_override: RwLock::new(None),
            player_resolver: RwLock::new(None),
        }
    }
}
