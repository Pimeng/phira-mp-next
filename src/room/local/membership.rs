//! 成员加入、退出与空房销毁；在同一把房间锁内构造有序广播计划。

use super::LocalRoom;
use crate::packet::clientbound::ClientBoundPacket;
use crate::packet::data::FullUserProfile;
use crate::packet::message::Message;
use crate::player::Player;
use crate::room::behavior::{self, Broadcast};
use crate::room::{GameError, GameResult, JoinOutcome};
use std::sync::Arc;

impl LocalRoom {
    pub(super) fn join_member(
        &self,
        player: Arc<dyn Player>,
        is_monitor: bool,
    ) -> GameResult<(JoinOutcome, Broadcast)> {
        let mut inner = self.lock();
        if inner.destroyed {
            return Err(GameError("ERROR_ROOM_NOT_FOUND"));
        }
        if inner.contains_member(player.id()) {
            return Ok((JoinOutcome::AlreadyIn, vec![]));
        }
        if !is_monitor {
            if inner.players.len() >= inner.setting.max_player {
                return Err(GameError("ERROR_ROOM_FULL"));
            }
            if inner.setting.locked && !inner.players.is_empty() {
                return Err(GameError("ERROR_ROOM_LOCKED"));
            }
        }

        let first_player = inner.players.is_empty() && !is_monitor && inner.setting.host;
        let mut plan = Broadcast::new();
        if !first_player {
            let profile = FullUserProfile {
                user_id: player.id(),
                user_name: player.name(),
                monitor: is_monitor,
            };
            plan.extend(behavior::broadcast_all(
                &inner,
                ClientBoundPacket::on_join_room(profile),
            ));
            plan.extend(behavior::broadcast_all(
                &inner,
                ClientBoundPacket::message(Message::JoinRoom {
                    user: player.id(),
                    name: player.name(),
                }),
            ));
        }

        if is_monitor {
            inner.monitors.push(player.clone());
            inner.state.handle_join(player.id());
            Ok((JoinOutcome::Joined { is_monitor: true }, plan))
        } else {
            if first_player {
                inner.host = Some(player.clone());
            }
            inner.players.push(player.clone());
            inner.state.handle_join(player.id());
            Ok((
                if first_player {
                    JoinOutcome::FirstPlayer
                } else {
                    JoinOutcome::Joined { is_monitor: false }
                },
                plan,
            ))
        }
    }

    pub(super) fn leave_member(&self, player_id: i32) -> (bool, Broadcast, bool) {
        let mut inner = self.lock();
        let Some(name) = inner
            .players
            .iter()
            .chain(inner.monitors.iter())
            .find(|p| p.id() == player_id)
            .map(|p| p.name())
        else {
            return (false, vec![], false);
        };

        let was_host = inner.host.as_ref().map(|h| h.id()) == Some(player_id);
        inner.players.retain(|p| p.id() != player_id);
        inner.monitors.retain(|p| p.id() != player_id);

        // 空房间且 autoDestroy → 销毁
        if inner.players.is_empty() && inner.monitors.is_empty() && inner.setting.auto_destroy {
            inner.destroyed = true;
            if let Some(on_destroy) = self.on_destroy.lock().unwrap().take() {
                on_destroy();
            }
            return (true, vec![], true);
        }

        let mut plan = Broadcast::new();
        if was_host {
            plan.extend(behavior::transfer_host_plan(&mut inner));
        }
        plan.extend(behavior::broadcast_all(
            &inner,
            ClientBoundPacket::message(Message::LeaveRoom {
                user: player_id,
                name,
            }),
        ));
        inner.state.handle_leave(player_id);
        (true, plan, false)
    }
}
