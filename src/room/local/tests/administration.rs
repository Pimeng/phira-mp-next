use super::*;

// ---------------- 管理员操作（控制台命令） ----------------

#[test]
fn admin_set_max_player_validates_and_applies() {
    let room = make_room();
    room.join(player(1, "A"), false).unwrap();
    assert_eq!(
        room.admin_set_max_player(0).unwrap_err().0,
        "ERROR_INVALID_ARGUMENT"
    );
    room.admin_set_max_player(2).unwrap();
    assert_eq!(room.setting().max_player, 2);
    room.join(player(2, "B"), false).unwrap();
    assert_eq!(
        room.join(player(3, "C"), false).unwrap_err().0,
        "ERROR_ROOM_FULL"
    );
}

#[test]
fn admin_set_locked_forces_without_host() {
    let room = make_room();
    room.join(player(1, "A"), false).unwrap();
    room.join(player(2, "B"), false).unwrap();
    // 管理员强制锁定（无需房主鉴权）
    let plan = room.admin_set_locked(true).unwrap();
    assert!(room.setting().locked);
    assert_eq!(
        room.join(player(3, "C"), false).unwrap_err().0,
        "ERROR_ROOM_LOCKED"
    );
    match message_for(&plan, 2) {
        Some(Message::LockRoom { lock }) => assert!(lock),
        other => panic!("expected LockRoom(true), got {other:?}"),
    }
    room.admin_set_locked(false).unwrap();
    assert!(!room.setting().locked);
    room.join(player(3, "C"), false).unwrap();
}

#[test]
fn admin_set_cycle_forces_without_host() {
    let room = make_room();
    room.join(player(1, "A"), false).unwrap();
    let plan = room.admin_set_cycle(true).unwrap();
    assert!(room.setting().cycle);
    match message_for(&plan, 1) {
        Some(Message::CycleRoom { cycle }) => assert!(cycle),
        other => panic!("expected CycleRoom(true), got {other:?}"),
    }
}

#[test]
fn admin_chat_bypasses_chat_switch_and_uses_system_user() {
    let setting = RoomSetting {
        chat: false,
        ..Default::default()
    };
    let room = LocalRoom::new("RN", setting, || {});
    room.join(player(1, "A"), false).unwrap();
    let plan = room.admin_chat("system msg".into()).unwrap();
    match message_for(&plan, 1) {
        Some(Message::Chat { user, content }) => {
            assert_eq!(user, 0, "系统消息 user=0");
            assert_eq!(content, "system msg");
        }
        other => panic!("expected Chat, got {other:?}"),
    }
}

#[test]
fn admin_next_host_requires_member_and_applies_on_cycle_end() {
    let setting = RoomSetting {
        cycle: true,
        ..Default::default()
    };
    let room = LocalRoom::new("RC", setting, || {});
    room.join(player(1, "A"), false).unwrap();
    room.join(player(2, "B"), false).unwrap();
    assert_eq!(
        room.admin_set_next_host(99).unwrap_err().0,
        "ERROR_PLAYER_NOT_IN_ROOM"
    );
    room.admin_set_next_host(2).unwrap();

    room.commit_select_chart(1, 1, "C".into()).unwrap();
    room.require_start(1).unwrap();
    room.ready(2).unwrap(); // Playing
    room.commit_played(1, 1, 1.0, true).unwrap();
    let out = room.commit_played(2, 1, 1.0, true).unwrap();
    assert!(out.game_ended);
    assert!(room.is_host(2), "nexthost 指定应在循环对局结束后生效");
    // 新房主外的成员收到 NewHost{2}
    let pkts = packet_for(&out.broadcasts, 1);
    assert!(pkts.iter().any(|p| matches!(
        p,
        ClientBoundPacket::Message {
            message: Message::NewHost { user: 2 },
            ..
        }
    )));
}

#[test]
fn admin_next_host_current_host_skips_transfer() {
    let setting = RoomSetting {
        cycle: true,
        ..Default::default()
    };
    let room = LocalRoom::new("RC", setting, || {});
    room.join(player(1, "A"), false).unwrap();
    room.join(player(2, "B"), false).unwrap();
    room.admin_set_next_host(1).unwrap(); // 指定当前房主 → 本轮不转移

    room.commit_select_chart(1, 1, "C".into()).unwrap();
    room.require_start(1).unwrap();
    room.ready(2).unwrap();
    room.commit_played(1, 1, 1.0, true).unwrap();
    let out = room.commit_played(2, 1, 1.0, true).unwrap();
    assert!(out.game_ended);
    assert!(room.is_host(1), "指定当前房主视为不转移");
}

#[test]
fn admin_transfer_host_immediate() {
    let room = make_room();
    room.join(player(1, "A"), false).unwrap();
    room.join(player(2, "B"), false).unwrap();
    assert!(room.is_host(1));
    assert_eq!(
        room.admin_transfer_host(99).unwrap_err().0,
        "ERROR_PLAYER_NOT_IN_ROOM"
    );
    let plan = room.admin_transfer_host(2).unwrap();
    assert!(room.is_host(2));
    assert!(!room.is_host(1));
    // 旧房主 ChangeHost(false)，新房主 ChangeHost(true)
    let pkts1 = packet_for(&plan, 1);
    assert!(
        pkts1
            .iter()
            .any(|p| matches!(p, ClientBoundPacket::ChangeHost { is_host: false, .. }))
    );
    let pkts2 = packet_for(&plan, 2);
    assert!(
        pkts2
            .iter()
            .any(|p| matches!(p, ClientBoundPacket::ChangeHost { is_host: true, .. }))
    );
}
