use super::*;

// ---------------- 锁/循环（对应 LocalRoomOperationTest） ----------------

#[test]
fn lock_toggle_ignores_client_value() {
    let room = make_room();
    room.join(player(1, "A"), false).unwrap();
    room.toggle_lock(1).unwrap();
    let err = room.join(player(2, "B"), false).unwrap_err();
    assert_eq!(err.0, "ERROR_ROOM_LOCKED");
    room.join(player(3, "M"), true).unwrap();
    room.toggle_lock(1).unwrap();
    room.join(player(2, "B"), false).unwrap();
    assert_eq!(
        room.toggle_lock(2).unwrap_err().0,
        "ERROR_PERMISSION_DENIED"
    );
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
    assert_eq!(
        room.toggle_cycle(2).unwrap_err().0,
        "ERROR_PERMISSION_DENIED"
    );
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
    let setting = RoomSetting {
        chat: false,
        ..Default::default()
    };
    let room = LocalRoom::new("RN", setting, || {});
    room.join(player(1, "A"), false).unwrap();
    assert_eq!(
        room.chat(1, "x".into()).unwrap_err().0,
        "ERROR_CHAT_NOT_ENABLED"
    );
}
