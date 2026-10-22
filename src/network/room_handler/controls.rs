//! 聊天、离房和房间设置；负责回复、广播与扩展事件的顺序。

use super::RoomHandler;
use crate::events;
use crate::network::handler::{HandleOutcome, HandlerContext};
use crate::packet::PacketResult;
use crate::packet::clientbound::ClientBoundPacket;

impl RoomHandler {
    pub(super) async fn on_chat(&self, ctx: &HandlerContext, content: String) -> HandleOutcome {
        // RoomChatEvent：可改写内容，可取消
        let event = events::RoomChatEvent {
            room: self.room.clone(),
            player: self.player.clone(),
            message: content,
            cancel_reason: None,
        };
        let event = ctx.server.events.post_mut(events::ROOM_CHAT, event).await;
        if let Some(reason) = event.cancel_reason {
            ctx.send(ClientBoundPacket::chat_result(PacketResult::failed(reason)))
                .await;
            return HandleOutcome::Ok;
        }

        let room = self.room.clone();
        let user_id = self.player.id();
        self.respond_to_action(
            ctx,
            move || room.chat(user_id, event.message),
            ClientBoundPacket::chat_result,
        )
        .await
    }

    pub(super) async fn on_leave_room(&mut self, ctx: &HandlerContext) -> HandleOutcome {
        let (left, plan, _destroyed) = self.room.leave(self.player.id());
        if !left {
            let msg = ctx
                .server
                .i18n
                .message(self.player.language().as_deref(), "ERROR_ROOM_NOT_FOUND");
            ctx.send(ClientBoundPacket::leave_room_result(PacketResult::failed(
                msg,
            )))
            .await;
            return HandleOutcome::Ok;
        }
        ctx.send(ClientBoundPacket::leave_room_result(PacketResult::ok()))
            .await;
        crate::room::send_broadcasts(plan).await;
        ctx.server
            .events
            .post(
                events::PLAYER_LEAVE_ROOM,
                events::PlayerLeaveRoomEvent {
                    player: self.player.clone(),
                    room: self.room.clone(),
                },
            )
            .await;
        // 离房切回 fallback（对应 Java setPacketHandler(fallback)）
        match self.fallback.take() {
            Some(fallback) => HandleOutcome::Switch(fallback),
            None => HandleOutcome::Close,
        }
    }

    pub(super) async fn on_lock_room(&self, ctx: &HandlerContext) -> HandleOutcome {
        let locked = !self.room.setting().locked;
        ctx.server
            .events
            .post(
                events::ROOM_LOCK_CHANGE,
                events::RoomLockChangeEvent {
                    room: self.room.clone(),
                    player: self.player.clone(),
                    locked,
                },
            )
            .await;
        let room = self.room.clone();
        let user_id = self.player.id();
        self.respond_to_action(
            ctx,
            move || room.toggle_lock(user_id),
            ClientBoundPacket::lock_room_result,
        )
        .await
    }

    pub(super) async fn on_cycle_room(&self, ctx: &HandlerContext) -> HandleOutcome {
        let cycle = !self.room.setting().cycle;
        ctx.server
            .events
            .post(
                events::ROOM_CYCLE_CHANGE,
                events::RoomCycleChangeEvent {
                    room: self.room.clone(),
                    player: self.player.clone(),
                    cycle,
                },
            )
            .await;
        let room = self.room.clone();
        let user_id = self.player.id();
        self.respond_to_action(
            ctx,
            move || room.toggle_cycle(user_id),
            ClientBoundPacket::cycle_room_result,
        )
        .await
    }
}
