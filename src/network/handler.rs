//! PacketHandler 链（7.5 节）。
//!
//! 阶段状态机：Authenticate → Play → Room。
//! 处理同一连接的包严格按到达顺序（读循环串行 await）。
//!
//! 对应 Java 的标记接口：
//! - [`PacketHandler::player_ref`]（对应 `PlayerHolder`）：绑定玩家。
//! - [`PacketHandler::room_ref`]（对应 `RoomHolder`）：所在房间（`LocalPlayer.getRoom()`
//!   的反查来源——玩家所在房间由 handler 位置决定）。
//! - [`PacketHandler::is_suspendable_room_holder`]（对应 `SuspendableRoomHolder`）：
//!   掉线时是否允许挂起会话（仅房间内玩家 true；Monitor/PlayHandler false）。

use crate::packet::serverbound::ServerBoundPacket;
use futures::future::BoxFuture;
use std::sync::Arc;

/// 包处理结果。
pub enum HandleOutcome {
    /// 正常完成。
    Ok,
    /// 业务失败：已回复 failed 包（handler 自行发送），连接保持。
    Failed,
    /// 关闭连接。
    Close,
    /// 切换到下一个 Handler（阶段推进）。
    Switch(Box<dyn PacketHandler>),
}

/// 阶段处理器（对应 Java `ServerBoundPacketHandler`）。
pub trait PacketHandler: Send + Sync + 'static {
    /// 处理一个入站包。在同一连接的读循环内串行调用。
    fn handle<'a>(
        &'a mut self,
        ctx: &'a HandlerContext,
        packet: ServerBoundPacket,
    ) -> BoxFuture<'a, HandleOutcome>;

    /// 当前 handler 绑定的玩家（对应 `PlayerHolder`）。
    fn player_ref(&self) -> Option<Arc<dyn crate::player::Player>> {
        None
    }

    /// 当前 handler 所在的房间（对应 `RoomHolder`）。
    fn room_ref(&self) -> Option<Arc<dyn crate::room::Room>> {
        None
    }

    /// 是否为可挂起的房间持有者（对应 `SuspendableRoomHolder`）。
    /// 仅 RoomHandler 中的非 Monitor 玩家返回 true。
    fn is_suspendable_room_holder(&self) -> bool {
        self.room_ref().is_some()
    }
}

/// Handler 运行上下文（连接句柄 + 全局上下文）。
pub struct HandlerContext {
    pub conn: crate::network::connection::ConnectionHandle,
    pub server: Arc<crate::server::ServerContext>,
}

impl HandlerContext {
    /// 便捷：回复一个包。
    pub async fn send(&self, packet: crate::packet::clientbound::ClientBoundPacket) {
        self.conn.send(packet).await;
    }

    /// 便捷：回复预编码共享帧（零拷贝路径）。
    pub async fn send_frame(&self, frame: crate::packet::clientbound::SharedFrame) {
        self.conn.send_frame(frame).await;
    }

    /// 关闭连接（flush 由 writer drain 保证）。
    pub async fn close(&self) {
        self.conn.close().await;
    }
}

/// 初始 handler 工厂（对应 Java 接管 `setPacketHandler` 的能力）：
/// 返回连接的第一个 handler（默认 `AuthenticateHandler`）。
pub type InitialHandlerFactory =
    Arc<dyn Fn() -> Box<dyn PacketHandler> + Send + Sync>;
