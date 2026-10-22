use super::*;

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
    assert_eq!(err.0, "ERROR_ROOM_FULL");
    room.join(player(3, "M"), true).unwrap();
    assert!(room.is_monitor_user(3));
}

#[test]
fn leave_removes_member_and_last_leaves_destroys() {
    use std::sync::atomic::{AtomicBool, Ordering};
    let destroyed = Arc::new(AtomicBool::new(false));
    let d = destroyed.clone();
    let setting = RoomSetting {
        auto_destroy: true,
        ..Default::default()
    };
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
    assert!(
        pkts.iter()
            .any(|p| matches!(p, ClientBoundPacket::ChangeHost { is_host: true, .. }))
    );
    // NewHost 消息广播给非新房主成员（玩家 2 是新房主，不接收 NewHost）
}
