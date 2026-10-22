//! 房间阶段的包分发与共享回复逻辑。
//!
//! - `controls`：聊天、离房、锁房和循环设置。
//! - `chart`：选曲与谱面数据获取。
//! - `gameplay`：开局、准备和中止。
//! - `results`：成绩提交与对局录制。
//!
//! 房间操作同步生成广播计划；回复和广播在操作返回、锁已释放后发送。

mod chart;
mod controls;
mod gameplay;
mod results;

use super::handler::{HandleOutcome, HandlerContext, PacketHandler};
use crate::events;
use crate::log::log_debug;
use crate::packet::PacketResult;
use crate::packet::clientbound::ClientBoundPacket;
use crate::packet::serverbound::ServerBoundPacket;
use crate::player::Player;
use crate::room::behavior::Broadcast;
use crate::room::{GameResult, Room};
use futures::future::BoxFuture;
use std::sync::Arc;

pub struct RoomHandler {
    player: Arc<dyn Player>,
    room: Arc<dyn Room>,
    fallback: Option<Box<dyn PacketHandler>>,
}

impl RoomHandler {
    pub fn new(
        player: Arc<dyn Player>,
        room: Arc<dyn Room>,
        fallback: Box<dyn PacketHandler>,
    ) -> Self {
        Self {
            player,
            room,
            fallback: Some(fallback),
        }
    }

    /// 执行房间操作，先回复请求，再发送操作产生的广播计划。
    /// 操作失败时仍继续处理连接，错误通过相应结果包回复。
    async fn respond_to_action(
        &self,
        ctx: &HandlerContext,
        action: impl FnOnce() -> GameResult<Broadcast>,
        response: impl FnOnce(PacketResult<()>) -> ClientBoundPacket,
    ) -> HandleOutcome {
        match action() {
            Ok(plan) => {
                ctx.send(response(PacketResult::ok())).await;
                crate::room::send_broadcasts(plan).await;
            }
            Err(error) => {
                let message = ctx
                    .server
                    .i18n
                    .message(self.player.language().as_deref(), error.0);
                ctx.send(response(PacketResult::failed(message))).await;
            }
        }
        HandleOutcome::Ok
    }

    async fn notify_state_change(&self, ctx: &HandlerContext) {
        ctx.server
            .events
            .post(
                events::ROOM_STATE_CHANGE,
                events::RoomStateChangeEvent {
                    room: self.room.clone(),
                    new_state: self.room.game_state_protocol(),
                },
            )
            .await;
    }

    fn close_unexpected_packet(&self, ctx: &HandlerContext) -> HandleOutcome {
        log_debug!(
            &ctx.server.i18n,
            "LOG_ROOM_UNEXPECTED_PACKET",
            ("id", self.player.id()),
        );
        HandleOutcome::Close
    }
}

impl PacketHandler for RoomHandler {
    fn player_ref(&self) -> Option<Arc<dyn Player>> {
        Some(self.player.clone())
    }

    fn room_ref(&self) -> Option<Arc<dyn Room>> {
        Some(self.room.clone())
    }

    fn is_suspendable_room_holder(&self) -> bool {
        // 仅非 Monitor 玩家可挂起（对应 Java SuspendableRoomHolder）
        !self.room.is_monitor_user(self.player.id())
    }

    fn handle<'a>(
        &'a mut self,
        ctx: &'a HandlerContext,
        packet: ServerBoundPacket,
    ) -> BoxFuture<'a, HandleOutcome> {
        Box::pin(async move {
            match packet {
                ServerBoundPacket::Ping => {
                    ctx.send(ClientBoundPacket::pong()).await;
                    HandleOutcome::Ok
                }
                ServerBoundPacket::Chat { message, .. } => self.on_chat(ctx, message).await,
                ServerBoundPacket::LeaveRoom { .. } => self.on_leave_room(ctx).await,
                ServerBoundPacket::LockRoom { .. } => self.on_lock_room(ctx).await,
                ServerBoundPacket::CycleRoom { .. } => self.on_cycle_room(ctx).await,
                ServerBoundPacket::SelectChart { id, .. } => self.on_select_chart(ctx, id).await,
                ServerBoundPacket::RequestStart { .. } => self.on_request_start(ctx).await,
                ServerBoundPacket::Ready { .. } => self.on_ready(ctx).await,
                ServerBoundPacket::CancelReady { .. } => self.on_cancel_ready(ctx).await,
                ServerBoundPacket::Played { record_id, .. } => self.on_played(ctx, record_id).await,
                ServerBoundPacket::Abort { .. } => self.on_abort(ctx).await,
                // touch/judge：收集 + 转发 monitor（无回复包）
                ServerBoundPacket::Touches { frames, .. } => {
                    crate::room::send_broadcasts(self.room.touch_send(self.player.id(), frames))
                        .await;
                    HandleOutcome::Ok
                }
                ServerBoundPacket::Judges { judges, .. } => {
                    crate::room::send_broadcasts(self.room.judge_send(self.player.id(), judges))
                        .await;
                    HandleOutcome::Ok
                }
                // 房间内收到大厅/认证包 → 踢（对应 Java RoomHandler.kick()）
                ServerBoundPacket::Authenticate { .. }
                | ServerBoundPacket::CreateRoom { .. }
                | ServerBoundPacket::JoinRoom { .. } => self.close_unexpected_packet(ctx),
            }
        })
    }
}
