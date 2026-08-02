//! LocalRoom（对应 Java `LocalRoom`）：默认房间实现。
//!
//! - 内嵌成员管理器（players/monitors/host 轮转）。
//! - 锁内决策（本结构 `Mutex<Inner>`），产出广播计划；锁外由 handler 发送。
//! - 「房间→玩家」单向强引用；玩家所在房间由 handler 反查（不经字段）。

use super::behavior::{self, Broadcast};
use super::state::RoomState;
use super::{
    CommitGameOutcome, GameResult, JoinOutcome, PlayedCheck, Room, RoomSetting, RoomSnapshot,
};
use crate::packet::clientbound::{ClientBoundPacket, JoinRoomData};
use crate::packet::data::{FullUserProfile, RoomInfo};
use crate::packet::message::Message;
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
        let g = self.lock();
        g.players.iter().any(|p| p.id() == user_id) || g.monitors.iter().any(|p| p.id() == user_id)
    }

    fn contains_monitor(&self, user_id: i32) -> bool {
        self.lock().monitors.iter().any(|p| p.id() == user_id)
    }

    fn is_monitor_user(&self, user_id: i32) -> bool {
        self.lock().monitors.iter().any(|p| p.id() == user_id)
    }

    fn is_host(&self, user_id: i32) -> bool {
        let g = self.lock();
        g.setting.host && g.host.as_ref().map(|h| h.id()) == Some(user_id)
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
        let g = self.lock();
        RoomInfo {
            room_id: self.id.clone(),
            state: g.state.to_protocol(),
            live: g.setting.live,
            locked: g.setting.locked,
            cycle: g.setting.cycle,
            is_host: g.setting.host && g.host.as_ref().map(|h| h.id()) == Some(viewer.id()),
            // WaitForReady 状态下对 viewer 恒 true（易错点 3）
            is_ready: matches!(g.state, RoomState::WaitForReady { .. }),
            users: member_profiles(&g),
        }
    }

    /// JoinRoom 快照（3.3 节 0x09 载荷）。
    fn join_room_data(&self) -> JoinRoomData {
        let g = self.lock();
        JoinRoomData {
            game_state: g.state.to_protocol(),
            users: member_profiles(&g),
            live: g.setting.live,
        }
    }

    fn snapshot(&self) -> RoomSnapshot {
        let g = self.lock();
        RoomSnapshot {
            room_id: self.id.clone(),
            state_kind_name: match &g.state {
                RoomState::SelectChart { .. } => "SelectChart",
                RoomState::WaitForReady { .. } => "WaitForReady",
                RoomState::Playing { .. } => "Playing",
            }
            .to_string(),
            locked: g.setting.locked,
            players: g.players.iter().map(|p| p.id()).collect(),
            monitors: g.monitors.iter().map(|p| p.id()).collect(),
            host: g.host.as_ref().map(|h| h.id()),
            chart_id: match &g.state {
                RoomState::SelectChart { chart_id, .. }
                | RoomState::WaitForReady { chart_id, .. }
                | RoomState::Playing { chart_id, .. } => *chart_id,
            },
            chart_name: match &g.state {
                RoomState::SelectChart { chart_name, .. }
                | RoomState::WaitForReady { chart_name, .. }
                | RoomState::Playing { chart_name, .. } => chart_name.clone(),
            },
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
        let mut g = self.lock();
        if g.destroyed {
            return Err(super::GameError("error.room_not_found"));
        }
        if g.players.iter().any(|p| p.id() == player.id())
            || g.monitors.iter().any(|p| p.id() == player.id())
        {
            return Ok((JoinOutcome::AlreadyIn, vec![]));
        }
        if !is_monitor {
            if g.players.len() >= g.setting.max_player {
                return Err(super::GameError("error.room_full"));
            }
            if g.setting.locked && !g.players.is_empty() {
                return Err(super::GameError("error.room_locked"));
            }
        }

        let first_player = g.players.is_empty() && !is_monitor && g.setting.host;
        let mut plan = Broadcast::new();
        if !first_player {
            let profile = FullUserProfile {
                user_id: player.id(),
                user_name: player.name(),
                monitor: is_monitor,
            };
            plan.extend(behavior::broadcast_all(&g, ClientBoundPacket::on_join_room(profile)));
            plan.extend(behavior::broadcast_all(
                &g,
                ClientBoundPacket::message(Message::JoinRoom {
                    user: player.id(),
                    name: player.name(),
                }),
            ));
        }

        if is_monitor {
            g.monitors.push(player.clone());
            g.state.handle_join(player.id());
            Ok((JoinOutcome::Joined { is_monitor: true }, plan))
        } else {
            if first_player {
                g.host = Some(player.clone());
            }
            g.players.push(player.clone());
            g.state.handle_join(player.id());
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

    fn leave(&self, player_id: i32) -> (bool, Broadcast, bool) {
        let mut g = self.lock();
        let Some(name) = g
            .players
            .iter()
            .chain(g.monitors.iter())
            .find(|p| p.id() == player_id)
            .map(|p| p.name())
        else {
            return (false, vec![], false);
        };

        let was_host = g.host.as_ref().map(|h| h.id()) == Some(player_id);
        g.players.retain(|p| p.id() != player_id);
        g.monitors.retain(|p| p.id() != player_id);

        // 空房间且 autoDestroy → 销毁
        if g.players.is_empty() && g.monitors.is_empty() && g.setting.auto_destroy {
            g.destroyed = true;
            if let Some(cb) = self.on_destroy.lock().unwrap().take() {
                cb();
            }
            return (true, vec![], true);
        }

        let mut plan = Broadcast::new();
        if was_host {
            plan.extend(behavior::transfer_host_plan(&mut g));
        }
        plan.extend(behavior::broadcast_all(
            &g,
            ClientBoundPacket::message(Message::LeaveRoom {
                user: player_id,
                name,
            }),
        ));
        g.state.handle_leave(player_id);
        (true, plan, false)
    }

    // ---------- 鉴权 ----------

    fn validate_host(&self, user_id: i32) -> GameResult<()> {
        if self.is_host(user_id) {
            Ok(())
        } else {
            Err(super::GameError("error.permission_denied"))
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
        let g = self.lock();
        match &g.state {
            RoomState::Playing { done, chart_id, chart_name, .. } => {
                if done.contains(&user_id) {
                    Ok(PlayedCheck::AlreadyDone)
                } else {
                    Ok(PlayedCheck::CanPlay {
                        chart_id: *chart_id,
                        chart_name: chart_name.clone(),
                    })
                }
            }
            _ => Err(super::GameError("error.invalid_state")),
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

    fn touch_send(
        &self,
        user_id: i32,
        frames: Vec<crate::packet::data::TouchFrame>,
    ) -> Broadcast {
        behavior::touch_send(&self.inner, user_id, frames)
    }

    fn judge_send(
        &self,
        user_id: i32,
        judges: Vec<crate::packet::data::JudgeEvent>,
    ) -> Broadcast {
        behavior::judge_send(&self.inner, user_id, judges)
    }

    fn cleanup_for_suspend(&self, user_id: i32) -> Broadcast {
        behavior::cleanup_for_suspend(&self.inner, user_id)
    }
}

/// 成员协议档案（players + monitors，monitor 标志位正确）。
fn member_profiles(g: &Inner) -> Vec<FullUserProfile> {
    g.players
        .iter()
        .map(|p| FullUserProfile {
            user_id: p.id(),
            user_name: p.name(),
            monitor: false,
        })
        .chain(g.monitors.iter().map(|p| FullUserProfile {
            user_id: p.id(),
            user_name: p.name(),
            monitor: true,
        }))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::network::connection::ConnectionHandle;
    use crate::packet::clientbound::ClientBoundPacket;
    use crate::packet::message::Message;
    use crate::phira::UserInfo;
    use crate::player::LocalPlayer;

    /// 构造测试用玩家（在线）。广播内容经 `broadcast plan` 的 SharedFrame 直接解码断言，
    /// 无需经 socket/连接通道，故连接的 rx 仅 forget 保活。
    fn player(id: i32, name: &str) -> Arc<dyn Player> {
        let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
        std::mem::forget(rx); // 保持 tx 配对端存活 → is_online() == true
        let conn = ConnectionHandle::new_for_test(tx);
        LocalPlayer::new(
            Arc::new(UserInfo {
                id,
                name: name.to_string(),
                ..Default::default()
            }),
            conn,
        )
    }

    fn make_room() -> Arc<LocalRoom> {
        LocalRoom::new("R1", RoomSetting::default(), || {})
    }

    /// 解码广播计划里发给指定玩家的第一个 message 包。
    fn message_for(
        plan: &crate::room::behavior::Broadcast,
        user_id: i32,
    ) -> Option<Message> {
        plan.iter()
            .filter(|(p, _)| p.id() == user_id)
            .filter_map(|(_, frame)| {
                // frame 是「带 VarInt 帧头的完整字节」；剥掉帧头后解码
                decode_frame_payload(frame)
            })
            .find_map(|p| match p {
                ClientBoundPacket::Message { message, .. } => Some(message),
                _ => None,
            })
    }

    fn packet_for(
        plan: &crate::room::behavior::Broadcast,
        user_id: i32,
    ) -> Vec<ClientBoundPacket> {
        plan.iter()
            .filter(|(p, _)| p.id() == user_id)
            .filter_map(|(_, frame)| decode_frame_payload(frame))
            .collect()
    }

    /// 剥掉 VarInt 帧头后解码 ClientBoundPacket。
    fn decode_frame_payload(frame: &crate::packet::clientbound::SharedFrame) -> Option<ClientBoundPacket> {
        // 跳过一个 VarInt（最多 5 字节）
        let bytes = frame.as_ref().as_ref();
        let mut i = 0;
        for _ in 0..5 {
            if i >= bytes.len() {
                return None;
            }
            let b = bytes[i];
            i += 1;
            if b & 0x80 == 0 {
                break;
            }
        }
        ClientBoundPacket::decode_frame(&bytes[i..]).ok()
    }

    // ---------------- 成员管理（对应 LocalRoomPlayerManagementTest） ----------------

    #[test]
    fn join_as_player_and_monitor_sets() {
        let room = make_room();
        let (outcome, _) = room.join(player(1, "A"), false).unwrap();
        assert!(matches!(outcome, JoinOutcome::FirstPlayer));
        assert!(room.contains_member(1));
        assert!(!room.is_monitor_user(1));

        room.join(player(2, "M"), true).unwrap();
        assert!(room.is_monitor_user(2));
        assert!(!room.is_host(2));
    }

    #[test]
    fn join_first_player_becomes_host() {
        let room = make_room();
        let p1 = player(1, "A");
        let (outcome, _b) = room.join(p1.clone(), false).unwrap();
        assert!(matches!(outcome, JoinOutcome::FirstPlayer));
        assert!(room.is_host(1));
        assert!(room.contains_member(1));
    }

    #[test]
    fn join_room_full_and_locked() {
        let setting = RoomSetting {
            max_player: 1,
            ..Default::default()
        };
        let room = LocalRoom::new("R2", setting, || {});
        room.join(player(1, "A"), false).unwrap();
        let err = room.join(player(2, "B"), false).unwrap_err();
        assert_eq!(err.0, "error.room_full");
        room.join(player(3, "M"), true).unwrap();
        assert!(room.is_monitor_user(3));
    }

    #[test]
    fn leave_removes_member_and_last_leaves_destroys() {
        use std::sync::atomic::{AtomicBool, Ordering};
        let destroyed = Arc::new(AtomicBool::new(false));
        let d = destroyed.clone();
        let setting = RoomSetting { auto_destroy: true, ..Default::default() };
        let room = LocalRoom::new("RD", setting, move || d.store(true, Ordering::SeqCst));
        room.join(player(1, "A"), false).unwrap();
        let (left, _plan, destroyed_flag) = room.leave(1);
        assert!(left);
        assert!(destroyed_flag);
        assert!(destroyed.load(Ordering::SeqCst));
        assert!(room.is_destroyed());
        assert!(!room.contains_member(1));
    }

    #[test]
    fn host_transfer_by_user_id_order() {
        let room = make_room();
        room.join(player(5, "E"), false).unwrap();
        room.join(player(2, "B"), false).unwrap();
        room.join(player(9, "I"), false).unwrap();
        room.leave(5);
        assert!(room.is_host(9));
        room.leave(9);
        assert!(room.is_host(2));
    }

    #[test]
    fn host_transfer_broadcasts_new_host() {
        let room = make_room();
        room.join(player(1, "A"), false).unwrap();
        room.join(player(2, "B"), false).unwrap();
        let (_left, plan, _d) = room.leave(1);
        // 玩家 2 收到 change_host(true)
        let pkts = packet_for(&plan, 2);
        assert!(pkts.iter().any(|p| matches!(p, ClientBoundPacket::ChangeHost { is_host: true, .. })));
        // NewHost 消息广播给非新房主成员（玩家 2 是新房主，不接收 NewHost）
    }

    // ---------------- 锁/循环（对应 LocalRoomOperationTest） ----------------

    #[test]
    fn lock_toggle_ignores_client_value() {
        let room = make_room();
        room.join(player(1, "A"), false).unwrap();
        room.toggle_lock(1).unwrap();
        let err = room.join(player(2, "B"), false).unwrap_err();
        assert_eq!(err.0, "error.room_locked");
        room.join(player(3, "M"), true).unwrap();
        room.toggle_lock(1).unwrap();
        room.join(player(2, "B"), false).unwrap();
        assert_eq!(room.toggle_lock(2).unwrap_err().0, "error.permission_denied");
    }

    #[test]
    fn lock_broadcasts_lock_message() {
        let room = make_room();
        room.join(player(1, "A"), false).unwrap();
        room.join(player(2, "B"), false).unwrap();
        let plan = room.toggle_lock(1).unwrap();
        match message_for(&plan, 2) {
            Some(Message::LockRoom { lock }) => assert!(lock),
            other => panic!("expected LockRoom(true), got {other:?}"),
        }
    }

    #[test]
    fn cycle_toggle_host_only() {
        let room = make_room();
        room.join(player(1, "A"), false).unwrap();
        room.join(player(2, "B"), false).unwrap();
        assert_eq!(room.toggle_cycle(2).unwrap_err().0, "error.permission_denied");
        let plan = room.toggle_cycle(1).unwrap();
        assert!(room.setting().cycle);
        match message_for(&plan, 2) {
            Some(Message::CycleRoom { cycle }) => assert!(cycle),
            other => panic!("expected CycleRoom(true), got {other:?}"),
        }
    }

    // ---------------- 聊天 ----------------

    #[test]
    fn chat_broadcasts_to_all() {
        let room = make_room();
        room.join(player(1, "A"), false).unwrap();
        room.join(player(2, "B"), false).unwrap();
        let plan = room.chat(1, "hello".into()).unwrap();
        match message_for(&plan, 2) {
            Some(Message::Chat { user, content }) => {
                assert_eq!(user, 1);
                assert_eq!(content, "hello");
            }
            other => panic!("expected Chat, got {other:?}"),
        }
    }

    #[test]
    fn chat_disabled_rejected() {
        let setting = RoomSetting { chat: false, ..Default::default() };
        let room = LocalRoom::new("RN", setting, || {});
        room.join(player(1, "A"), false).unwrap();
        assert_eq!(room.chat(1, "x".into()).unwrap_err().0, "error.chat_not_enabled");
    }

    // ---------------- 选谱 ----------------

    #[test]
    fn select_chart_host_only_and_state() {
        let room = make_room();
        room.join(player(1, "A"), false).unwrap();
        room.join(player(2, "B"), false).unwrap();
        assert_eq!(room.validate_select_chart(2).unwrap_err().0, "error.permission_denied");
        room.validate_select_chart(1).unwrap();
        let plan = room.commit_select_chart(1, 42, "Chart".into()).unwrap();
        match room.game_state_protocol() {
            GameState::SelectChart { chart_id } => assert_eq!(chart_id, Some(42)),
            _ => panic!(),
        }
        // 广播 SelectChart 消息 + ChangeState
        match message_for(&plan, 2) {
            Some(Message::SelectChart { user, id, name }) => {
                assert_eq!(user, 1);
                assert_eq!(id, 42);
                assert_eq!(name, "Chart");
            }
            other => panic!("expected SelectChart, got {other:?}"),
        }
    }

    #[test]
    fn select_chart_rejected_in_playing() {
        let room = make_room();
        room.join(player(1, "A"), false).unwrap();
        room.commit_select_chart(1, 1, "C".into()).unwrap();
        room.require_start(1).unwrap(); // 单人直接 Playing
        assert_eq!(room.validate_select_chart(1).unwrap_err().0, "error.invalid_state");
    }

    // ---------------- 开局/就绪（对应 RoomWaitForReadyStateTest） ----------------

    #[test]
    fn single_player_start_goes_playing_directly() {
        let room = make_room();
        room.join(player(1, "A"), false).unwrap();
        room.commit_select_chart(1, 42, "Chart".into()).unwrap();
        let plan = room.require_start(1).unwrap();
        assert!(matches!(room.game_state_protocol(), GameState::Playing));
        // 单人开局：统一体验也发 StartPlaying（最佳体验，非 Java 强制语义）
        let pkts = packet_for(&plan, 1);
        assert!(pkts.iter().any(|p| matches!(p, ClientBoundPacket::ChangeState { game_state: GameState::Playing, .. })));
        assert!(pkts.iter().any(|p| matches!(p, ClientBoundPacket::Message { message: Message::StartPlaying, .. })));
    }

    #[test]
    fn multi_player_ready_flow() {
        let room = make_room();
        room.join(player(1, "A"), false).unwrap();
        room.join(player(2, "B"), false).unwrap();
        room.commit_select_chart(1, 42, "Chart".into()).unwrap();
        room.require_start(1).unwrap();
        assert!(matches!(room.game_state_protocol(), GameState::WaitForReady));
        let (plan, started) = room.ready(2).unwrap();
        assert!(started);
        assert!(matches!(room.game_state_protocol(), GameState::Playing));
        assert_eq!(room.cancel_ready(1).unwrap_err().0, "error.invalid_state");
        // ready 广播 Ready{user:2}
        assert!(matches!(message_for(&plan, 1), Some(Message::Ready { user: 2 })));
    }

    #[test]
    fn ready_partial_does_not_start() {
        let room = make_room();
        room.join(player(1, "A"), false).unwrap();
        room.join(player(2, "B"), false).unwrap();
        room.join(player(3, "C"), false).unwrap();
        room.commit_select_chart(1, 1, "C".into()).unwrap();
        room.require_start(1).unwrap();
        let (_p, started) = room.ready(2).unwrap();
        assert!(!started);
        assert!(matches!(room.game_state_protocol(), GameState::WaitForReady));
    }

    #[test]
    fn cancel_ready_prevents_start() {
        let room = make_room();
        room.join(player(1, "A"), false).unwrap();
        room.join(player(2, "B"), false).unwrap();
        room.join(player(3, "C"), false).unwrap();
        room.commit_select_chart(1, 1, "C".into()).unwrap();
        room.require_start(1).unwrap(); // ready={1}
        room.ready(2).unwrap(); // ready={1,2}，online={1,2,3} 未开局
        let plan = room.cancel_ready(2).unwrap(); // ready={1}
        assert!(matches!(message_for(&plan, 1), Some(Message::CancelReady { user: 2 })));
        // ready 集合只有 {1}，online={1,2,3}，不应开局
        assert!(matches!(room.game_state_protocol(), GameState::WaitForReady));
        // 玩家 2 重新 ready 后仍缺玩家 3 → 不开局
        let (_p, started) = room.ready(2).unwrap();
        assert!(!started);
    }

    #[test]
    fn require_start_wrong_state_rejected() {
        let room = make_room();
        room.join(player(1, "A"), false).unwrap();
        room.commit_select_chart(1, 1, "C".into()).unwrap();
        room.require_start(1).unwrap(); // → Playing
        assert_eq!(room.require_start(1).unwrap_err().0, "error.invalid_state");
        assert_eq!(room.ready(1).unwrap_err().0, "error.invalid_state");
        assert_eq!(room.cancel_ready(1).unwrap_err().0, "error.invalid_state");
    }

    #[test]
    fn leave_in_wait_for_ready_removes_from_ready_set() {
        let room = make_room();
        room.join(player(1, "A"), false).unwrap();
        room.join(player(2, "B"), false).unwrap();
        room.join(player(3, "C"), false).unwrap();
        room.commit_select_chart(1, 1, "C".into()).unwrap();
        room.require_start(1).unwrap();
        room.ready(3).unwrap(); // ready={1,3}
        room.leave(3); // 3 离开 → ready={1}，但 online={1,2}
        let (_p, started) = room.ready(2).unwrap(); // ready={1,2}=online
        assert!(started);
    }

    // ---------------- 对局（对应 RoomPlayingStateTest） ----------------

    #[test]
    fn playing_played_idempotent_and_game_end_cycle() {
        let setting = RoomSetting {
            cycle: true,
            ..Default::default()
        };
        let room = LocalRoom::new("RC", setting, || {});
        room.join(player(1, "A"), false).unwrap();
        room.join(player(2, "B"), false).unwrap();
        room.commit_select_chart(1, 42, "Chart".into()).unwrap();
        room.require_start(1).unwrap();
        room.ready(2).unwrap();

        match room.check_played(1).unwrap() {
            PlayedCheck::CanPlay { chart_id, .. } => assert_eq!(chart_id, Some(42)),
            _ => panic!(),
        }
        let out1 = room.commit_played(1, 990000, 0.995, true).unwrap();
        assert!(!out1.game_ended);
        match room.check_played(1).unwrap() {
            PlayedCheck::AlreadyDone => {}
            _ => panic!("played should be idempotent"),
        }

        let out2 = room.commit_abort(2).unwrap();
        assert!(out2.game_ended);
        match room.game_state_protocol() {
            GameState::SelectChart { chart_id } => assert_eq!(chart_id, Some(42)),
            _ => panic!(),
        }
        assert!(room.is_host(2), "cycle should transfer host to next player");
    }

    #[test]
    fn played_broadcasts_played_message() {
        let room = make_room();
        room.join(player(1, "A"), false).unwrap();
        room.join(player(2, "B"), false).unwrap();
        room.commit_select_chart(1, 1, "C".into()).unwrap();
        room.require_start(1).unwrap();
        room.ready(2).unwrap();
        let out = room.commit_played(1, 1000000, 1.0, true).unwrap();
        match message_for(&out.broadcasts, 2) {
            Some(Message::Played { user, score, full_combo, .. }) => {
                assert_eq!(user, 1);
                assert_eq!(score, 1000000);
                assert!(full_combo);
            }
            other => panic!("expected Played, got {other:?}"),
        }
    }

    #[test]
    fn abort_broadcasts_and_ends_game() {
        let room = make_room();
        room.join(player(1, "A"), false).unwrap();
        room.join(player(2, "B"), false).unwrap();
        room.commit_select_chart(1, 1, "C".into()).unwrap();
        room.require_start(1).unwrap();
        room.ready(2).unwrap();
        let out1 = room.commit_abort(1).unwrap();
        assert!(matches!(message_for(&out1.broadcasts, 2), Some(Message::Abort { user: 1 })));
        assert!(!out1.game_ended); // 玩家 2 还没 done
        let out2 = room.commit_abort(2).unwrap();
        assert!(out2.game_ended);
    }

    #[test]
    fn played_collects_recording_data() {
        let room = make_room();
        room.join(player(1, "A"), false).unwrap();
        room.join(player(9, "M"), true).unwrap();
        room.commit_select_chart(1, 7, "S".into()).unwrap();
        room.require_start(1).unwrap();
        room.ready(9).unwrap();
        // 玩家 1 发送 touch/judge
        room.touch_send(1, vec![crate::packet::data::TouchFrame { time: 1.0, points: vec![] }]);
        room.judge_send(1, vec![crate::packet::data::JudgeEvent {
            time: 1.0, line_id: 0, note_id: 0,
            judgement: crate::packet::data::Judgement::Perfect,
        }]);
        let out = room.commit_played(1, 1, 1.0, true).unwrap();
        let rec = out.recording.expect("should collect recording");
        assert_eq!(rec.chart_id, Some(7));
        assert_eq!(rec.touch_frames.len(), 1);
        assert_eq!(rec.judge_events.len(), 1);
    }

    #[test]
    fn played_wrong_state_rejected() {
        let room = make_room();
        room.join(player(1, "A"), false).unwrap();
        // SelectChart 状态
        assert_eq!(room.commit_played(1, 1, 1.0, true).err().unwrap().0, "error.invalid_state");
        assert_eq!(room.commit_abort(1).err().unwrap().0, "error.invalid_state");
    }

    #[test]
    fn mid_game_join_marks_done() {
        let room = make_room();
        room.join(player(1, "A"), false).unwrap();
        room.commit_select_chart(1, 1, "C".into()).unwrap();
        room.require_start(1).unwrap(); // 单人 Playing
        // 对局中加入的玩家直接算 done
        room.join(player(2, "B"), false).unwrap();
        match room.check_played(2).unwrap() {
            PlayedCheck::AlreadyDone => {}
            _ => panic!("mid-game join should be AlreadyDone"),
        }
    }

    // ---------------- touch/judge 转发 ----------------

    #[test]
    fn touch_forward_to_monitor_any_state_collects_in_playing() {
        let room = make_room();
        room.join(player(1, "A"), false).unwrap();
        room.join(player(9, "M"), true).unwrap();
        // Java：转发给 monitor 是无条件的（任何状态都转）
        let f = room.touch_send(1, vec![crate::packet::data::TouchFrame { time: 0.1, points: vec![] }]);
        assert_eq!(f.len(), 1, "non-Playing also forwards to monitor");
        // 但收集仅在 Playing（非 Playing 不采集 → 录制无数据）
        room.commit_select_chart(1, 1, "C".into()).unwrap();
        room.require_start(1).unwrap(); // total=2（含 monitor）→ WaitForReady
        room.ready(9).unwrap(); // monitor ready → 全员就绪 → Playing
        assert!(matches!(room.game_state_protocol(), GameState::Playing));
        let f = room.touch_send(1, vec![crate::packet::data::TouchFrame {
            time: 0.5,
            points: vec![],
        }]);
        assert_eq!(f.len(), 1);
        // Playing 收集到了 → played 应有录制数据
        let out = room.commit_played(1, 1, 1.0, true).unwrap();
        assert!(out.recording.is_some());
    }

    #[test]
    fn judge_forward_to_monitor_only() {
        let room = make_room();
        room.join(player(1, "A"), false).unwrap();
        room.join(player(2, "B"), false).unwrap(); // 普通玩家不收
        room.join(player(9, "M"), true).unwrap();
        room.commit_select_chart(1, 1, "C".into()).unwrap();
        room.require_start(1).unwrap();
        room.ready(2).unwrap();
        room.ready(9).unwrap();
        let f = room.judge_send(1, vec![crate::packet::data::JudgeEvent {
            time: 1.0, line_id: 0, note_id: 0,
            judgement: crate::packet::data::Judgement::Good,
        }]);
        // 仅 1 个目标（monitor 9）
        assert_eq!(f.len(), 1);
        assert_eq!(f[0].0.id(), 9);
    }

    // ---------------- 挂起清理 ----------------

    #[test]
    fn suspend_cleanup_cancel_ready() {
        let room = make_room();
        room.join(player(1, "A"), false).unwrap();
        room.join(player(2, "B"), false).unwrap();
        room.require_start(1).unwrap();
        let _ = room.cleanup_for_suspend(2);
        let (_b, started) = room.ready(2).unwrap();
        assert!(started);
    }

    #[test]
    fn suspend_cleanup_abort_in_playing() {
        let room = make_room();
        room.join(player(1, "A"), false).unwrap();
        room.join(player(2, "B"), false).unwrap();
        room.commit_select_chart(1, 1, "C".into()).unwrap();
        room.require_start(1).unwrap();
        room.ready(2).unwrap();
        // 玩家 1 挂起 → cleanup 广播 Abort{1}，玩家 2 played 后对局结束
        let plan = room.cleanup_for_suspend(1);
        assert!(matches!(message_for(&plan, 2), Some(Message::Abort { user: 1 })));
        let out = room.commit_played(2, 1, 1.0, true).unwrap();
        assert!(out.game_ended);
    }
}
