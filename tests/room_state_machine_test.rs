//! 房间状态机常规 + 极端测试（纯逻辑，无网络）。
//!
//! 使用 `TestPlayer`（无连接内存玩家）+ `LocalRoom`，直接驱动
//! Room trait 的 join/leave/操作，对广播计划里的预编码共享帧解码断言。
//!
//! 覆盖：成员管理 / 权限 / 锁定与循环 / 选曲 / 开局（单人/多人）/
//! ready 与 cancelReady / played 与 abort 幂等 / 对局结束判定（含全员掉线）/
//! touch/judge 收集与 monitor 转发 / 录制数据提取 / 房主转移 / 挂起清理 /
//! 快照与会话信息 / 自动销毁。

mod common;

use common::TestPlayer;
use phira_mp::packet::clientbound::ClientBoundPacket;
use phira_mp::packet::data::{CompactPos, JudgeEvent, Judgement, TouchFrame, TouchPoint};
use phira_mp::packet::message::Message;
use phira_mp::packet::state::GameState;
use phira_mp::player::Player;
use phira_mp::room::behavior::Broadcast;
use phira_mp::room::{JoinOutcome, LocalRoom, Room, RoomSetting};
use std::sync::Arc;

fn make_room() -> Arc<LocalRoom> {
    LocalRoom::new("R1", RoomSetting::default(), || {})
}

fn player(id: i32, name: &str) -> Arc<TestPlayer> {
    TestPlayer::new(id, name)
}

fn message_for(plan: &Broadcast, user_id: i32) -> Option<Message> {
    plan.iter()
        .filter(|(p, _)| p.id() == user_id)
        .filter_map(|(_, frame)| common::decode_frame_payload(frame))
        .find_map(|p| match p {
            ClientBoundPacket::Message { message, .. } => Some(message),
            _ => None,
        })
}

fn packets_for(plan: &Broadcast, user_id: i32) -> Vec<ClientBoundPacket> {
    plan.iter()
        .filter(|(p, _)| p.id() == user_id)
        .filter_map(|(_, frame)| common::decode_frame_payload(frame))
        .collect()
}

/// 广播计划中出现的（去重后的）ChangeState 序列。
/// 注意：同一份 ChangeState 帧会发给每个成员，plan 中会出现重复，按值去重。
fn state_changes(plan: &Broadcast) -> Vec<GameState> {
    let mut out: Vec<GameState> = Vec::new();
    for (_, frame) in plan {
        if let Some(ClientBoundPacket::ChangeState { game_state, .. }) =
            common::decode_frame_payload(frame)
            && !out.contains(&game_state)
        {
            out.push(game_state);
        }
    }
    out
}

/// 单人房间推进到 Playing。
fn setup_solo_playing(room: &Arc<LocalRoom>, p: &Arc<TestPlayer>) {
    room.join(p.clone(), false).unwrap();
    room.commit_select_chart(p.id(), 42, "C42".into()).unwrap();
    room.require_start(p.id()).unwrap();
    assert!(matches!(room.game_state_protocol(), GameState::Playing));
}

/// 双人房间推进到 Playing。
fn setup_duo_playing(room: &Arc<LocalRoom>, p1: &Arc<TestPlayer>, p2: &Arc<TestPlayer>) {
    room.join(p1.clone(), false).unwrap();
    room.join(p2.clone(), false).unwrap();
    room.commit_select_chart(p1.id(), 42, "C42".into()).unwrap();
    room.require_start(p1.id()).unwrap(); // → WaitForReady，p1 自动 ready
    assert!(matches!(
        room.game_state_protocol(),
        GameState::WaitForReady
    ));
    room.ready(p2.id()).unwrap(); // 全员 ready → Playing
    assert!(matches!(room.game_state_protocol(), GameState::Playing));
}

fn assert_err<T>(r: Result<T, phira_mp::room::GameError>, expected: &str) {
    match r {
        Err(e) => assert_eq!(e.0, expected, "expected {expected}"),
        Ok(_) => panic!("expected error {expected}, got ok"),
    }
}

fn assert_commit_err(
    r: Result<phira_mp::room::CommitGameOutcome, phira_mp::room::GameError>,
    expected: &str,
) {
    match r {
        Err(e) => assert_eq!(e.0, expected, "expected {expected}"),
        Ok(_) => panic!("expected error {expected}, got ok"),
    }
}

// ============================== 成员管理 ==============================

#[test]
fn join_first_player_becomes_host() {
    let room = make_room();
    let p1 = player(1, "A");
    let (outcome, plan) = room.join(p1.clone(), false).unwrap();
    assert!(matches!(outcome, JoinOutcome::FirstPlayer));
    assert!(room.is_host(1));
    assert!(room.contains_member(1));
    assert!(!room.contains_monitor(1));
    assert!(plan.is_empty(), "first player join broadcasts nothing");
}

#[test]
fn join_second_player_broadcasts() {
    let room = make_room();
    let p1 = player(1, "A");
    let p2 = player(2, "B");
    room.join(p1.clone(), false).unwrap();
    let (outcome, plan) = room.join(p2.clone(), false).unwrap();
    assert!(matches!(outcome, JoinOutcome::Joined { is_monitor: false }));
    // p1 收到 OnJoinRoom + JoinRoom message
    let pkts = packets_for(&plan, 1);
    assert!(pkts.iter().any(|p| matches!(p,
        ClientBoundPacket::OnJoinRoom { user_profile, .. } if user_profile.user_id == 2)));
    assert!(matches!(
        message_for(&plan, 1),
        Some(Message::JoinRoom { user: 2, .. })
    ));
}

#[test]
fn join_monitor() {
    let room = make_room();
    let p1 = player(1, "A");
    let m = player(99, "Monitor");
    room.join(p1.clone(), false).unwrap();
    let (outcome, _) = room.join(m.clone(), true).unwrap();
    assert!(matches!(outcome, JoinOutcome::Joined { is_monitor: true }));
    assert!(room.contains_monitor(99));
    assert!(!room.is_host(99));
    assert!(room.contains_member(99));
}

#[test]
fn join_duplicate_returns_already_in() {
    let room = make_room();
    let p1 = player(1, "A");
    room.join(p1.clone(), false).unwrap();
    let (outcome, plan) = room.join(p1.clone(), false).unwrap();
    assert!(matches!(outcome, JoinOutcome::AlreadyIn));
    assert!(plan.is_empty());
    // monitor 与 player 互斥：同一人不能同时是 monitor
    let (outcome, _) = room.join(p1.clone(), true).unwrap();
    assert!(matches!(outcome, JoinOutcome::AlreadyIn));
}

#[test]
fn join_room_full() {
    let room = LocalRoom::new(
        "F",
        RoomSetting {
            max_player: 1,
            ..Default::default()
        },
        || {},
    );
    room.join(player(1, "A"), false).unwrap();
    assert_err(room.join(player(2, "B"), false), "ERROR_ROOM_FULL");
    // monitor 不受 max_player 限制
    room.join(player(3, "M"), true).unwrap();
    assert!(room.contains_monitor(3));
}

#[test]
fn join_locked_room() {
    let room = make_room();
    let p1 = player(1, "A");
    room.join(p1.clone(), false).unwrap();
    room.toggle_lock(1).unwrap();
    // 非房主被拒
    assert_err(room.join(player(2, "B"), false), "ERROR_ROOM_LOCKED");
    // monitor 可进
    room.join(player(3, "M"), true).unwrap();
    // 空房间可进（先离开后加锁，再进）
    let room2 = LocalRoom::new(
        "L",
        RoomSetting {
            locked: true,
            ..Default::default()
        },
        || {},
    );
    room2.join(player(10, "X"), false).unwrap();
    assert!(room2.contains_member(10));
}

#[test]
fn join_destroyed_room() {
    let room = LocalRoom::new(
        "D",
        RoomSetting {
            auto_destroy: true,
            ..Default::default()
        },
        || {},
    );
    room.join(player(1, "A"), false).unwrap();
    room.leave(1);
    assert!(room.is_destroyed());
    assert_err(room.join(player(2, "B"), false), "ERROR_ROOM_NOT_FOUND");
}

#[test]
fn leave_non_member_returns_false() {
    let room = make_room();
    let (left, plan, destroyed) = room.leave(999);
    assert!(!left);
    assert!(plan.is_empty());
    assert!(!destroyed);
}

#[test]
fn leave_last_player_destroys_and_fires_callback() {
    use std::sync::atomic::{AtomicBool, Ordering};
    let destroyed = Arc::new(AtomicBool::new(false));
    let flag = destroyed.clone();
    let room = LocalRoom::new("AD", RoomSetting::default(), move || {
        flag.store(true, Ordering::SeqCst);
    });
    room.join(player(1, "A"), false).unwrap();
    let (left, plan, room_destroyed) = room.leave(1);
    assert!(left);
    assert!(room_destroyed);
    assert!(destroyed.load(Ordering::SeqCst));
    assert!(room.is_destroyed());
    assert!(!room.contains_member(1));
    // 广播在销毁时为空（房间已空，无人可收）
    assert!(plan.is_empty());
}

#[test]
fn auto_destroy_false_keeps_room_empty() {
    let room = LocalRoom::new(
        "K",
        RoomSetting {
            auto_destroy: false,
            ..Default::default()
        },
        || {},
    );
    room.join(player(1, "A"), false).unwrap();
    let (left, plan, destroyed) = room.leave(1);
    assert!(left);
    assert!(!destroyed);
    assert!(!room.is_destroyed());
    // 无剩余成员可广播（广播给全体成员的列表为空）
    assert!(plan.is_empty());
    // 空房间允许重新加入成为房主
    room.join(player(2, "B"), false).unwrap();
    assert!(room.is_host(2));
}

// ============================== 权限与锁定/循环 ==============================

#[test]
fn non_host_operations_denied() {
    let room = make_room();
    let p1 = player(1, "A");
    let p2 = player(2, "B");
    room.join(p1.clone(), false).unwrap();
    room.join(p2.clone(), false).unwrap();
    assert_err(room.toggle_lock(2), "ERROR_PERMISSION_DENIED");
    assert_err(room.toggle_cycle(2), "ERROR_PERMISSION_DENIED");
    assert_err(room.validate_select_chart(2), "ERROR_PERMISSION_DENIED");
    assert_err(
        room.commit_select_chart(2, 1, "C".into()),
        "ERROR_PERMISSION_DENIED",
    );
    assert_err(room.require_start(2), "ERROR_PERMISSION_DENIED");
}

#[test]
fn lock_toggle_ignores_client_value() {
    let room = make_room();
    let p1 = player(1, "A");
    room.join(p1.clone(), false).unwrap();
    room.toggle_lock(1).unwrap();
    assert!(room.setting().locked);
    room.toggle_lock(1).unwrap();
    assert!(!room.setting().locked);
}

#[test]
fn lock_broadcasts_message() {
    let room = make_room();
    let p1 = player(1, "A");
    let p2 = player(2, "B");
    room.join(p1.clone(), false).unwrap();
    room.join(p2.clone(), false).unwrap();
    let plan = room.toggle_lock(1).unwrap();
    assert!(matches!(
        message_for(&plan, 2),
        Some(Message::LockRoom { lock: true })
    ));
    assert!(matches!(
        message_for(&plan, 1),
        Some(Message::LockRoom { lock: true })
    ));
}

#[test]
fn cycle_toggle_and_broadcast() {
    let room = make_room();
    let p1 = player(1, "A");
    let p2 = player(2, "B");
    room.join(p1.clone(), false).unwrap();
    room.join(p2.clone(), false).unwrap();
    let plan = room.toggle_cycle(1).unwrap();
    assert!(room.setting().cycle);
    assert!(matches!(
        message_for(&plan, 2),
        Some(Message::CycleRoom { cycle: true })
    ));
    // 再次切换回 false
    let plan = room.toggle_cycle(1).unwrap();
    assert!(!room.setting().cycle);
    assert!(matches!(
        message_for(&plan, 2),
        Some(Message::CycleRoom { cycle: false })
    ));
}

// ============================== 聊天 ==============================

#[test]
fn chat_broadcast_and_disabled() {
    let room = make_room();
    let p1 = player(1, "A");
    let p2 = player(2, "B");
    room.join(p1.clone(), false).unwrap();
    room.join(p2.clone(), false).unwrap();
    let plan = room.chat(1, "hello 世界".into()).unwrap();
    assert!(
        matches!(message_for(&plan, 2), Some(Message::Chat { user: 1, content }) if content == "hello 世界")
    );
    // 房主也被广播
    assert!(matches!(
        message_for(&plan, 1),
        Some(Message::Chat { user: 1, .. })
    ));

    let room2 = LocalRoom::new(
        "NC",
        RoomSetting {
            chat: false,
            ..Default::default()
        },
        || {},
    );
    room2.join(player(1, "A"), false).unwrap();
    assert_err(room2.chat(1, "hi".into()), "ERROR_CHAT_NOT_ENABLED");
}

// ============================== 选曲 ==============================

#[test]
fn select_chart_commits_and_broadcasts() {
    let room = make_room();
    let p1 = player(1, "A");
    let p2 = player(2, "B");
    room.join(p1.clone(), false).unwrap();
    room.join(p2.clone(), false).unwrap();
    let plan = room.commit_select_chart(1, 42, "Chart42".into()).unwrap();
    assert!(matches!(message_for(&plan, 2),
        Some(Message::SelectChart { user: 1, id: 42, name }) if name == "Chart42"));
    assert_eq!(
        state_changes(&plan),
        vec![GameState::SelectChart { chart_id: Some(42) }]
    );
    assert_eq!(
        room.game_state_protocol(),
        GameState::SelectChart { chart_id: Some(42) }
    );
}

#[test]
fn select_chart_again_updates_chart() {
    let room = make_room();
    let p1 = player(1, "A");
    room.join(p1.clone(), false).unwrap();
    room.commit_select_chart(1, 1, "C1".into()).unwrap();
    room.commit_select_chart(1, 2, "C2".into()).unwrap();
    assert_eq!(
        room.game_state_protocol(),
        GameState::SelectChart { chart_id: Some(2) }
    );
}

#[test]
fn select_chart_wrong_state() {
    let room = make_room();
    let p1 = player(1, "A");
    setup_solo_playing(&room, &p1); // 进入 Playing
    assert_err(room.validate_select_chart(1), "ERROR_INVALID_STATE");
    assert_err(
        room.commit_select_chart(1, 99, "C".into()),
        "ERROR_INVALID_STATE",
    );
}

// ============================== 开局 ==============================

#[test]
fn require_start_solo_goes_directly_playing() {
    let room = make_room();
    let p1 = player(1, "A");
    room.join(p1.clone(), false).unwrap();
    let plan = room.require_start(1).unwrap();
    assert!(matches!(room.game_state_protocol(), GameState::Playing));
    assert_eq!(state_changes(&plan), vec![GameState::Playing]);
    assert!(matches!(message_for(&plan, 1), Some(Message::StartPlaying)));
}

#[test]
fn require_start_solo_without_chart_still_starts() {
    // 极端：未选曲直接开始（require_start 只校验状态，不校验 chart_id）
    let room = make_room();
    let p1 = player(1, "A");
    room.join(p1.clone(), false).unwrap();
    let plan = room.require_start(1).unwrap();
    assert!(matches!(room.game_state_protocol(), GameState::Playing));
    assert_eq!(state_changes(&plan), vec![GameState::Playing]);
}

#[test]
fn require_start_multi_enters_wait_for_ready() {
    let room = make_room();
    let p1 = player(1, "A");
    let p2 = player(2, "B");
    room.join(p1.clone(), false).unwrap();
    room.join(p2.clone(), false).unwrap();
    room.commit_select_chart(1, 42, "C42".into()).unwrap();
    let plan = room.require_start(1).unwrap();
    assert!(matches!(
        room.game_state_protocol(),
        GameState::WaitForReady
    ));
    assert_eq!(state_changes(&plan), vec![GameState::WaitForReady]);
    assert!(matches!(
        message_for(&plan, 1),
        Some(Message::GameStart { user: 1 })
    ));
    assert!(matches!(
        message_for(&plan, 2),
        Some(Message::GameStart { user: 1 })
    ));
}

#[test]
fn require_start_wrong_state() {
    let room = make_room();
    let p1 = player(1, "A");
    setup_solo_playing(&room, &p1); // Playing
    assert_err(room.require_start(1), "ERROR_INVALID_STATE");
}

#[test]
fn ready_all_starts_game() {
    let room = make_room();
    let p1 = player(1, "A");
    let p2 = player(2, "B");
    room.join(p1.clone(), false).unwrap();
    room.join(p2.clone(), false).unwrap();
    room.commit_select_chart(1, 42, "C42".into()).unwrap();
    room.require_start(1).unwrap(); // p1 自动 ready

    let (plan, started) = room.ready(p2.id()).unwrap();
    assert!(started);
    assert!(matches!(room.game_state_protocol(), GameState::Playing));
    // 广播：Ready(p2) message + ChangeState(Playing) + StartPlaying
    assert!(matches!(
        message_for(&plan, 1),
        Some(Message::Ready { user: 2 })
    ));
    assert_eq!(state_changes(&plan), vec![GameState::Playing]);
    assert!(packets_for(&plan, 1).iter().any(|p| matches!(
        p,
        ClientBoundPacket::Message {
            message: Message::StartPlaying,
            ..
        }
    )));
}

#[test]
fn ready_partial_does_not_start() {
    let room = make_room();
    let p1 = player(1, "A");
    let p2 = player(2, "B");
    let p3 = player(3, "C");
    room.join(p1.clone(), false).unwrap();
    room.join(p2.clone(), false).unwrap();
    room.join(p3.clone(), false).unwrap();
    room.commit_select_chart(1, 42, "C42".into()).unwrap();
    room.require_start(1).unwrap();

    let (plan, started) = room.ready(p2.id()).unwrap();
    assert!(!started, "p3 未 ready");
    assert!(matches!(
        room.game_state_protocol(),
        GameState::WaitForReady
    ));
    assert!(!plan.iter().any(|(_, f)| {
        matches!(
            common::decode_frame_payload(f),
            Some(ClientBoundPacket::ChangeState {
                game_state: GameState::Playing,
                ..
            })
        )
    }));

    // p3 ready → 开始
    let (_, started) = room.ready(p3.id()).unwrap();
    assert!(started);
    assert!(matches!(room.game_state_protocol(), GameState::Playing));
}

#[test]
fn ready_idempotent() {
    let room = make_room();
    let p1 = player(1, "A");
    let p2 = player(2, "B");
    room.join(p1.clone(), false).unwrap();
    room.join(p2.clone(), false).unwrap();
    room.commit_select_chart(1, 42, "C42".into()).unwrap();
    room.require_start(1).unwrap();

    // 第一次 ready 已开局
    let (_, started) = room.ready(p2.id()).unwrap();
    assert!(started);
    // Playing 状态下再 ready → 错误状态
    assert_err(room.ready(p2.id()), "ERROR_INVALID_STATE");
}

#[test]
fn ready_in_wrong_state() {
    let room = make_room();
    let p1 = player(1, "A");
    room.join(p1.clone(), false).unwrap();
    assert_err(room.ready(1), "ERROR_INVALID_STATE"); // SelectChart
    setup_solo_playing(&room, &p1);
    assert_err(room.ready(1), "ERROR_INVALID_STATE"); // Playing
}

#[test]
fn cancel_ready_removes_and_broadcasts() {
    let room = make_room();
    let p1 = player(1, "A");
    let p2 = player(2, "B");
    room.join(p1.clone(), false).unwrap();
    room.join(p2.clone(), false).unwrap();
    room.commit_select_chart(1, 42, "C42".into()).unwrap();
    room.require_start(1).unwrap();

    let plan = room.cancel_ready(1).unwrap();
    assert!(matches!(
        message_for(&plan, 2),
        Some(Message::CancelReady { user: 1 })
    ));
    // p1 取消后 p2 ready 不再能开局（p1 未 ready）
    let (_, started) = room.ready(p2.id()).unwrap();
    assert!(!started);
    assert!(matches!(
        room.game_state_protocol(),
        GameState::WaitForReady
    ));
}

#[test]
fn cancel_ready_wrong_state() {
    let room = make_room();
    let p1 = player(1, "A");
    room.join(p1.clone(), false).unwrap();
    assert_err(room.cancel_ready(1), "ERROR_INVALID_STATE");
}

// ============================== 对局结束 ==============================

#[test]
fn solo_played_ends_game() {
    let room = make_room();
    let p1 = player(1, "A");
    setup_solo_playing(&room, &p1);
    let outcome = room.commit_played(1, 990_000, 99.0, true).unwrap();
    assert!(outcome.game_ended);
    assert!(outcome.recording.is_none()); // 无 touch/judge
    assert!(matches!(
        room.game_state_protocol(),
        GameState::SelectChart { chart_id: Some(42) }
    ));
    // 广播含 Played message + ChangeState(SelectChart) + GameEnd
    let pkts = packets_for(&outcome.broadcasts, 1);
    assert!(pkts.iter().any(|p| matches!(
        p,
        ClientBoundPacket::Message {
            message: Message::Played {
                user: 1,
                score: 990_000,
                ..
            },
            ..
        }
    )));
    assert!(pkts.iter().any(|p| matches!(
        p,
        ClientBoundPacket::ChangeState {
            game_state: GameState::SelectChart { chart_id: Some(42) },
            ..
        }
    )));
    assert!(pkts.iter().any(|p| matches!(
        p,
        ClientBoundPacket::Message {
            message: Message::GameEnd,
            ..
        }
    )));
}

#[test]
fn played_idempotent_within_game() {
    // 幂等只发生在同一对局内（done 已含 → 空计划、不重复结束）
    let room = make_room();
    let p1 = player(1, "A");
    let p2 = player(2, "B");
    setup_duo_playing(&room, &p1, &p2);

    let first = room.commit_played(1, 100, 10.0, false).unwrap();
    assert!(!first.game_ended, "p2 未完成");
    // 同一对局内重复 played → 幂等空计划
    let second = room.commit_played(1, 200, 20.0, true).unwrap();
    assert!(!second.game_ended);
    assert!(second.broadcasts.is_empty());
    assert!(second.recording.is_none());

    // 对局结束后状态回到 SelectChart，再 played 属错误状态
    room.commit_played(2, 300, 30.0, false).unwrap(); // 全员完成 → 结束
    assert!(matches!(
        room.game_state_protocol(),
        GameState::SelectChart { chart_id: Some(42) }
    ));
    assert_err(
        room.commit_played(1, 400, 40.0, true),
        "ERROR_INVALID_STATE",
    );
}

#[test]
fn abort_idempotent_within_game() {
    let room = make_room();
    let p1 = player(1, "A");
    let p2 = player(2, "B");
    setup_duo_playing(&room, &p1, &p2);
    let outcome = room.commit_abort(1).unwrap();
    assert!(!outcome.game_ended, "p2 未完成");
    assert!(packets_for(&outcome.broadcasts, 1).iter().any(|p| matches!(
        p,
        ClientBoundPacket::Message {
            message: Message::Abort { user: 1 },
            ..
        }
    )));
    // 同一对局内重复 abort → 幂等空计划
    let second = room.commit_abort(1).unwrap();
    assert!(!second.game_ended);
    assert!(second.broadcasts.is_empty());
    // 对局结束后 abort → 错误状态
    room.commit_played(2, 1, 1.0, false).unwrap();
    assert!(matches!(
        room.game_state_protocol(),
        GameState::SelectChart { chart_id: Some(42) }
    ));
    assert_commit_err(room.commit_abort(1), "ERROR_INVALID_STATE");
}

#[test]
fn played_in_wrong_state() {
    let room = make_room();
    let p1 = player(1, "A");
    room.join(p1.clone(), false).unwrap();
    assert_err(room.commit_played(1, 1, 1.0, false), "ERROR_INVALID_STATE");
    assert_err(room.commit_abort(1), "ERROR_INVALID_STATE");
}

#[test]
fn duo_game_ends_when_all_done() {
    let room = make_room();
    let p1 = player(1, "A");
    let p2 = player(2, "B");
    setup_duo_playing(&room, &p1, &p2);

    // p1 abort，p2 未完成 → 不结束
    let out1 = room.commit_abort(1).unwrap();
    assert!(!out1.game_ended);
    assert!(matches!(room.game_state_protocol(), GameState::Playing));

    // p2 played → 全员完成 → 结束
    let out2 = room.commit_played(2, 500_000, 95.0, false).unwrap();
    assert!(out2.game_ended);
    assert!(matches!(
        room.game_state_protocol(),
        GameState::SelectChart { chart_id: Some(42) }
    ));
    // 两个玩家都收到 GameEnd
    assert!(packets_for(&out2.broadcasts, 1).iter().any(|p| matches!(
        p,
        ClientBoundPacket::Message {
            message: Message::GameEnd,
            ..
        }
    )));
    assert!(packets_for(&out2.broadcasts, 2).iter().any(|p| matches!(
        p,
        ClientBoundPacket::Message {
            message: Message::GameEnd,
            ..
        }
    )));
}

#[test]
fn all_offline_ends_game() {
    let room = make_room();
    let p1 = player(1, "A");
    let p2 = player(2, "B");
    setup_duo_playing(&room, &p1, &p2);
    // 全员掉线
    p1.set_online(false);
    p2.set_online(false);
    // 任意一人 played → 判定：无在线玩家 → 结束
    let out = room.commit_played(1, 0, 0.0, false).unwrap();
    assert!(out.game_ended);
    assert!(matches!(
        room.game_state_protocol(),
        GameState::SelectChart { chart_id: Some(42) }
    ));
}

#[test]
fn offline_player_does_not_block_game_end() {
    let room = make_room();
    let p1 = player(1, "A");
    let p2 = player(2, "B");
    let p3 = player(3, "C");
    room.join(p1.clone(), false).unwrap();
    room.join(p2.clone(), false).unwrap();
    room.join(p3.clone(), false).unwrap();
    room.commit_select_chart(1, 42, "C42".into()).unwrap();
    room.require_start(1).unwrap();
    room.ready(2).unwrap();
    room.ready(3).unwrap();
    assert!(matches!(room.game_state_protocol(), GameState::Playing));

    // p3 掉线（在线判定只考虑在线玩家）
    p3.set_online(false);
    let out1 = room.commit_abort(1).unwrap();
    assert!(!out1.game_ended);
    let out2 = room.commit_played(2, 1, 1.0, false).unwrap();
    assert!(out2.game_ended, "掉线者不应阻塞结束");
}

#[test]
fn check_played_semantics() {
    use phira_mp::room::PlayedCheck;
    let room = make_room();
    let p1 = player(1, "A");
    let p2 = player(2, "B");
    setup_duo_playing(&room, &p1, &p2);
    assert!(matches!(
        room.check_played(1).unwrap(),
        PlayedCheck::CanPlay {
            chart_id: Some(42),
            ..
        }
    ));
    room.commit_abort(1).unwrap();
    assert!(matches!(
        room.check_played(1).unwrap(),
        PlayedCheck::AlreadyDone
    ));
    assert!(matches!(
        room.check_played(2).unwrap(),
        PlayedCheck::CanPlay { .. }
    ));
    // 对局结束后（状态回 SelectChart）→ 错误状态
    room.commit_abort(2).unwrap();
    assert!(matches!(
        room.game_state_protocol(),
        GameState::SelectChart { chart_id: Some(42) }
    ));
    assert_err(room.check_played(1), "ERROR_INVALID_STATE");
}

// ============================== touch/judge 与录制 ==============================

fn one_frame(t: f32) -> TouchFrame {
    TouchFrame {
        time: t,
        points: vec![TouchPoint {
            id: 0,
            pos: CompactPos::from_f32(0.5, 0.5),
        }],
    }
}

fn one_judge(t: f32) -> JudgeEvent {
    JudgeEvent {
        time: t,
        line_id: 0,
        note_id: 1,
        judgement: Judgement::Perfect,
    }
}

#[test]
fn touch_collect_only_in_playing() {
    let room = make_room();
    let p1 = player(1, "A");
    room.join(p1.clone(), false).unwrap();
    // SelectChart 状态：不收集
    room.touch_send(1, vec![one_frame(1.0)]);
    assert_commit_err(room.commit_played(1, 1, 1.0, false), "ERROR_INVALID_STATE"); // 未开局，无法 played 验证录制

    // Playing 状态：收集
    room.commit_select_chart(1, 42, "C42".into()).unwrap();
    room.require_start(1).unwrap();
    room.touch_send(1, vec![one_frame(1.0), one_frame(2.0)]);
    room.judge_send(1, vec![one_judge(1.5)]);
    let out = room.commit_played(1, 100, 10.0, false).unwrap();
    let rec = out.recording.expect("recording extracted");
    assert_eq!(rec.chart_id, Some(42));
    assert_eq!(rec.chart_name.as_deref(), Some("C42"));
    assert_eq!(rec.touch_frames.len(), 2);
    assert_eq!(rec.judge_events.len(), 1);
}

#[test]
fn touch_collected_per_player() {
    let room = make_room();
    let p1 = player(1, "A");
    let p2 = player(2, "B");
    setup_duo_playing(&room, &p1, &p2);
    room.touch_send(1, vec![one_frame(1.0)]);
    room.touch_send(2, vec![one_frame(1.0), one_frame(2.0)]);
    let out1 = room.commit_played(1, 1, 1.0, false).unwrap();
    assert_eq!(out1.recording.as_ref().unwrap().touch_frames.len(), 1);
    let out2 = room.commit_played(2, 1, 1.0, false).unwrap();
    assert_eq!(out2.recording.as_ref().unwrap().touch_frames.len(), 2);
}

#[test]
fn no_touch_no_recording() {
    let room = make_room();
    let p1 = player(1, "A");
    setup_solo_playing(&room, &p1);
    let out = room.commit_played(1, 1, 1.0, false).unwrap();
    assert!(out.recording.is_none());
}

#[test]
fn touch_forwarded_to_monitor_in_any_state() {
    let room = make_room();
    let p1 = player(1, "A");
    let m = player(50, "M");
    room.join(p1.clone(), false).unwrap();
    room.join(m.clone(), true).unwrap();
    // SelectChart 状态也转发（无条件）
    let plan = room.touch_send(1, vec![one_frame(1.0)]);
    let m_pkts = packets_for(&plan, 50);
    assert!(m_pkts.iter().any(|p| matches!(p,
        ClientBoundPacket::Touches { from_player_id: 1, frames, .. } if frames.len() == 1)));
    // 非 Playing 状态不收集，但转发仍发生
    assert_commit_err(room.commit_played(1, 1, 1.0, false), "ERROR_INVALID_STATE");

    // judge 同理
    let plan = room.judge_send(1, vec![one_judge(2.0)]);
    assert!(packets_for(&plan, 50).iter().any(|p| matches!(p,
        ClientBoundPacket::Judges { from_player_id: 1, judges, .. } if judges.len() == 1)));
    // 转发不回发给玩家自己
    assert!(packets_for(&plan, 1).is_empty());
}

#[test]
fn touch_forwarded_in_playing_too() {
    let room = make_room();
    let p1 = player(1, "A");
    let m = player(50, "M");
    room.join(p1.clone(), false).unwrap();
    room.join(m.clone(), true).unwrap();
    room.commit_select_chart(1, 42, "C42".into()).unwrap();
    // 含 monitor 共 2 人 → WaitForReady；monitor 也需 ready
    room.require_start(1).unwrap();
    room.ready(50).unwrap();
    assert!(matches!(room.game_state_protocol(), GameState::Playing));
    let plan = room.touch_send(1, vec![one_frame(1.0)]);
    assert!(packets_for(&plan, 50).iter().any(|p| matches!(
        p,
        ClientBoundPacket::Touches {
            from_player_id: 1,
            ..
        }
    )));
}

// ============================== 房主转移 ==============================

#[test]
fn host_transfer_by_ascending_user_id() {
    let room = make_room();
    room.join(player(5, "E"), false).unwrap();
    room.join(player(2, "B"), false).unwrap();
    room.join(player(9, "I"), false).unwrap();
    assert!(room.is_host(5));
    room.leave(5);
    assert!(room.is_host(9), "升序取下一个（>5 的最小 id）");
    room.leave(9);
    assert!(room.is_host(2), "无更大 id 时取最小");
}

#[test]
fn host_transfer_broadcasts() {
    let room = make_room();
    let p1 = player(1, "A");
    let p2 = player(2, "B");
    let p3 = player(3, "C");
    room.join(p1.clone(), false).unwrap();
    room.join(p2.clone(), false).unwrap();
    room.join(p3.clone(), false).unwrap();
    let (_left, plan, _d) = room.leave(1);
    // 旧房主收到 ChangeHost(false)
    assert!(
        packets_for(&plan, 1)
            .iter()
            .any(|p| matches!(p, ClientBoundPacket::ChangeHost { is_host: false, .. }))
    );
    // 新房主（2）收到 ChangeHost(true)，不收 NewHost
    assert!(
        packets_for(&plan, 2)
            .iter()
            .any(|p| matches!(p, ClientBoundPacket::ChangeHost { is_host: true, .. }))
    );
    assert!(!packets_for(&plan, 2).iter().any(|p| matches!(
        p,
        ClientBoundPacket::Message {
            message: Message::NewHost { .. },
            ..
        }
    )));
    // 其他成员（3）收到 NewHost(2) 与 LeaveRoom(1)（NewHost 先广播）
    assert!(matches!(
        message_for(&plan, 3),
        Some(Message::NewHost { user: 2 })
    ));
    assert!(packets_for(&plan, 3).iter().any(|p| matches!(
        p,
        ClientBoundPacket::Message {
            message: Message::LeaveRoom { user: 1, .. },
            ..
        }
    )));
}

#[test]
fn host_transfer_last_player_clears_host() {
    let room = make_room();
    let p1 = player(1, "A");
    let p2 = player(2, "B");
    room.join(p1.clone(), false).unwrap();
    room.join(p2.clone(), false).unwrap();
    room.leave(1);
    assert!(room.is_host(2));
    // 房主 2 离开，无人可转移
    let (_left, _plan, destroyed) = room.leave(2);
    assert!(destroyed, "最后一人离开 → 销毁");
}

#[test]
fn cycle_mode_transfers_host_on_game_end() {
    let room = LocalRoom::new(
        "CY",
        RoomSetting {
            cycle: true,
            ..Default::default()
        },
        || {},
    );
    let p1 = player(1, "A");
    let p2 = player(2, "B");
    let p3 = player(3, "C");
    room.join(p1.clone(), false).unwrap();
    room.join(p2.clone(), false).unwrap();
    room.join(p3.clone(), false).unwrap();
    room.commit_select_chart(1, 42, "C42".into()).unwrap();
    room.require_start(1).unwrap();
    room.ready(2).unwrap();
    room.ready(3).unwrap();
    assert!(room.is_host(1));

    room.commit_abort(2).unwrap();
    room.commit_played(3, 1, 1.0, false).unwrap();
    // p1 完成后全员 done → GameEnd
    let out = room.commit_abort(1).unwrap();
    assert!(out.game_ended);
    // cycle 模式：房主转移到 2（>1 的最小 id）
    assert!(room.is_host(2), "cycle 模式下对局结束转移房主");
    assert!(
        packets_for(&out.broadcasts, 2)
            .iter()
            .any(|p| matches!(p, ClientBoundPacket::ChangeHost { is_host: true, .. }))
    );
    assert!(
        packets_for(&out.broadcasts, 1)
            .iter()
            .any(|p| matches!(p, ClientBoundPacket::ChangeHost { is_host: false, .. }))
    );
}

#[test]
fn non_cycle_mode_keeps_host_on_game_end() {
    let room = make_room(); // cycle = false
    let p1 = player(1, "A");
    let p2 = player(2, "B");
    setup_duo_playing(&room, &p1, &p2);
    room.commit_abort(2).unwrap();
    room.commit_played(1, 1, 1.0, false).unwrap();
    assert!(room.is_host(1), "非 cycle 模式房主不变");
}

// ============================== 挂起清理 ==============================

#[test]
fn cleanup_wait_ready_sends_cancel_ready() {
    let room = make_room();
    let p1 = player(1, "A");
    let p2 = player(2, "B");
    room.join(p1.clone(), false).unwrap();
    room.join(p2.clone(), false).unwrap();
    room.commit_select_chart(1, 42, "C42".into()).unwrap();
    room.require_start(1).unwrap(); // WaitForReady，p1 自动 ready
    assert!(matches!(
        room.game_state_protocol(),
        GameState::WaitForReady
    ));

    // 挂起清理：p1 的 ready 被撤销并广播
    let plan = room.cleanup_for_suspend(1);
    assert!(matches!(
        message_for(&plan, 2),
        Some(Message::CancelReady { user: 1 })
    ));
    // 未 ready 的玩家不受影响
    let plan2 = room.cleanup_for_suspend(2);
    assert!(plan2.is_empty(), "p2 未 ready，无需清理");
}

#[test]
fn cleanup_playing_sends_abort_and_may_end_game() {
    let room = make_room();
    let p1 = player(1, "A");
    setup_solo_playing(&room, &p1);
    let plan = room.cleanup_for_suspend(1);
    // 单人：abort + 全员（唯一在线）完成 → GameEnd + 回 SelectChart
    assert!(matches!(
        message_for(&plan, 1),
        Some(Message::Abort { user: 1 })
    ));
    assert!(matches!(
        room.game_state_protocol(),
        GameState::SelectChart { chart_id: Some(42) }
    ));
    let pkts = packets_for(&plan, 1);
    assert!(pkts.iter().any(|p| matches!(
        p,
        ClientBoundPacket::Message {
            message: Message::GameEnd,
            ..
        }
    )));
}

#[test]
fn cleanup_playing_abort_idempotent() {
    let room = make_room();
    let p1 = player(1, "A");
    let p2 = player(2, "B");
    setup_duo_playing(&room, &p1, &p2);
    // p1 挂起清理 → abort(1)，p2 未完成 → 不结束
    let plan = room.cleanup_for_suspend(1);
    assert!(matches!(
        message_for(&plan, 1),
        Some(Message::Abort { user: 1 })
    ));
    assert!(matches!(room.game_state_protocol(), GameState::Playing));
    // 再次清理同一个人 → 幂等（done 已含 1）
    let plan2 = room.cleanup_for_suspend(1);
    assert!(plan2.is_empty());
}

// ============================== 快照与会话信息 ==============================

#[test]
fn build_room_info_viewer_perspective() {
    let room = make_room();
    let p1 = player(1, "A");
    let p2 = player(2, "B");
    let m = player(50, "M");
    room.join(p1.clone(), false).unwrap();
    room.join(p2.clone(), false).unwrap();
    room.join(m.clone(), true).unwrap();
    room.commit_select_chart(1, 42, "C42".into()).unwrap();

    let info = room.build_room_info(&*p1);
    assert_eq!(info.room_id, "R1");
    assert!(info.is_host);
    assert_eq!(info.state, GameState::SelectChart { chart_id: Some(42) });
    assert!(!info.is_ready);
    assert_eq!(info.users.len(), 3);
    // users 顺序：players 在前（非 monitor），monitors 在后（monitor=true）
    assert_eq!(info.users[0].user_id, 1);
    assert!(!info.users[0].monitor);
    assert_eq!(info.users[1].user_id, 2);
    assert!(!info.users[1].monitor);
    assert_eq!(info.users[2].user_id, 50);
    assert!(info.users[2].monitor);

    // viewer = monitor → is_host false
    let info_m = room.build_room_info(&*m);
    assert!(!info_m.is_host);
}

#[test]
fn build_room_info_wait_ready_ready_always_true() {
    let room = make_room();
    let p1 = player(1, "A");
    let p2 = player(2, "B");
    room.join(p1.clone(), false).unwrap();
    room.join(p2.clone(), false).unwrap();
    room.commit_select_chart(1, 42, "C42".into()).unwrap();
    room.require_start(1).unwrap(); // WaitForReady
    // 无论 viewer 是否 ready，is_ready 恒 true（易错点 3）
    let info = room.build_room_info(&*p2);
    assert!(info.is_ready);
    assert!(matches!(info.state, GameState::WaitForReady));
}

#[test]
fn join_room_data_matches() {
    let room = make_room();
    let p1 = player(1, "A");
    let p2 = player(2, "B");
    room.join(p1.clone(), false).unwrap();
    room.join(p2.clone(), false).unwrap();
    room.commit_select_chart(1, 42, "C42".into()).unwrap();
    let data = room.join_room_data();
    assert_eq!(
        data.game_state,
        GameState::SelectChart { chart_id: Some(42) }
    );
    assert_eq!(data.users.len(), 2);
    assert!(!data.live);
}

#[test]
fn snapshot_fields() {
    let room = make_room();
    let p1 = player(1, "A");
    let p2 = player(2, "B");
    room.join(p1.clone(), false).unwrap();
    room.join(p2.clone(), false).unwrap();
    room.commit_select_chart(1, 42, "Chart42".into()).unwrap();
    let snap = room.snapshot();
    assert_eq!(snap.room_id, "R1");
    assert_eq!(snap.state_kind(), "SelectChart");
    assert_eq!(snap.players, vec![1, 2]);
    assert!(snap.monitors.is_empty());
    assert_eq!(snap.host, Some(1));
    assert_eq!(snap.chart_id, Some(42));
    assert_eq!(snap.chart_name.as_deref(), Some("Chart42"));
    assert!(!snap.locked);
}

#[test]
fn setting_host_disabled_means_no_host() {
    let room = LocalRoom::new(
        "NH",
        RoomSetting {
            host: false,
            ..Default::default()
        },
        || {},
    );
    let p1 = player(1, "A");
    room.join(p1.clone(), false).unwrap();
    assert!(!room.is_host(1));
    assert_eq!(room.snapshot().host, None);
    // 无房主 → 全员无权 toggle_lock
    assert_err(room.toggle_lock(1), "ERROR_PERMISSION_DENIED");
}

#[test]
fn game_records_is_empty() {
    // 当前实现 records() 恒空（留待对接成绩系统）
    let room = make_room();
    let p1 = player(1, "A");
    setup_solo_playing(&room, &p1);
    room.commit_played(1, 100, 10.0, false).unwrap();
    assert!(room.game_records().is_empty());
}

#[test]
fn is_monitor_user_and_contains() {
    let room = make_room();
    room.join(player(1, "A"), false).unwrap();
    room.join(player(2, "M"), true).unwrap();
    assert!(room.contains_member(1));
    assert!(room.contains_member(2));
    assert!(room.is_monitor_user(2));
    assert!(!room.is_monitor_user(1));
    assert!(!room.contains_member(999));
}

// ============================== 无响应钩子 ==============================

#[tokio::test]
async fn send_frame_hook_collects_frames() {
    // 验证 TestPlayer 的 send_frame 会被广播链调用（room.send_broadcasts 路径）
    let room = make_room();
    let p1 = player(1, "A");
    let p2 = player(2, "B");
    room.join(p1.clone(), false).unwrap();
    room.join(p2.clone(), false).unwrap();
    let plan = room.chat(1, "ping".into()).unwrap();
    phira_mp::room::send_broadcasts(plan).await;
    assert_eq!(
        p2.messages(),
        vec![Message::Chat {
            user: 1,
            content: "ping".into()
        }]
    );
    // p1 也被广播
    assert_eq!(p1.messages().len(), 1);
}
