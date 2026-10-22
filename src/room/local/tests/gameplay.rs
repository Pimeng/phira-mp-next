use super::*;

// ---------------- 选谱 ----------------

#[test]
fn select_chart_host_only_and_state() {
    let room = make_room();
    room.join(player(1, "A"), false).unwrap();
    room.join(player(2, "B"), false).unwrap();
    assert_eq!(
        room.validate_select_chart(2).unwrap_err().0,
        "ERROR_PERMISSION_DENIED"
    );
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
    assert_eq!(
        room.validate_select_chart(1).unwrap_err().0,
        "ERROR_INVALID_STATE"
    );
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
    assert!(pkts.iter().any(|p| matches!(
        p,
        ClientBoundPacket::ChangeState {
            game_state: GameState::Playing,
            ..
        }
    )));
    assert!(pkts.iter().any(|p| matches!(
        p,
        ClientBoundPacket::Message {
            message: Message::StartPlaying,
            ..
        }
    )));
}

#[test]
fn multi_player_ready_flow() {
    let room = make_room();
    room.join(player(1, "A"), false).unwrap();
    room.join(player(2, "B"), false).unwrap();
    room.commit_select_chart(1, 42, "Chart".into()).unwrap();
    room.require_start(1).unwrap();
    assert!(matches!(
        room.game_state_protocol(),
        GameState::WaitForReady
    ));
    let (plan, started) = room.ready(2).unwrap();
    assert!(started);
    assert!(matches!(room.game_state_protocol(), GameState::Playing));
    assert_eq!(room.cancel_ready(1).unwrap_err().0, "ERROR_INVALID_STATE");
    // ready 广播 Ready{user:2}
    assert!(matches!(
        message_for(&plan, 1),
        Some(Message::Ready { user: 2 })
    ));
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
    assert!(matches!(
        room.game_state_protocol(),
        GameState::WaitForReady
    ));
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
    assert!(matches!(
        message_for(&plan, 1),
        Some(Message::CancelReady { user: 2 })
    ));
    // ready 集合只有 {1}，online={1,2,3}，不应开局
    assert!(matches!(
        room.game_state_protocol(),
        GameState::WaitForReady
    ));
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
    assert_eq!(room.require_start(1).unwrap_err().0, "ERROR_INVALID_STATE");
    assert_eq!(room.ready(1).unwrap_err().0, "ERROR_INVALID_STATE");
    assert_eq!(room.cancel_ready(1).unwrap_err().0, "ERROR_INVALID_STATE");
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
        Some(Message::Played {
            user,
            score,
            full_combo,
            ..
        }) => {
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
    assert!(matches!(
        message_for(&out1.broadcasts, 2),
        Some(Message::Abort { user: 1 })
    ));
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
    room.touch_send(
        1,
        vec![crate::packet::data::TouchFrame {
            time: 1.0,
            points: vec![],
        }],
    );
    room.judge_send(
        1,
        vec![crate::packet::data::JudgeEvent {
            time: 1.0,
            line_id: 0,
            note_id: 0,
            judgement: crate::packet::data::Judgement::Perfect,
        }],
    );
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
    assert_eq!(
        room.commit_played(1, 1, 1.0, true).err().unwrap().0,
        "ERROR_INVALID_STATE"
    );
    assert_eq!(room.commit_abort(1).err().unwrap().0, "ERROR_INVALID_STATE");
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
    let f = room.touch_send(
        1,
        vec![crate::packet::data::TouchFrame {
            time: 0.1,
            points: vec![],
        }],
    );
    assert_eq!(f.len(), 1, "non-Playing also forwards to monitor");
    // 但收集仅在 Playing（非 Playing 不采集 → 录制无数据）
    room.commit_select_chart(1, 1, "C".into()).unwrap();
    room.require_start(1).unwrap(); // total=2（含 monitor）→ WaitForReady
    room.ready(9).unwrap(); // monitor ready → 全员就绪 → Playing
    assert!(matches!(room.game_state_protocol(), GameState::Playing));
    let f = room.touch_send(
        1,
        vec![crate::packet::data::TouchFrame {
            time: 0.5,
            points: vec![],
        }],
    );
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
    let f = room.judge_send(
        1,
        vec![crate::packet::data::JudgeEvent {
            time: 1.0,
            line_id: 0,
            note_id: 0,
            judgement: crate::packet::data::Judgement::Good,
        }],
    );
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
    assert!(matches!(
        message_for(&plan, 2),
        Some(Message::Abort { user: 1 })
    ));
    let out = room.commit_played(2, 1, 1.0, true).unwrap();
    assert!(out.game_ended);
}
