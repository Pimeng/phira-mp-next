//! 选曲流程：扩展事件、权限/状态校验、数据获取和提交。

use super::RoomHandler;
use crate::events;
use crate::network::handler::{HandleOutcome, HandlerContext};
use crate::packet::PacketResult;
use crate::packet::clientbound::ClientBoundPacket;

impl RoomHandler {
    pub(super) async fn on_select_chart(
        &self,
        ctx: &HandlerContext,
        chart_id: i32,
    ) -> HandleOutcome {
        let lang = self.player.language();
        let i18n = &ctx.server.i18n;
        let response = ClientBoundPacket::select_chart_result;

        // RoomPreSelectChartEvent：可注入 chart 跳过远程拉谱，可取消
        let event = events::RoomPreSelectChartEvent {
            room: self.room.clone(),
            player: self.player.clone(),
            chart_id,
            chart: None,
            cancel_reason: None,
        };
        let event = ctx
            .server
            .events
            .post_mut(events::ROOM_PRE_SELECT_CHART, event)
            .await;
        if let Some(reason) = event.cancel_reason {
            ctx.send(response(PacketResult::failed(reason))).await;
            return HandleOutcome::Ok;
        }

        if let Err(error) = self.room.validate_select_chart(self.player.id()) {
            ctx.send(response(PacketResult::failed(
                i18n.message(lang.as_deref(), error.0),
            )))
            .await;
            return HandleOutcome::Ok;
        }

        // 谱面数据源：事件注入 → 全局 provider → 远程
        let chart = match event.chart {
            Some(chart) => Some(chart),
            None => match crate::player::chart_provider() {
                Some(provider) => provider(chart_id).await.ok(),
                None => ctx.server.phira.get_chart_info(chart_id).await.ok(),
            },
        };
        let Some(chart) = chart else {
            ctx.send(response(PacketResult::failed(
                i18n.message(lang.as_deref(), "ERROR_CHART_NOT_FOUND"),
            )))
            .await;
            return HandleOutcome::Ok;
        };

        let room = self.room.clone();
        let user_id = self.player.id();
        let name = chart.name.clone();
        let outcome = self
            .respond_to_action(
                ctx,
                move || room.commit_select_chart(user_id, chart_id, name),
                ClientBoundPacket::select_chart_result,
            )
            .await;

        ctx.server
            .events
            .post(
                events::ROOM_POST_SELECT_CHART,
                events::RoomPostSelectChartEvent {
                    room: self.room.clone(),
                    player: self.player.clone(),
                    chart_id,
                },
            )
            .await;
        self.notify_state_change(ctx).await;
        outcome
    }
}
