//! 开局、准备、取消准备和中止对局。

use super::RoomHandler;
use crate::events;
use crate::network::handler::{HandleOutcome, HandlerContext};
use crate::packet::PacketResult;
use crate::packet::clientbound::ClientBoundPacket;

impl RoomHandler {
    pub(super) async fn on_request_start(&self, ctx: &HandlerContext) -> HandleOutcome {
        // GameRequireStartEvent：可取消
        let event = events::GameRequireStartEvent {
            room: self.room.clone(),
            player: self.player.clone(),
            cancel_reason: None,
        };
        let event = ctx
            .server
            .events
            .post_mut(events::GAME_REQUIRE_START, event)
            .await;
        if let Some(reason) = event.cancel_reason {
            ctx.send(ClientBoundPacket::request_start_result(
                PacketResult::failed(reason),
            ))
            .await;
            return HandleOutcome::Ok;
        }

        let room = self.room.clone();
        let user_id = self.player.id();
        let outcome = self
            .respond_to_action(
                ctx,
                move || room.require_start(user_id),
                ClientBoundPacket::request_start_result,
            )
            .await;

        ctx.server
            .events
            .post(
                events::GAME_START,
                events::GameStartEvent {
                    room: self.room.clone(),
                    player: self.player.clone(),
                },
            )
            .await;
        self.notify_state_change(ctx).await;
        if matches!(
            self.room.game_state_protocol(),
            crate::packet::state::GameState::Playing
        ) {
            ctx.server
                .events
                .post(
                    events::GAME_PLAYING_START,
                    events::GamePlayingStartEvent {
                        room: self.room.clone(),
                    },
                )
                .await;
        }
        outcome
    }

    pub(super) async fn on_ready(&self, ctx: &HandlerContext) -> HandleOutcome {
        let room = self.room.clone();
        let user_id = self.player.id();

        match room.ready(user_id) {
            Ok((plan, started)) => {
                ctx.send(ClientBoundPacket::ready_result(PacketResult::ok()))
                    .await;
                crate::room::send_broadcasts(plan).await;
                ctx.server
                    .events
                    .post(
                        events::PLAYER_READY,
                        events::PlayerReadyEvent {
                            room: self.room.clone(),
                            player: self.player.clone(),
                        },
                    )
                    .await;
                if started {
                    self.notify_state_change(ctx).await;
                    ctx.server
                        .events
                        .post(
                            events::GAME_PLAYING_START,
                            events::GamePlayingStartEvent {
                                room: self.room.clone(),
                            },
                        )
                        .await;
                }
                HandleOutcome::Ok
            }
            Err(error) => {
                let msg = ctx
                    .server
                    .i18n
                    .message(self.player.language().as_deref(), error.0);
                ctx.send(ClientBoundPacket::ready_result(PacketResult::failed(msg)))
                    .await;
                HandleOutcome::Ok
            }
        }
    }

    pub(super) async fn on_cancel_ready(&self, ctx: &HandlerContext) -> HandleOutcome {
        let room = self.room.clone();
        let user_id = self.player.id();
        let outcome = self
            .respond_to_action(
                ctx,
                move || room.cancel_ready(user_id),
                ClientBoundPacket::cancel_ready_result,
            )
            .await;
        ctx.server
            .events
            .post(
                events::PLAYER_CANCEL_READY,
                events::PlayerCancelReadyEvent {
                    room: self.room.clone(),
                    player: self.player.clone(),
                },
            )
            .await;
        outcome
    }

    pub(super) async fn on_abort(&self, ctx: &HandlerContext) -> HandleOutcome {
        let room = self.room.clone();
        let user_id = self.player.id();
        let outcome = self
            .respond_to_action(
                ctx,
                move || room.commit_abort(user_id).map(|out| out.broadcasts),
                ClientBoundPacket::abort_result,
            )
            .await;
        ctx.server
            .events
            .post(
                events::GAME_ABORT,
                events::GameAbortEvent {
                    room: self.room.clone(),
                    player: self.player.clone(),
                },
            )
            .await;
        outcome
    }
}
