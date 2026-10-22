use super::*;
use crate::network::connection::{ConnectionHandle, WriterMsg};
use crate::packet::clientbound::ClientBoundPacket;
use crate::packet::message::Message;
use crate::phira::UserInfo;
use crate::server::ServerArgs;
use tokio::sync::mpsc;

/// 独立 ServerContext（不启动网络；GLOBAL_CTX 被覆盖不影响本测试，均不依赖它）。
fn ctx() -> Arc<ServerContext> {
    ServerContext::new(ServerArgs {
        host: "127.0.0.1".into(),
        language: "zh-CN".into(),
        session_timeout: 300,
        phira_api: "http://127.0.0.1/".into(),
        ..Default::default()
    })
}

/// 注册一个在线 LocalPlayer，返回 (玩家, writer 通道接收端)。
/// rx 保持存活 → is_online() == true；可从 rx 读取发出的帧。
fn register(
    ctx: &Arc<ServerContext>,
    id: i32,
    name: &str,
) -> (
    Arc<dyn crate::player::Player>,
    mpsc::UnboundedReceiver<WriterMsg>,
) {
    let (tx, rx) = mpsc::unbounded_channel::<WriterMsg>();
    let conn = ConnectionHandle::new_for_test(tx);
    let info = Arc::new(UserInfo {
        id,
        name: name.into(),
        ..Default::default()
    });
    let (res, _old) = ctx
        .players
        .resolve_player(
            info.clone(),
            Some(&conn),
            |i, c| crate::player::LocalPlayer::new(i, c.expect("conn")),
            |_p, _c| Ok(None),
        )
        .unwrap();
    (res.player, rx)
}

/// 剥掉 VarInt 帧头后解码 ClientBoundPacket。
fn decode_frame_payload(
    frame: &crate::packet::clientbound::SharedFrame,
) -> Option<ClientBoundPacket> {
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

#[test]
fn ban_and_unban_commands() {
    let ctx = ctx();
    assert!(process_command(&ctx, "ban 42"));
    assert!(ctx.bans.is_banned(42));
    assert!(process_command(&ctx, "banlist"));
    // unban / pardon 均为解封
    assert!(process_command(&ctx, "pardon 42"));
    assert!(!ctx.bans.is_banned(42));
    assert!(process_command(&ctx, "ban 42"));
    assert!(ctx.bans.is_banned(42));
    assert!(process_command(&ctx, "unban 42"));
    assert!(!ctx.bans.is_banned(42));
}

#[test]
fn ban_kicks_online_player() {
    let ctx = ctx();
    let (p, _rx) = register(&ctx, 1, "A");
    assert!(process_command(&ctx, "ban 1"));
    let lp = crate::player::local_of(&p).expect("local player");
    assert!(lp.is_kicked(), "在线玩家应被踢出");
}

#[test]
fn banroom_unbanroom_commands() {
    let ctx = ctx();
    assert!(process_command(&ctx, "banroom 42 R1"));
    assert!(ctx.bans.is_room_banned("R1", 42));
    assert!(!ctx.bans.is_room_banned("R2", 42));
    assert!(process_command(&ctx, "unbanroom 42 R1"));
    assert!(!ctx.bans.is_room_banned("R1", 42));
    assert!(process_command(&ctx, "unbanroom 42 R1")); // 已解封
}

#[test]
fn broadcast_sends_system_message_to_all_online() {
    let ctx = ctx();
    let (_p1, mut rx1) = register(&ctx, 1, "A");
    let (_p2, mut rx2) = register(&ctx, 2, "B");
    assert!(process_command(&ctx, "  SaY\t hello  世界\t朋友  "));
    for rx in [&mut rx1, &mut rx2] {
        let msg = rx.try_recv().expect("should receive frame");
        match msg {
            WriterMsg::Shared(frame) => {
                let pkt = decode_frame_payload(&frame).expect("decode");
                match pkt {
                    ClientBoundPacket::Message {
                        message: Message::Chat { user, content },
                        ..
                    } => {
                        assert_eq!(user, 0, "系统广播 user=0");
                        assert_eq!(content, "hello  世界\t朋友");
                    }
                    other => panic!("expected Chat, got {other:?}"),
                }
            }
            other => panic!("expected Shared frame, got {other:?}"),
        }
    }
}

#[test]
fn maxusers_roominfo_commands() {
    let ctx = ctx();
    // 保持房间强引用，避免弱引用失效
    let _room = ctx.rooms.create_room("R1").unwrap();
    assert!(process_command(&ctx, "maxusers R1 4"));
    assert_eq!(ctx.rooms.find_room("R1").unwrap().setting().max_player, 4);
    assert!(process_command(&ctx, "roominfo R1"));
    assert!(process_command(&ctx, "roominfo")); // 缺参数不 panic
}

#[test]
fn room_management_commands_with_players() {
    let ctx = ctx();
    let room = ctx.rooms.create_room("R1").unwrap();
    let (p1, _rx1) = register(&ctx, 1, "A");
    let (p2, _rx2) = register(&ctx, 2, "B");
    room.join(p1, false).unwrap();
    room.join(p2, false).unwrap();
    assert!(room.is_host(1));

    assert!(process_command(&ctx, "lock R1 true"));
    assert!(room.setting().locked);
    assert!(process_command(&ctx, "lock R1 false"));
    assert!(!room.setting().locked);

    assert!(process_command(&ctx, "cycle R1 true"));
    assert!(room.setting().cycle);

    assert!(process_command(&ctx, "sethost R1 2"));
    assert!(room.is_host(2));
    assert!(!room.is_host(1));

    assert!(process_command(&ctx, "nexthost R1 1"));
    // 循环模式对局结束 → 房主转移到指定玩家
    room.commit_select_chart(2, 1, "C".into()).unwrap();
    room.require_start(2).unwrap();
    room.ready(1).unwrap();
    room.commit_played(2, 1, 1.0, true).unwrap();
    let out = room.commit_played(1, 1, 1.0, true).unwrap();
    assert!(out.game_ended);
    assert!(room.is_host(1), "nexthost 应在循环对局结束后生效");
}

#[test]
fn unknown_command_falls_through() {
    let ctx = ctx();
    assert!(!process_command(&ctx, "frobnicate 1 2 3"));
    assert!(process_command(&ctx, "")); // 空行不算 unknown
    assert!(process_command(&ctx, " \t\u{3000}\n"));
    assert!(!process_command(&ctx, "stop")); // 关闭由终端处理
    assert!(!ctx.is_shutdown_requested());
}

#[test]
fn invalid_room_arguments_do_not_change_settings() {
    let ctx = ctx();
    let room = ctx.rooms.create_room("R1").unwrap();
    let initial = room.setting();

    for command in [
        "lock R1 maybe",
        "lock R1",
        "cycle R1 maybe",
        "cycle R1",
        "maxusers R1 invalid",
        "maxusers R1",
    ] {
        assert!(process_command(&ctx, command));
        let setting = room.setting();
        assert_eq!(setting.locked, initial.locked, "{command}");
        assert_eq!(setting.cycle, initial.cycle, "{command}");
        assert_eq!(setting.max_player, initial.max_player, "{command}");
    }

    assert!(process_command(&ctx, "LoCk R1 YeS"));
    assert!(room.setting().locked);
    assert!(process_command(&ctx, "CYCLE R1 oN"));
    assert!(room.setting().cycle);
}
