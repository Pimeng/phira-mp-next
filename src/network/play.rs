//! PlayHandler（7.5 节）：已认证未入房阶段。
//!
//! 允许：Ping（回 Pong）、CreateRoom、JoinRoom。
//! 其他包 → player.kick()（断开）。

use crate::network::handler::{HandleOutcome, HandlerContext, PacketHandler};
use crate::network::room_handler::RoomHandler;
use crate::packet::clientbound::ClientBoundPacket;
use crate::packet::serverbound::ServerBoundPacket;
use crate::packet::PacketResult;
use crate::player::Player;
use crate::room::JoinOutcome;
use futures::future::BoxFuture;
use std::sync::Arc;

pub struct PlayHandler {
    player: Arc<Player>,
}

impl PlayHandler {
    pub fn new(player: Arc<Player>) -> Self {
        Self { player }
    }
}

impl PacketHandler for PlayHandler {
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
                    self.create_room(ctx, room_id).await
                }
                ServerBoundPacket::JoinRoom { room_id, monitor, .. } => {
                    self.join_room(ctx, room_id, monitor).await
                }
                _ => {
                    // 不允许的包 → 踢出
                    kick(&self.player).await;
                    HandleOutcome::Close
                }
            }
        })
    }

    fn player_ref(&self) -> Option<Arc<Player>> {
        Some(self.player.clone())
    }
}

impl PlayHandler {
    async fn create_room(&self, ctx: &HandlerContext, room_id: String) -> HandleOutcome {
        let server = &ctx.server;
        match server.rooms.create_room(&room_id) {
            Ok(room) => {
                // 回复成功
                ctx.send(ClientBoundPacket::create_room_result(PacketResult::ok())).await;
                // 创建者加入房间（首个玩家自动成为房主）
                match room.join(self.player.clone(), false) {
                    Ok((_outcome, broadcasts)) => {
                        send_broadcasts(broadcasts).await;
                        // 维护「玩家→房间」索引（供认证响应/断线清理使用）
                        ctx.server.enter_room(self.player.id(), room.clone());
                        // 发送初始状态
                        ctx.send(ClientBoundPacket::change_state(room.game_state_protocol()))
                            .await;
                        ctx.send(ClientBoundPacket::change_host(true)).await;
                        HandleOutcome::Switch(Box::new(RoomHandler::new(self.player.clone(), room)))
                    }
                    Err(e) => {
                        ctx.send(ClientBoundPacket::create_room_result(PacketResult::failed(
                            server.i18n.message_for(&self.player, e.0),
                        )))
                        .await;
                        HandleOutcome::Failed
                    }
                }
            }
            Err(e) => {
                ctx.send(ClientBoundPacket::create_room_result(PacketResult::failed(
                    server.i18n.message_for(&self.player, e.0),
                )))
                .await;
                HandleOutcome::Failed
            }
        }
    }

    async fn join_room(&self, ctx: &HandlerContext, room_id: String, monitor: bool) -> HandleOutcome {
        let server = &ctx.server;
        let Some(room) = server.rooms.find_room(&room_id) else {
            ctx.send(ClientBoundPacket::join_room_result(PacketResult::failed(
                server.i18n.message_for(&self.player, "error.room_not_found"),
            )))
            .await;
            return HandleOutcome::Failed;
        };

        match room.join(self.player.clone(), monitor) {
            Ok((outcome, broadcasts)) => {
                // 回复 JoinRoom 快照
                let data = room.join_room_data();
                ctx.send(ClientBoundPacket::join_room_result(PacketResult::Success(data)))
                    .await;
                // 广播（不含自己）
                send_broadcasts(broadcasts).await;
                // 维护「玩家→房间」索引
                ctx.server.enter_room(self.player.id(), room.clone());
                // 房主身份
                let is_host = room.is_host(self.player.id());
                if matches!(outcome, JoinOutcome::FirstPlayer) || is_host {
                    ctx.send(ClientBoundPacket::change_host(true)).await;
                }
                HandleOutcome::Switch(Box::new(RoomHandler::new(self.player.clone(), room)))
            }
            Err(e) => {
                ctx.send(ClientBoundPacket::join_room_result(PacketResult::failed(
                    server.i18n.message_for(&self.player, e.0),
                )))
                .await;
                HandleOutcome::Failed
            }
        }
    }
}

pub(crate) async fn send_broadcasts(broadcasts: Vec<(Arc<Player>, ClientBoundPacket)>) {
    for (target, packet) in broadcasts {
        target.send(packet).await;
    }
}

pub(crate) async fn kick(player: &Arc<Player>) {
    player.kick();
    player.connection().close().await;
}
