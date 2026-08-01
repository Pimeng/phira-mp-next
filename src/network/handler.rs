//! PacketHandler 链（7.5 节）。
//!
//! 阶段状态机：Authenticate → Play → Room。
//! 处理同一连接的包严格按到达顺序（读循环串行 await）。

use crate::packet::serverbound::ServerBoundPacket;
use crate::room::Room;
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

/// 阶段处理器。
pub trait PacketHandler: Send + 'static {
    /// 处理一个入站包。在同一连接的读循环内串行调用。
    fn handle<'a>(
        &'a mut self,
        ctx: &'a HandlerContext,
        packet: ServerBoundPacket,
    ) -> BoxFuture<'a, HandleOutcome>;

    /// 当前 handler 所在的房间（会话挂起用，仅 RoomHandler 返回 Some）。
    fn room_ref(&self) -> Option<Arc<Room>> {
        None
    }

    /// 当前 handler 绑定的玩家（断线清理/会话挂起用）。
    fn player_ref(&self) -> Option<std::sync::Arc<crate::player::Player>> {
        None
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

    /// 回复失败消息（构造对应包的 failed PacketResult 由调用方完成）。
    pub async fn close(&self) {
        self.conn.close().await;
    }
}
