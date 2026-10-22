//! 玩家身份、本地连接实现与全局注册表。
//!
//! - [`Player`] 定义身份、扩展数据和生命周期钩子，自定义玩家无需持有本地连接。
//! - [`LocalPlayer`] 通过 [`ConnectionReference`] 管理连接换绑、发包和在线状态。
//! - [`PlayerRegistry`] 负责按用户 ID 注册玩家、恢复会话和清理注册。
//! - 数据源 provider 允许替换认证、谱面和成绩查询。
//!
//! 具体实现按职责拆分，公开类型与函数统一从本模块导出。

mod local;
mod providers;
mod registry;

pub use local::{ConnectionReference, LocalPlayer, local_of};
pub use providers::{
    AuthProvider, ChartProvider, Providers, RecordProvider, auth_provider, chart_provider,
    record_provider, set_auth_provider, set_chart_provider, set_record_provider,
};
pub use registry::{PlayerRegistry, ResolveResult, ResolveType, SuspendedContext};

use crate::network::connection::{ConnectionHandle, DisconnectReason};
use crate::packet::clientbound::SharedFrame;
use crate::phira::UserInfo;
use std::any::Any;
use std::sync::Arc;

/// 玩家领域身份。实现者无需关心网络连接（连接由 [`LocalPlayer`] 或 handler 层管理）。
///
/// # 自定义玩家（假想 `RemotePlayer`）需要做什么
/// 1. `impl Player for RemotePlayer`：提供 id/name/user_info；覆盖 [`Player::as_any`]；
///    按需覆盖扩展槽、连接状态与生命周期钩子。
/// 2. 注册：调用 [`PlayerRegistry::resolve_player`]，constructor 构造 `Arc<RemotePlayer>`，
///    resumer 定义「旧玩家 → 新连接」的接管语义（无连接玩家可直接 `Ok(None)`）。
/// 3. 此后房间/事件/协议层只经 `Arc<dyn Player>` 使用该玩家——全链路无感知。
pub trait Player: Send + Sync + 'static {
    fn id(&self) -> i32;
    fn name(&self) -> String;
    fn user_info(&self) -> Arc<UserInfo>;

    fn language(&self) -> Option<String> {
        self.user_info().language.clone()
    }

    /// downcast 支持（自定义实现返回 `self`）。
    fn as_any(&self) -> &dyn Any;

    // ---- 扩展数据槽（对应 Java 自定义 Player 子类的字段） ----

    /// 挂接扩展数据（按 `TypeId` 存取；默认实现忽略）。
    fn set_extension_typed(&self, _type_id: std::any::TypeId, _ext: Arc<dyn Any + Send + Sync>) {}

    /// 按 `TypeId` 取扩展数据（默认实现返回 None）。
    fn extension_typed(&self, _type_id: std::any::TypeId) -> Option<Arc<dyn Any + Send + Sync>> {
        None
    }

    // ---- 连接/生命周期钩子（默认实现；自定义玩家按需覆写） ----

    /// 当前绑定的连接句柄（无连接的实现返回 None）。
    fn bound_connection(&self) -> Option<ConnectionHandle> {
        None
    }

    /// 是否在线（默认 false；LocalPlayer 按连接状态判断）。
    fn is_online(&self) -> bool {
        false
    }

    /// 掉线时是否允许挂起会话（RemotePlayer 等无连接实现覆写为 false）。
    fn can_suspend(&self) -> bool {
        true
    }

    /// 会话关闭钩子（对应 Java `closeBinder`；默认空实现）。
    /// 框架断线清理时调用；无连接的实现由其宿主在会话结束时自行调用。
    fn on_session_closed(&self, _reason: DisconnectReason) {}

    /// 发送预编码共享帧（广播零拷贝路径；无连接实现覆写为转发到远端）。
    fn send_frame<'a>(&'a self, _frame: SharedFrame) -> futures::future::BoxFuture<'a, ()> {
        Box::pin(async {})
    }

    /// 踢下线（默认空实现）。
    fn kick(&self) {}
}

/// 泛型便捷：设置扩展数据。
pub fn set_extension<T: Any + Send + Sync>(player: &dyn Player, ext: T) {
    player.set_extension_typed(std::any::TypeId::of::<T>(), Arc::new(ext));
}

/// 泛型便捷：按类型取扩展数据。
pub fn extension<T: Any + Send + Sync>(player: &dyn Player) -> Option<Arc<T>> {
    player
        .extension_typed(std::any::TypeId::of::<T>())?
        .downcast::<T>()
        .ok()
}

/// downcast：trait 对象 → 具体类型引用（经 [`Player::as_any`]）。
pub fn downcast_player<T: Player>(player: &dyn Player) -> Option<&T> {
    player.as_any().downcast_ref::<T>()
}

/// `dyn Player` 的 Debug（经 id/name 呈现；自定义实现无需自带 Debug）。
impl std::fmt::Debug for dyn Player {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Player")
            .field("id", &self.id())
            .field("name", &self.name())
            .finish()
    }
}
