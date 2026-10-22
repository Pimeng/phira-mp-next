//! LocalRoom（对应 Java `LocalRoom`）：默认房间实现。
//!
//! - 内嵌成员管理器（players/monitors/host 轮转）。
//! - 锁内决策（本结构 `Mutex<Inner>`），产出广播计划；锁外由 handler 发送。
//! - 「房间→玩家」单向强引用；玩家所在房间由 handler 反查（不经字段）。
//!
//! 本模块保留领域接口与只读快照；`membership` 管理入退房，
//! 对局和设置操作委托给 [`super::behavior`]。

mod membership;

#[cfg(test)]
mod tests;

use super::behavior::{self, Broadcast};
use super::state::RoomState;
use super::{
    CommitGameOutcome, GameResult, JoinOutcome, PlayedCheck, Room, RoomSetting, RoomSnapshot,
};
use crate::packet::clientbound::JoinRoomData;
use crate::packet::data::{FullUserProfile, RoomInfo};
use crate::packet::state::GameState;
use crate::player::Player;
use std::sync::{Arc, Mutex};

/// 锁内状态（锁内只做状态修改，不做网络 I/O / await）。
pub(crate) struct Inner {
    pub(crate) host: Option<Arc<dyn Player>>,
    pub(crate) players: Vec<Arc<dyn Player>>,
    pub(crate) monitors: Vec<Arc<dyn Player>>,
    pub(crate) state: RoomState,
    pub(crate) setting: RoomSetting,
    pub(crate) destroyed: bool,
    /// 下一轮房主（控制台 `nexthost`；循环模式对局结束时优先转移）。
    pub(crate) next_host: Option<i32>,
}

impl Inner {
    fn contains_member(&self, user_id: i32) -> bool {
        self.players
            .iter()
            .chain(self.monitors.iter())
            .any(|player| player.id() == user_id)
    }
}

pub struct LocalRoom {
    id: String,
    pub(crate) inner: Mutex<Inner>,
    /// 销毁回调（从全局注册表移除）。
    on_destroy: Mutex<Option<Box<dyn Fn() + Send + Sync>>>,
}

impl LocalRoom {
    pub fn new(
        id: impl Into<String>,
        setting: RoomSetting,
        on_destroy: impl Fn() + Send + Sync + 'static,
    ) -> Arc<Self> {
        Arc::new(Self {
            id: id.into(),
            inner: Mutex::new(Inner {
                host: None,
                players: Vec::new(),
                monitors: Vec::new(),
                state: RoomState::SelectChart {
                    chart_id: None,
                    chart_name: None,
                },
                setting,
                destroyed: false,
                next_host: None,
            }),
            on_destroy: Mutex::new(Some(Box::new(on_destroy))),
        })
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Inner> {
        self.inner.lock().unwrap()
    }
}

impl Room for LocalRoom {
    fn id(&self) -> &str {
        &self.id
    }

    // ---------- 查询 ----------

    fn contains_member(&self, user_id: i32) -> bool {
        self.lock().contains_member(user_id)
    }

    fn contains_monitor(&self, user_id: i32) -> bool {
        self.lock().monitors.iter().any(|p| p.id() == user_id)
    }

    fn is_monitor_user(&self, user_id: i32) -> bool {
        self.contains_monitor(user_id)
    }

    fn is_host(&self, user_id: i32) -> bool {
        let inner = self.lock();
        inner.setting.host && inner.host.as_ref().map(|h| h.id()) == Some(user_id)
    }

    fn is_destroyed(&self) -> bool {
        self.lock().destroyed
    }

    fn game_state_protocol(&self) -> GameState {
        self.lock().state.to_protocol()
    }

    fn setting(&self) -> RoomSetting {
        self.lock().setting.clone()
    }

    /// viewer 视角 RoomInfo 快照（3.5 / 7.2 节）。
    fn build_room_info(&self, viewer: &dyn Player) -> RoomInfo {
        let inner = self.lock();
        RoomInfo {
            room_id: self.id.clone(),
            state: inner.state.to_protocol(),
            live: inner.setting.live,
            locked: inner.setting.locked,
            cycle: inner.setting.cycle,
            is_host: inner.setting.host && inner.host.as_ref().map(|h| h.id()) == Some(viewer.id()),
            // WaitForReady 状态下对 viewer 恒 true（易错点 3）
            is_ready: matches!(inner.state, RoomState::WaitForReady { .. }),
            users: member_profiles(&inner),
        }
    }

    /// JoinRoom 快照（3.3 节 0x09 载荷）。
    fn join_room_data(&self) -> JoinRoomData {
        let inner = self.lock();
        JoinRoomData {
            game_state: inner.state.to_protocol(),
            users: member_profiles(&inner),
            live: inner.setting.live,
        }
    }

    fn snapshot(&self) -> RoomSnapshot {
        let inner = self.lock();
        let (state_kind, chart_id, chart_name) = match &inner.state {
            RoomState::SelectChart {
                chart_id,
                chart_name,
            } => ("SelectChart", chart_id, chart_name),
            RoomState::WaitForReady {
                chart_id,
                chart_name,
                ..
            } => ("WaitForReady", chart_id, chart_name),
            RoomState::Playing {
                chart_id,
                chart_name,
                ..
            } => ("Playing", chart_id, chart_name),
        };
        RoomSnapshot {
            room_id: self.id.clone(),
            state_kind_name: state_kind.to_string(),
            locked: inner.setting.locked,
            players: inner.players.iter().map(|p| p.id()).collect(),
            monitors: inner.monitors.iter().map(|p| p.id()).collect(),
            host: inner.host.as_ref().map(|h| h.id()),
            chart_id: *chart_id,
            chart_name: chart_name.clone(),
        }
    }

    fn game_records(&self) -> Vec<(i32, i32, f32, bool)> {
        self.lock().state.records().to_vec()
    }

    // ---------- 成员管理 ----------

    fn join(
        &self,
        player: Arc<dyn Player>,
        is_monitor: bool,
    ) -> GameResult<(JoinOutcome, Broadcast)> {
        self.join_member(player, is_monitor)
    }

    fn leave(&self, player_id: i32) -> (bool, Broadcast, bool) {
        self.leave_member(player_id)
    }

    // ---------- 鉴权 ----------

    fn validate_host(&self, user_id: i32) -> GameResult<()> {
        if self.is_host(user_id) {
            Ok(())
        } else {
            Err(super::GameError("ERROR_PERMISSION_DENIED"))
        }
    }

    // ---------- 操作（委托 behavior 层） ----------

    fn toggle_lock(&self, user_id: i32) -> GameResult<Broadcast> {
        self.validate_host(user_id)?;
        behavior::toggle_lock(&self.inner)
    }

    fn toggle_cycle(&self, user_id: i32) -> GameResult<Broadcast> {
        self.validate_host(user_id)?;
        behavior::toggle_cycle(&self.inner)
    }

    fn chat(&self, user_id: i32, content: String) -> GameResult<Broadcast> {
        behavior::chat(&self.inner, user_id, content)
    }

    fn validate_select_chart(&self, user_id: i32) -> GameResult<()> {
        self.validate_host(user_id)?;
        behavior::validate_select_chart(&self.inner)
    }

    fn commit_select_chart(
        &self,
        user_id: i32,
        chart_id: i32,
        chart_name: String,
    ) -> GameResult<Broadcast> {
        self.validate_host(user_id)?;
        behavior::commit_select_chart(&self.inner, user_id, chart_id, chart_name)
    }

    fn require_start(&self, user_id: i32) -> GameResult<Broadcast> {
        self.validate_host(user_id)?;
        behavior::require_start(&self.inner, user_id)
    }

    fn ready(&self, user_id: i32) -> GameResult<(Broadcast, bool)> {
        behavior::ready(&self.inner, user_id)
    }

    fn cancel_ready(&self, user_id: i32) -> GameResult<Broadcast> {
        behavior::cancel_ready(&self.inner, user_id)
    }

    fn check_played(&self, user_id: i32) -> GameResult<PlayedCheck> {
        let inner = self.lock();
        match &inner.state {
            RoomState::Playing {
                done,
                chart_id,
                chart_name,
                ..
            } => {
                if done.contains(&user_id) {
                    Ok(PlayedCheck::AlreadyDone)
                } else {
                    Ok(PlayedCheck::CanPlay {
                        chart_id: *chart_id,
                        chart_name: chart_name.clone(),
                    })
                }
            }
            _ => Err(super::GameError("ERROR_INVALID_STATE")),
        }
    }

    fn commit_played(
        &self,
        user_id: i32,
        score: i32,
        accuracy: f32,
        full_combo: bool,
    ) -> GameResult<CommitGameOutcome> {
        behavior::commit_played(&self.inner, user_id, score, accuracy, full_combo)
    }

    fn commit_abort(&self, user_id: i32) -> GameResult<CommitGameOutcome> {
        behavior::commit_abort(&self.inner, user_id)
    }

    fn touch_send(&self, user_id: i32, frames: Vec<crate::packet::data::TouchFrame>) -> Broadcast {
        behavior::touch_send(&self.inner, user_id, frames)
    }

    fn judge_send(&self, user_id: i32, judges: Vec<crate::packet::data::JudgeEvent>) -> Broadcast {
        behavior::judge_send(&self.inner, user_id, judges)
    }

    fn cleanup_for_suspend(&self, user_id: i32) -> Broadcast {
        behavior::cleanup_for_suspend(&self.inner, user_id)
    }

    // ---------- 管理员操作（控制台命令；绕过玩家鉴权） ----------

    fn admin_set_max_player(&self, count: usize) -> GameResult<()> {
        behavior::set_max_player(&self.inner, count)
    }

    fn admin_set_locked(&self, locked: bool) -> GameResult<Broadcast> {
        behavior::set_locked(&self.inner, locked)
    }

    fn admin_set_cycle(&self, cycle: bool) -> GameResult<Broadcast> {
        behavior::set_cycle(&self.inner, cycle)
    }

    fn admin_set_next_host(&self, user_id: i32) -> GameResult<()> {
        behavior::set_next_host(&self.inner, user_id)
    }

    fn admin_transfer_host(&self, user_id: i32) -> GameResult<Broadcast> {
        behavior::transfer_host_to(&self.inner, user_id)
    }

    fn admin_chat(&self, content: String) -> GameResult<Broadcast> {
        behavior::admin_chat(&self.inner, content)
    }
}

/// 成员协议档案（players + monitors，monitor 标志位正确）。
fn member_profiles(inner: &Inner) -> Vec<FullUserProfile> {
    inner
        .players
        .iter()
        .map(|p| FullUserProfile {
            user_id: p.id(),
            user_name: p.name(),
            monitor: false,
        })
        .chain(inner.monitors.iter().map(|p| FullUserProfile {
            user_id: p.id(),
            user_name: p.name(),
            monitor: true,
        }))
        .collect()
}
