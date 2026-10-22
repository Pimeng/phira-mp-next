//! 成绩提交与录制文件落盘。

use super::RoomHandler;
use crate::events;
use crate::network::handler::{HandleOutcome, HandlerContext};
use crate::packet::PacketResult;
use crate::packet::clientbound::ClientBoundPacket;
use crate::room::{PlayedCheck, RecordingData};

impl RoomHandler {
    pub(super) async fn on_played(&self, ctx: &HandlerContext, record_id: i32) -> HandleOutcome {
        let lang = self.player.language();
        let i18n = &ctx.server.i18n;
        let response = ClientBoundPacket::played_result;

        let check = match self.room.check_played(self.player.id()) {
            Ok(check) => check,
            Err(error) => {
                ctx.send(response(PacketResult::failed(
                    i18n.message(lang.as_deref(), error.0),
                )))
                .await;
                return HandleOutcome::Ok;
            }
        };
        let PlayedCheck::CanPlay { .. } = check else {
            ctx.send(response(PacketResult::ok())).await; // 幂等：已完成
            return HandleOutcome::Ok;
        };

        // 成绩数据源：provider → 远程
        let record = match crate::player::record_provider() {
            Some(provider) => provider(record_id).await.ok(),
            None => ctx.server.phira.get_record_info(record_id).await.ok(),
        };
        let Some(record) = record else {
            ctx.send(response(PacketResult::failed(
                i18n.message(lang.as_deref(), "ERROR_RECORD_NOT_FOUND"),
            )))
            .await;
            return HandleOutcome::Ok;
        };

        let room = self.room.clone();
        let user_id = self.player.id();
        let user_name = self.player.name();
        let score = record.score;
        let accuracy = record.accuracy;
        let full_combo = record.full_combo;
        let outcome = self
            .respond_to_action(
                ctx,
                move || {
                    let out = room.commit_played(user_id, score, accuracy, full_combo)?;
                    persist_recording(record_id, user_id, &user_name, out.recording);
                    Ok(out.broadcasts)
                },
                ClientBoundPacket::played_result,
            )
            .await;

        ctx.server
            .events
            .post(
                events::PLAYER_PLAYED,
                events::PlayerPlayedEvent {
                    room: self.room.clone(),
                    player: self.player.clone(),
                    score: record.score,
                    accuracy: record.accuracy,
                    full_combo: record.full_combo,
                },
            )
            .await;
        if !matches!(
            self.room.game_state_protocol(),
            crate::packet::state::GameState::Playing
        ) {
            ctx.server
                .events
                .post(
                    events::GAME_END,
                    events::GameEndEvent {
                        room: self.room.clone(),
                        records: self.room.game_records(),
                    },
                )
                .await;
            self.notify_state_change(ctx).await;
        }
        outcome
    }
}

/// 有录制数据且配置了目录时，将压缩和写盘放到阻塞线程池。
/// 在提交成绩后、发送成功回复前调度，保持与房间广播的原有顺序。
fn persist_recording(
    record_id: i32,
    user_id: i32,
    user_name: &str,
    recording: Option<RecordingData>,
) {
    if let Some(recording) = recording
        && let Some(dir) = crate::server::with_server_ctx(|ctx| ctx.record_dir.clone()).flatten()
        && let Some(record) = crate::record::maybe_build_record(
            record_id,
            user_id,
            user_name,
            recording.chart_id,
            recording.chart_name,
            recording.touch_frames,
            recording.judge_events,
        )
    {
        let dir = std::path::PathBuf::from(dir);
        tokio::task::spawn_blocking(move || {
            if let Err(error) = record.write_to(&dir, crate::record::CompressionType::Zstd) {
                crate::log::log_warn_global!(
                    "LOG_ROOM_RECORD_WRITE_FAILED",
                    ("err", error.to_string()),
                );
            }
        });
    }
}
