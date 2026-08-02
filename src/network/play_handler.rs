//! PlayHandler（7.6 节）：大厅阶段（建房/进房/心跳）。
//!
//! 与 Java 版一致的扁平化结构：`handle` 只做分发，每个分支一个独立小方法；
//! 对应 Java `PlayHandler implements PlayerHolder`（持玩家，不持房间）。

use super::handler::{HandleOutcome, HandlerContext, PacketHandler};
use crate::events;
use crate::packet::clientbound::{ClientBoundPacket, JoinRoomData};
use crate::packet::serverbound::ServerBoundPacket;
use crate::packet::PacketResult;
use crate::player::Player;
use crate::room::{JoinOutcome, RoomSetting};
use futures::future::BoxFuture;
use std::sync::Arc;
use tracing::debug;

/// 每玩家建房冷却（500ms）。
const CREATE_ROOM_COOLDOWN: std::time::Duration = std::time::Duration::from_millis(500);

pub struct PlayHandler {
    player: Arc<dyn Player>,
    last_create: std::time::Instant,
}

impl PlayHandler {
    pub fn new(player: Arc<dyn Player>) -> Self {
        Self {
            player,
            last_create: std::time::Instant::now()
                .checked_sub(std::time::Duration::from_secs(10))
                .unwrap_or_else(std::time::Instant::now),
        }
    }

    fn room_handler(&self, room: Arc<dyn crate::room::Room>) -> Box<dyn PacketHandler> {
        Box::new(super::room_handler::RoomHandler::new(
            self.player.clone(),
            room,
            Box::new(PlayHandler::new(self.player.clone())),
        ))
    }

    async fn reply(&self, ctx: &HandlerContext, packet: ClientBoundPacket) {
        ctx.send(packet).await;
    }

    async fn fail(&self, ctx: &HandlerContext, key: &str, make: impl FnOnce(PacketResult<()>) -> ClientBoundPacket) -> HandleOutcome {
        let msg = ctx.server.i18n.message(self.player.language().as_deref(), key);
        self.reply(ctx, make(PacketResult::failed(msg))).await;
        HandleOutcome::Ok
    }

    // ---- 建房 ----

    async fn on_create_room(&mut self, ctx: &HandlerContext, room_id: String) -> HandleOutcome {
        let mk = |r: PacketResult<()>| ClientBoundPacket::create_room_result(r);

        if room_id.is_empty() {
            return self.fail(ctx, "error.invalid_room_id", mk).await;
        }
        if self.last_create.elapsed() < CREATE_ROOM_COOLDOWN {
            return self.fail(ctx, "error.rate_limited", mk).await;
        }

        // RoomPreCreateEvent：可改写 setting，可取消
        let ev = events::RoomPreCreateEvent {
            creator: self.player.clone(),
            room_id: room_id.clone(),
            setting: RoomSetting::default(),
            cancel_reason: None,
        };
        let ev = ctx.server.events.post_mut(events::ROOM_PRE_CREATE, ev).await;
        if let Some(reason) = ev.cancel_reason {
            self.reply(ctx, mk(PacketResult::failed(reason))).await;
            return HandleOutcome::Ok;
        }

        let room = match ctx.server.rooms.create_room_with(&room_id, ev.setting) {
            Ok(r) => r,
            Err(e) => {
                return self.fail(ctx, e.0, mk).await;
            }
        };

        // 创建者加入房间：刚创建的空房间必然可加入；若失败属内部错误，快速失败
        // （对应 Java PlayHandler.handleCreateRoom 的 room.join(player, false)）
        let (_outcome, plan) = match room.join(self.player.clone(), false) {
            Ok(v) => v,
            Err(e) => {
                tracing::error!("creator join failed (room {}): {}", room.id(), e.0);
                ctx.close().await;
                return HandleOutcome::Close;
            }
        };
        // 锁外发送 join 广播（首个玩家通常为空计划）
        crate::room::send_broadcasts(plan).await;

        self.last_create = std::time::Instant::now();
        ctx.server.events.post(events::ROOM_POST_CREATE, events::RoomPostCreateEvent {
            room: room.clone(),
            creator: self.player.clone(),
        }).await;
        self.reply(ctx, mk(PacketResult::ok())).await;
        HandleOutcome::Switch(self.room_handler(room))
    }

    // ---- 进房 ----

    async fn on_join_room(&mut self, ctx: &HandlerContext, room_id: String, monitor: bool) -> HandleOutcome {
        let i18n = &ctx.server.i18n;
        let lang = self.player.language();
        let mk = |r: PacketResult<JoinRoomData>| ClientBoundPacket::join_room_result(r);

        let Some(room) = ctx.server.rooms.find_room(&room_id) else {
            let msg = i18n.message(lang.as_deref(), "error.room_not_found");
            self.reply(ctx, mk(PacketResult::failed(msg))).await;
            return HandleOutcome::Ok;
        };

        // PlayerPreJoinRoomEvent：可取消
        let ev = events::PlayerPreJoinRoomEvent {
            player: self.player.clone(),
            room: room.clone(),
            monitor,
            cancel_reason: None,
        };
        let ev = ctx.server.events.post_mut(events::PLAYER_PRE_JOIN_ROOM, ev).await;
        if let Some(reason) = ev.cancel_reason {
            self.reply(ctx, mk(PacketResult::failed(reason))).await;
            return HandleOutcome::Ok;
        }

        // PlayerPostJoinRoomEvent：可取消（对应 Java 二次确认点）
        let ev = events::PlayerPostJoinRoomEvent {
            player: self.player.clone(),
            room: room.clone(),
            cancel_reason: None,
        };
        let ev = ctx.server.events.post_mut(events::PLAYER_POST_JOIN_ROOM, ev).await;
        if let Some(reason) = ev.cancel_reason {
            self.reply(ctx, mk(PacketResult::failed(reason))).await;
            return HandleOutcome::Ok;
        }

        let (outcome, plan) = match room.join(self.player.clone(), monitor) {
            Ok(v) => v,
            Err(e) => {
                let msg = i18n.message(lang.as_deref(), e.0);
                self.reply(ctx, mk(PacketResult::failed(msg))).await;
                return HandleOutcome::Ok;
            }
        };
        if matches!(outcome, JoinOutcome::AlreadyIn) {
            self.reply(ctx, mk(PacketResult::Success(room.join_room_data()))).await;
            return HandleOutcome::Ok;
        }

        crate::room::send_broadcasts(plan).await;
        self.reply(ctx, mk(PacketResult::Success(room.join_room_data()))).await;
        if matches!(outcome, JoinOutcome::FirstPlayer) {
            self.reply(ctx, ClientBoundPacket::change_host(true)).await;
        }

        ctx.server.events.post(events::PLAYER_JOIN_ROOM_SUCCESS, events::PlayerJoinRoomSuccessEvent {
            player: self.player.clone(),
            room: room.clone(),
        }).await;

        HandleOutcome::Switch(self.room_handler(room))
    }
}

impl PacketHandler for PlayHandler {
    fn player_ref(&self) -> Option<Arc<dyn Player>> {
        Some(self.player.clone())
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
                ServerBoundPacket::CreateRoom { room_id, .. } => {
                    self.on_create_room(ctx, room_id).await
                }
                ServerBoundPacket::JoinRoom { room_id, monitor, .. } => {
                    self.on_join_room(ctx, room_id, monitor).await
                }
                _ => {
                    // 大厅阶段收到房间操作包 → 踢（对应 Java PlayHandler 无对应 handle 方法）
                    debug!("Play stage: unexpected packet (user {})", self.player.id());
                    HandleOutcome::Close
                }
            }
        })
    }
}
