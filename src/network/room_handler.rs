//! RoomHandler（7.5 节）：房间内阶段。
//!
//! - 允许房间操作全集 + Ping。
//! - 重复 Authenticate/CreateRoom/JoinRoom → player.kick()。
//! - 业务错误 → 对应包的 failed PacketResult（i18n 按玩家语言）。

use crate::network::handler::{HandleOutcome, HandlerContext, PacketHandler};
use crate::network::play::{kick, send_broadcasts, PlayHandler};
use crate::packet::clientbound::ClientBoundPacket;
use crate::packet::serverbound::ServerBoundPacket;
use crate::packet::PacketResult;
use crate::player::Player;
use crate::room::Room;
use futures::future::BoxFuture;
use std::sync::Arc;

pub struct RoomHandler {
    player: Arc<Player>,
    room: Arc<Room>,
}

impl RoomHandler {
    pub fn new(player: Arc<Player>, room: Arc<Room>) -> Self {
        Self { player, room }
    }
}

impl PacketHandler for RoomHandler {
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
                ServerBoundPacket::Chat { message, .. } => {
                    match self.room.chat(self.player.id(), message) {
                        Ok(b) => {
                            ctx.send(ClientBoundPacket::chat_result(PacketResult::ok())).await;
                            send_broadcasts(b).await;
                            HandleOutcome::Ok
                        }
                        Err(e) => self.fail(ctx, e.0, ClientBoundPacket::chat_result).await,
                    }
                }
                ServerBoundPacket::LeaveRoom { .. } => {
                    ctx.send(ClientBoundPacket::leave_room_result(PacketResult::ok())).await;
                    let (_left, broadcasts, _destroyed) = self.room.leave(self.player.id());
                    send_broadcasts(broadcasts).await;
                    // 移除「玩家→房间」索引
                    ctx.server.leave_room_index(self.player.id());
                    // 回到 PlayHandler 阶段
                    HandleOutcome::Switch(Box::new(PlayHandler::new(self.player.clone())))
                }
                ServerBoundPacket::LockRoom { .. } => {
                    match self.room.toggle_lock(self.player.id()) {
                        Ok(b) => {
                            ctx.send(ClientBoundPacket::lock_room_result(PacketResult::ok())).await;
                            send_broadcasts(b).await;
                            HandleOutcome::Ok
                        }
                        Err(e) => self.fail(ctx, e.0, ClientBoundPacket::lock_room_result).await,
                    }
                }
                ServerBoundPacket::CycleRoom { .. } => {
                    match self.room.toggle_cycle(self.player.id()) {
                        Ok(b) => {
                            ctx.send(ClientBoundPacket::cycle_room_result(PacketResult::ok())).await;
                            send_broadcasts(b).await;
                            HandleOutcome::Ok
                        }
                        Err(e) => self.fail(ctx, e.0, ClientBoundPacket::cycle_room_result).await,
                    }
                }
                ServerBoundPacket::SelectChart { id, .. } => {
                    self.select_chart(ctx, id).await
                }
                ServerBoundPacket::RequestStart { .. } => {
                    match self.room.require_start(self.player.id()) {
                        Ok(b) => {
                            ctx.send(ClientBoundPacket::request_start_result(PacketResult::ok())).await;
                            send_broadcasts(b).await;
                            HandleOutcome::Ok
                        }
                        Err(e) => self.fail(ctx, e.0, ClientBoundPacket::request_start_result).await,
                    }
                }
                ServerBoundPacket::Ready { .. } => {
                    match self.room.ready(self.player.id()) {
                        Ok((b, _started)) => {
                            ctx.send(ClientBoundPacket::ready_result(PacketResult::ok())).await;
                            send_broadcasts(b).await;
                            HandleOutcome::Ok
                        }
                        Err(e) => self.fail(ctx, e.0, ClientBoundPacket::ready_result).await,
                    }
                }
                ServerBoundPacket::CancelReady { .. } => {
                    match self.room.cancel_ready(self.player.id()) {
                        Ok(b) => {
                            ctx.send(ClientBoundPacket::cancel_ready_result(PacketResult::ok())).await;
                            send_broadcasts(b).await;
                            HandleOutcome::Ok
                        }
                        Err(e) => self.fail(ctx, e.0, ClientBoundPacket::cancel_ready_result).await,
                    }
                }
                ServerBoundPacket::Touches { frames, .. } => {
                    let forwards = self.room.touch_send(self.player.id(), frames);
                    send_broadcasts(forwards).await;
                    HandleOutcome::Ok
                }
                ServerBoundPacket::Judges { judges, .. } => {
                    let forwards = self.room.judge_send(self.player.id(), judges);
                    send_broadcasts(forwards).await;
                    HandleOutcome::Ok
                }
                ServerBoundPacket::Played { record_id, .. } => {
                    self.played(ctx, record_id).await
                }
                ServerBoundPacket::Abort { .. } => {
                    match self.room.commit_abort(self.player.id()) {
                        Ok(outcome) => {
                            ctx.send(ClientBoundPacket::abort_result(PacketResult::ok())).await;
                            send_broadcasts(outcome.broadcasts).await;
                            HandleOutcome::Ok
                        }
                        Err(e) => self.fail(ctx, e.0, ClientBoundPacket::abort_result).await,
                    }
                }
                // 重复认证/建房/入房 → 离房 + 踢出（易错点 9；Java: kick 先离房再断开）
                ServerBoundPacket::Authenticate { .. }
                | ServerBoundPacket::CreateRoom { .. }
                | ServerBoundPacket::JoinRoom { .. } => {
                    let (_left, broadcasts, _destroyed) = self.room.leave(self.player.id());
                    send_broadcasts(broadcasts).await;
                    ctx.server.leave_room_index(self.player.id());
                    kick(&self.player).await;
                    HandleOutcome::Close
                }
            }
        })
    }

    fn room_ref(&self) -> Option<Arc<Room>> {
        Some(self.room.clone())
    }

    fn player_ref(&self) -> Option<Arc<Player>> {
        Some(self.player.clone())
    }
}

impl RoomHandler {
    async fn fail(
        &self,
        ctx: &HandlerContext,
        key: &str,
        mk: impl Fn(PacketResult<()>) -> ClientBoundPacket,
    ) -> HandleOutcome {
        let msg = ctx.server.i18n.message_for(&self.player, key);
        ctx.send(mk(PacketResult::failed(msg))).await;
        HandleOutcome::Failed
    }

    /// selectChart：validate（锁内）→ await fetch → commit（锁内）。
    async fn select_chart(&self, ctx: &HandlerContext, chart_id: i32) -> HandleOutcome {
        if let Err(e) = self.room.validate_select_chart(self.player.id()) {
            return self.fail(ctx, e.0, ClientBoundPacket::select_chart_result).await;
        }
        let chart = match ctx.server.phira.get_chart_info(chart_id).await {
            Ok(c) => c,
            Err(_) => return self.fail(ctx, "error.chart_not_found", ClientBoundPacket::select_chart_result).await,
        };
        match self.room.commit_select_chart(
            self.player.id(),
            chart.id,
            chart.name.clone(),
            self.player.name(),
        ) {
            Ok(b) => {
                ctx.send(ClientBoundPacket::select_chart_result(PacketResult::ok())).await;
                send_broadcasts(b).await;
                HandleOutcome::Ok
            }
            Err(e) => self.fail(ctx, e.0, ClientBoundPacket::select_chart_result).await,
        }
    }

    /// played：check（锁内）→ await fetch record → commit（锁内）。
    async fn played(&self, ctx: &HandlerContext, record_id: i32) -> HandleOutcome {
        use crate::room::PlayedCheck;
        let (chart_id, chart_name) = match self.room.check_played(self.player.id()) {
            Ok(PlayedCheck::AlreadyDone) => {
                ctx.send(ClientBoundPacket::played_result(PacketResult::ok())).await;
                return HandleOutcome::Ok;
            }
            Ok(PlayedCheck::CanPlay { chart_id, chart_name }) => (chart_id, chart_name),
            Err(e) => return self.fail(ctx, e.0, ClientBoundPacket::played_result).await,
        };

        let record = match ctx.server.phira.get_record_info(record_id).await {
            Ok(r) => r,
            Err(_) => return self.fail(ctx, "error.record_not_found", ClientBoundPacket::played_result).await,
        };

        match self.room.commit_played(
            self.player.id(),
            record.score,
            record.accuracy,
            record.full_combo,
        ) {
            Ok(outcome) => {
                ctx.send(ClientBoundPacket::played_result(PacketResult::ok())).await;
                send_broadcasts(outcome.broadcasts).await;
                // 录制（可选）
                if let Some(rec) = outcome.recording {
                    if let Some(dir) = ctx.server.record_dir.clone() {
                        if let Some(record_file) = crate::record::maybe_build_record(
                            record_id,
                            self.player.id(),
                            &self.player.name(),
                            rec.chart_id.or(chart_id),
                            rec.chart_name.or(chart_name),
                            rec.touch_frames,
                            rec.judge_events,
                        ) {
                            let dir = dir.clone();
                            tokio::task::spawn_blocking(move || {
                                if let Err(e) = record_file.write_to(
                                    std::path::Path::new(&dir),
                                    crate::record::CompressionType::Zstd,
                                ) {
                                    tracing::warn!("write record failed: {e}");
                                }
                            });
                        }
                    }
                }
                HandleOutcome::Ok
            }
            Err(e) => self.fail(ctx, e.0, ClientBoundPacket::played_result).await,
        }
    }
}
