//! RoomHandler（7.7 节）：房间内全部操作委托 `room` 操作层。
//!
//! 对应 Java `RoomHandler implements SuspendableRoomHolder, PlayerHolder`：
//! - 持玩家 + 房间 + fallback（离房切回）。
//! - `handle_with_error` 高阶函数 = Java `handleWithException`：
//!   「委托操作 → 成功/失败回复 + 锁外广播」收敛成一行声明式调用。

use super::handler::{HandleOutcome, HandlerContext, PacketHandler};
use crate::events;
use crate::log::log_debug;
use crate::packet::clientbound::ClientBoundPacket;
use crate::packet::serverbound::ServerBoundPacket;
use crate::packet::PacketResult;
use crate::player::Player;
use crate::room::{GameResult, PlayedCheck, Room};
use futures::future::BoxFuture;
use std::sync::Arc;

pub struct RoomHandler {
    player: Arc<dyn Player>,
    room: Arc<dyn Room>,
    fallback: Option<Box<dyn PacketHandler>>,
}

impl RoomHandler {
    pub fn new(player: Arc<dyn Player>, room: Arc<dyn Room>, fallback: Box<dyn PacketHandler>) -> Self {
        Self { player, room, fallback: Some(fallback) }
    }

    /// = Java `handleWithException(action, successPacket, failedPacket)`：
    /// 委托操作，成功发 success 包；失败发 failed(i18n) 包；产出广播计划则锁外发送。
    async fn handle_with_error<F>(
        &self,
        ctx: &HandlerContext,
        action: impl FnOnce() -> GameResult<crate::room::behavior::Broadcast>,
        success: F,
        failed: impl FnOnce(String) -> ClientBoundPacket,
    ) -> HandleOutcome
    where
        F: FnOnce() -> ClientBoundPacket,
    {
        match action() {
            Ok(plan) => {
                ctx.send(success()).await;
                crate::room::send_broadcasts(plan).await;
            }
            Err(e) => {
                let msg = ctx.server.i18n.message(self.player.language().as_deref(), e.0);
                ctx.send(failed(msg)).await;
            }
        }
        HandleOutcome::Ok
    }

    fn kick(&self, ctx: &HandlerContext) -> HandleOutcome {
        log_debug!(
            &ctx.server.i18n,
            "LOG_ROOM_UNEXPECTED_PACKET",
            ("id", self.player.id()),
        );
        let _ = ctx;
        HandleOutcome::Close
    }

    // ---- 各操作（每个对应 Java 一个 handle 方法） ----

    async fn on_chat(&self, ctx: &HandlerContext, content: String) -> HandleOutcome {
        // RoomChatEvent：可改写内容，可取消
        let ev = events::RoomChatEvent {
            room: self.room.clone(),
            player: self.player.clone(),
            message: content,
            cancel_reason: None,
        };
        let ev = ctx.server.events.post_mut(events::ROOM_CHAT, ev).await;
        if let Some(reason) = ev.cancel_reason {
            ctx.send(ClientBoundPacket::chat_result(PacketResult::failed(reason))).await;
            return HandleOutcome::Ok;
        }

        let room = self.room.clone();
        let uid = self.player.id();
        self.handle_with_error(
            ctx,
            move || room.chat(uid, ev.message),
            || ClientBoundPacket::chat_result(PacketResult::ok()),
            |msg| ClientBoundPacket::chat_result(PacketResult::failed(msg)),
        )
        .await
    }

    async fn on_leave_room(&mut self, ctx: &HandlerContext) -> HandleOutcome {
        let (left, plan, _destroyed) = self.room.leave(self.player.id());
        if !left {
            let msg = ctx.server.i18n.message(self.player.language().as_deref(), "ERROR_ROOM_NOT_FOUND");
            ctx.send(ClientBoundPacket::leave_room_result(PacketResult::failed(msg))).await;
            return HandleOutcome::Ok;
        }
        ctx.send(ClientBoundPacket::leave_room_result(PacketResult::ok())).await;
        crate::room::send_broadcasts(plan).await;
        ctx.server.events.post(events::PLAYER_LEAVE_ROOM, events::PlayerLeaveRoomEvent {
            player: self.player.clone(),
            room: self.room.clone(),
        }).await;
        // 离房切回 fallback（对应 Java setPacketHandler(fallback)）
        match self.fallback.take() {
            Some(fb) => HandleOutcome::Switch(fb),
            None => HandleOutcome::Close,
        }
    }

    async fn on_lock_room(&self, ctx: &HandlerContext) -> HandleOutcome {
        let locked = !self.room.setting().locked;
        ctx.server.events.post(events::ROOM_LOCK_CHANGE, events::RoomLockChangeEvent {
            room: self.room.clone(),
            player: self.player.clone(),
            locked,
        }).await;
        let room = self.room.clone();
        let uid = self.player.id();
        self.handle_with_error(
            ctx,
            move || room.toggle_lock(uid),
            || ClientBoundPacket::lock_room_result(PacketResult::ok()),
            |msg| ClientBoundPacket::lock_room_result(PacketResult::failed(msg)),
        )
        .await
    }

    async fn on_cycle_room(&self, ctx: &HandlerContext) -> HandleOutcome {
        let cycle = !self.room.setting().cycle;
        ctx.server.events.post(events::ROOM_CYCLE_CHANGE, events::RoomCycleChangeEvent {
            room: self.room.clone(),
            player: self.player.clone(),
            cycle,
        }).await;
        let room = self.room.clone();
        let uid = self.player.id();
        self.handle_with_error(
            ctx,
            move || room.toggle_cycle(uid),
            || ClientBoundPacket::cycle_room_result(PacketResult::ok()),
            |msg| ClientBoundPacket::cycle_room_result(PacketResult::failed(msg)),
        )
        .await
    }

    async fn on_select_chart(&self, ctx: &HandlerContext, chart_id: i32) -> HandleOutcome {
        let lang = self.player.language();
        let i18n = &ctx.server.i18n;
        let mk = |r: PacketResult<()>| ClientBoundPacket::select_chart_result(r);

        // RoomPreSelectChartEvent：可注入 chart 跳过远程拉谱，可取消
        let ev = events::RoomPreSelectChartEvent {
            room: self.room.clone(),
            player: self.player.clone(),
            chart_id,
            chart: None,
            cancel_reason: None,
        };
        let ev = ctx.server.events.post_mut(events::ROOM_PRE_SELECT_CHART, ev).await;
        if let Some(reason) = ev.cancel_reason {
            ctx.send(mk(PacketResult::failed(reason))).await;
            return HandleOutcome::Ok;
        }

        if let Err(e) = self.room.validate_select_chart(self.player.id()) {
            ctx.send(mk(PacketResult::failed(i18n.message(lang.as_deref(), e.0)))).await;
            return HandleOutcome::Ok;
        }

        // 谱面数据源：事件注入 → 全局 provider → 远程
        let chart = match ev.chart {
            Some(c) => Some(c),
            None => match crate::player::chart_provider() {
                Some(p) => p(chart_id).await.ok(),
                None => fetch_chart(ctx, chart_id).await,
            },
        };
        let Some(chart) = chart else {
            ctx.send(mk(PacketResult::failed(i18n.message(lang.as_deref(), "ERROR_CHART_NOT_FOUND")))).await;
            return HandleOutcome::Ok;
        };

        let room = self.room.clone();
        let uid = self.player.id();
        let name = chart.name.clone();
        let outcome = self.handle_with_error(
            ctx,
            move || room.commit_select_chart(uid, chart_id, name),
            || ClientBoundPacket::select_chart_result(PacketResult::ok()),
            |msg| ClientBoundPacket::select_chart_result(PacketResult::failed(msg)),
        )
        .await;

        ctx.server.events.post(events::ROOM_POST_SELECT_CHART, events::RoomPostSelectChartEvent {
            room: self.room.clone(),
            player: self.player.clone(),
            chart_id,
        }).await;
        ctx.server.events.post(events::ROOM_STATE_CHANGE, events::RoomStateChangeEvent {
            room: self.room.clone(),
            new_state: self.room.game_state_protocol(),
        }).await;
        outcome
    }

    async fn on_request_start(&self, ctx: &HandlerContext) -> HandleOutcome {
        // GameRequireStartEvent：可取消
        let ev = events::GameRequireStartEvent {
            room: self.room.clone(),
            player: self.player.clone(),
            cancel_reason: None,
        };
        let ev = ctx.server.events.post_mut(events::GAME_REQUIRE_START, ev).await;
        if let Some(reason) = ev.cancel_reason {
            ctx.send(ClientBoundPacket::request_start_result(PacketResult::failed(reason))).await;
            return HandleOutcome::Ok;
        }

        let room = self.room.clone();
        let uid = self.player.id();
        let outcome = self.handle_with_error(
            ctx,
            move || room.require_start(uid),
            || ClientBoundPacket::request_start_result(PacketResult::ok()),
            |msg| ClientBoundPacket::request_start_result(PacketResult::failed(msg)),
        )
        .await;

        ctx.server.events.post(events::GAME_START, events::GameStartEvent {
            room: self.room.clone(),
            player: self.player.clone(),
        }).await;
        ctx.server.events.post(events::ROOM_STATE_CHANGE, events::RoomStateChangeEvent {
            room: self.room.clone(),
            new_state: self.room.game_state_protocol(),
        }).await;
        if matches!(self.room.game_state_protocol(), crate::packet::state::GameState::Playing) {
            ctx.server.events.post(events::GAME_PLAYING_START, events::GamePlayingStartEvent {
                room: self.room.clone(),
            }).await;
        }
        outcome
    }

    async fn on_ready(&self, ctx: &HandlerContext) -> HandleOutcome {
        let room = self.room.clone();
        let uid = self.player.id();
        
        match room.ready(uid) {
            Ok((plan, started)) => {
                ctx.send(ClientBoundPacket::ready_result(PacketResult::ok())).await;
                crate::room::send_broadcasts(plan).await;
                ctx.server.events.post(events::PLAYER_READY, events::PlayerReadyEvent {
                    room: self.room.clone(),
                    player: self.player.clone(),
                }).await;
                if started {
                    ctx.server.events.post(events::ROOM_STATE_CHANGE, events::RoomStateChangeEvent {
                        room: self.room.clone(),
                        new_state: self.room.game_state_protocol(),
                    }).await;
                    ctx.server.events.post(events::GAME_PLAYING_START, events::GamePlayingStartEvent {
                        room: self.room.clone(),
                    }).await;
                }
                HandleOutcome::Ok
            }
            Err(e) => {
                let msg = ctx.server.i18n.message(self.player.language().as_deref(), e.0);
                ctx.send(ClientBoundPacket::ready_result(PacketResult::failed(msg))).await;
                HandleOutcome::Ok
            }
        }
    }

    async fn on_cancel_ready(&self, ctx: &HandlerContext) -> HandleOutcome {
        let room = self.room.clone();
        let uid = self.player.id();
        let outcome = self.handle_with_error(
            ctx,
            move || room.cancel_ready(uid),
            || ClientBoundPacket::cancel_ready_result(PacketResult::ok()),
            |msg| ClientBoundPacket::cancel_ready_result(PacketResult::failed(msg)),
        )
        .await;
        ctx.server.events.post(events::PLAYER_CANCEL_READY, events::PlayerCancelReadyEvent {
            room: self.room.clone(),
            player: self.player.clone(),
        }).await;
        outcome
    }

    async fn on_played(&self, ctx: &HandlerContext, record_id: i32) -> HandleOutcome {
        let lang = self.player.language();
        let i18n = &ctx.server.i18n;
        let mk = |r: PacketResult<()>| ClientBoundPacket::played_result(r);

        let check = match self.room.check_played(self.player.id()) {
            Ok(c) => c,
            Err(e) => {
                ctx.send(mk(PacketResult::failed(i18n.message(lang.as_deref(), e.0)))).await;
                return HandleOutcome::Ok;
            }
        };
        let PlayedCheck::CanPlay { .. } = check else {
            ctx.send(mk(PacketResult::ok())).await; // 幂等：已完成
            return HandleOutcome::Ok;
        };

        // 成绩数据源：provider → 远程
        let record = match crate::player::record_provider() {
            Some(p) => p(record_id).await.ok(),
            None => fetch_record(ctx, record_id).await,
        };
        let Some(record) = record else {
            ctx.send(mk(PacketResult::failed(i18n.message(lang.as_deref(), "ERROR_RECORD_NOT_FOUND")))).await;
            return HandleOutcome::Ok;
        };

        let room = self.room.clone();
        let uid = self.player.id();
        let user_name = self.player.name();
        let score = record.score;
        let accuracy = record.accuracy;
        let full_combo = record.full_combo;
        let outcome = self.handle_with_error(
            ctx,
            move || {
                let out = room.commit_played(uid, score, accuracy, full_combo)?;
                // 录制数据落盘（触发条件：触摸/判定非空，6.5/10 节）
                if let Some(rec) = out.recording
                    && let Some(dir) = crate::server::with_server_ctx(|c| c.record_dir.clone()).flatten()
                        && let Some(phira_rec) = crate::record::maybe_build_record(
                            record_id,
                            uid,
                            &user_name,
                            rec.chart_id,
                            rec.chart_name,
                            rec.touch_frames,
                            rec.judge_events,
                        ) {
                            let dir = std::path::PathBuf::from(dir);
                            tokio::task::spawn_blocking(move || {
                                if let Err(e) = phira_rec.write_to(&dir, crate::record::CompressionType::Zstd) {
                                    crate::log::log_warn_global!(
                                        "LOG_ROOM_RECORD_WRITE_FAILED",
                                        ("err", e.to_string()),
                                    );
                                }
                            });
                        }
                Ok(out.broadcasts)
            },
            || ClientBoundPacket::played_result(PacketResult::ok()),
            |msg| ClientBoundPacket::played_result(PacketResult::failed(msg)),
        )
        .await;

        ctx.server.events.post(events::PLAYER_PLAYED, events::PlayerPlayedEvent {
            room: self.room.clone(),
            player: self.player.clone(),
            score: record.score,
            accuracy: record.accuracy,
            full_combo: record.full_combo,
        }).await;
        if !matches!(self.room.game_state_protocol(), crate::packet::state::GameState::Playing) {
            ctx.server.events.post(events::GAME_END, events::GameEndEvent {
                room: self.room.clone(),
                records: self.room.game_records(),
            }).await;
            ctx.server.events.post(events::ROOM_STATE_CHANGE, events::RoomStateChangeEvent {
                room: self.room.clone(),
                new_state: self.room.game_state_protocol(),
            }).await;
        }
        outcome
    }

    async fn on_abort(&self, ctx: &HandlerContext) -> HandleOutcome {
        let room = self.room.clone();
        let uid = self.player.id();
        let outcome = self.handle_with_error(
            ctx,
            move || room.commit_abort(uid).map(|out| out.broadcasts),
            || ClientBoundPacket::abort_result(PacketResult::ok()),
            |msg| ClientBoundPacket::abort_result(PacketResult::failed(msg)),
        )
        .await;
        ctx.server.events.post(events::GAME_ABORT, events::GameAbortEvent {
            room: self.room.clone(),
            player: self.player.clone(),
        }).await;
        outcome
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
                    crate::room::send_broadcasts(self.room.touch_send(self.player.id(), frames)).await;
                    HandleOutcome::Ok
                }
                ServerBoundPacket::Judges { judges, .. } => {
                    crate::room::send_broadcasts(self.room.judge_send(self.player.id(), judges)).await;
                    HandleOutcome::Ok
                }
                // 房间内收到大厅/认证包 → 踢（对应 Java RoomHandler.kick()）
                ServerBoundPacket::Authenticate { .. }
                | ServerBoundPacket::CreateRoom { .. }
                | ServerBoundPacket::JoinRoom { .. } => self.kick(ctx),
            }
        })
    }
}

/// 远程拉谱（对应 Java `PhiraFetcher.GET_CHART_INFO`）。
async fn fetch_chart(ctx: &HandlerContext, chart_id: i32) -> Option<Arc<crate::phira::ChartInfo>> {
    ctx.server.phira.get_chart_info(chart_id).await.ok()
}

/// 远程拉成绩（对应 Java `PhiraFetcher.GET_RECORD_INFO`）。
async fn fetch_record(ctx: &HandlerContext, record_id: i32) -> Option<Arc<crate::phira::GameRecord>> {
    ctx.server.phira.get_record_info(record_id).await.ok()
}
