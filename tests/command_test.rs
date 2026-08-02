//! 控制台命令集成测试：真实网络链路验证。
//!
//! 覆盖：封禁踢出在线玩家 / 重连认证被拒 / 解封后可重登、banroom 阻止进房、
//! 全服广播、房间管理命令（lock/cycle/maxusers/sethost/roomsay/roominfo/nexthost）。

mod common;

use common::{SERIAL, TestClient, auth_token, authenticate, start_mock_phira, start_server};
use phira_mp::command::process_command;
use phira_mp::packet::PacketResult;
use phira_mp::packet::clientbound::ClientBoundPacket;
use phira_mp::packet::message::Message;
use phira_mp::packet::serverbound::ServerBoundPacket;

/// 认证失败返回错误消息。
async fn auth_failure_message(client: &mut TestClient, n: i32) -> String {
    client
        .send(&ServerBoundPacket::Authenticate {
            token: auth_token(n),
            trailer: None,
        })
        .await;
    let p = client
        .recv_until(|p| matches!(p, ClientBoundPacket::Authenticate { .. }))
        .await;
    match p {
        ClientBoundPacket::Authenticate {
            result: PacketResult::Failed(msg),
            ..
        } => msg,
        ClientBoundPacket::Authenticate {
            result: PacketResult::Success(_),
            ..
        } => {
            panic!("auth should have failed")
        }
        _ => unreachable!(),
    }
}

/// 建房并返回 JoinRoom 结果（供后续断言）。
async fn create_room(client: &mut TestClient, room_id: &str) {
    client
        .send(&ServerBoundPacket::CreateRoom {
            room_id: room_id.to_string(),
            trailer: None,
        })
        .await;
    let p = client
        .recv_until(|p| matches!(p, ClientBoundPacket::CreateRoom { .. }))
        .await;
    match p {
        ClientBoundPacket::CreateRoom {
            result: PacketResult::Success(_),
            ..
        } => {}
        ClientBoundPacket::CreateRoom {
            result: PacketResult::Failed(msg),
            ..
        } => {
            panic!("create room failed: {msg}")
        }
        _ => unreachable!(),
    }
}

#[tokio::test]
async fn ban_kicks_online_player_and_blocks_relogin() {
    let _guard = SERIAL.lock().await;
    let phira = start_mock_phira().await;
    let (ctx, addr) = start_server(&phira.addr).await;

    // 玩家 1 登录成功
    let mut a = TestClient::connect(&addr).await;
    authenticate(&mut a, 1).await;

    // ban 1 → 在线玩家被踢出
    assert!(process_command(&ctx, "ban 1"));
    a.expect_closed().await;
    assert!(ctx.bans.is_banned(1));

    // 重连 → 认证被拒（ERROR_BANNED）
    let mut a2 = TestClient::connect(&addr).await;
    let msg = auth_failure_message(&mut a2, 1).await;
    assert!(msg.contains("封禁"), "expected banned message, got {msg}");

    // unban → 可重新登录
    assert!(process_command(&ctx, "unban 1"));
    let mut a3 = TestClient::connect(&addr).await;
    authenticate(&mut a3, 1).await;
}

#[tokio::test]
async fn banroom_blocks_join_until_unban() {
    let _guard = SERIAL.lock().await;
    let phira = start_mock_phira().await;
    let (ctx, addr) = start_server(&phira.addr).await;

    let mut a = TestClient::connect(&addr).await;
    authenticate(&mut a, 1).await;
    create_room(&mut a, "R1").await;

    let mut b = TestClient::connect(&addr).await;
    authenticate(&mut b, 2).await;

    // banroom 2 R1 → B 进房被拒
    assert!(process_command(&ctx, "banroom 2 R1"));
    b.send(&ServerBoundPacket::JoinRoom {
        room_id: "R1".into(),
        monitor: false,
        trailer: None,
    })
    .await;
    let p = b
        .recv_until(|p| matches!(p, ClientBoundPacket::JoinRoom { .. }))
        .await;
    match p {
        ClientBoundPacket::JoinRoom {
            result: PacketResult::Failed(msg),
            ..
        } => {
            assert!(
                msg.contains("禁止"),
                "expected banned-from-room message, got {msg}"
            );
        }
        ClientBoundPacket::JoinRoom {
            result: PacketResult::Success(_),
            ..
        } => {
            panic!("join should have failed")
        }
        _ => unreachable!(),
    }

    // unbanroom → B 可进房
    assert!(process_command(&ctx, "unbanroom 2 R1"));
    b.send(&ServerBoundPacket::JoinRoom {
        room_id: "R1".into(),
        monitor: false,
        trailer: None,
    })
    .await;
    let p = b
        .recv_until(|p| matches!(p, ClientBoundPacket::JoinRoom { .. }))
        .await;
    assert!(
        matches!(
            p,
            ClientBoundPacket::JoinRoom {
                result: PacketResult::Success(_),
                ..
            }
        ),
        "join should succeed after unban"
    );
}

#[tokio::test]
async fn broadcast_and_room_management_commands() {
    let _guard = SERIAL.lock().await;
    let phira = start_mock_phira().await;
    let (ctx, addr) = start_server(&phira.addr).await;

    let mut a = TestClient::connect(&addr).await;
    authenticate(&mut a, 1).await;
    let mut b = TestClient::connect(&addr).await;
    authenticate(&mut b, 2).await;

    // 全服广播：双方都收到系统消息（user=0）
    assert!(process_command(&ctx, "broadcast hello all"));
    let p = a
        .recv_until(|p| matches!(p, ClientBoundPacket::Message { message: Message::Chat { user: 0, content }, .. } if content == "hello all"))
        .await;
    assert!(matches!(
        p,
        ClientBoundPacket::Message {
            message: Message::Chat { user: 0, .. },
            ..
        }
    ));
    let p = b
        .recv_until(|p| matches!(p, ClientBoundPacket::Message { message: Message::Chat { user: 0, content }, .. } if content == "hello all"))
        .await;
    assert!(matches!(
        p,
        ClientBoundPacket::Message {
            message: Message::Chat { user: 0, .. },
            ..
        }
    ));

    // A 建房，B 加入
    create_room(&mut a, "R1").await;
    b.send(&ServerBoundPacket::JoinRoom {
        room_id: "R1".into(),
        monitor: false,
        trailer: None,
    })
    .await;
    b.recv_until(|p| matches!(p, ClientBoundPacket::JoinRoom { .. }))
        .await;

    let room = ctx.rooms.find_room("R1").expect("room R1");
    assert!(room.is_host(1));

    // lock / cycle
    assert!(process_command(&ctx, "lock R1 true"));
    assert!(room.setting().locked);
    b.recv_until(|p| {
        matches!(
            p,
            ClientBoundPacket::Message {
                message: Message::LockRoom { lock: true },
                ..
            }
        )
    })
    .await;
    assert!(process_command(&ctx, "cycle R1 true"));
    assert!(room.setting().cycle);
    b.recv_until(|p| {
        matches!(
            p,
            ClientBoundPacket::Message {
                message: Message::CycleRoom { cycle: true },
                ..
            }
        )
    })
    .await;

    // maxusers
    assert!(process_command(&ctx, "maxusers R1 3"));
    assert_eq!(room.setting().max_player, 3);

    // sethost → 立即转移房主
    assert!(process_command(&ctx, "sethost R1 2"));
    assert!(room.is_host(2));
    b.recv_until(|p| matches!(p, ClientBoundPacket::ChangeHost { is_host: true, .. }))
        .await;

    // roomsay（绕过 chat 开关；user=0 系统消息）
    assert!(process_command(&ctx, "roomsay R1 hi room"));
    let p = a
        .recv_until(|p| matches!(p, ClientBoundPacket::Message { message: Message::Chat { user: 0, content }, .. } if content == "hi room"))
        .await;
    assert!(matches!(
        p,
        ClientBoundPacket::Message {
            message: Message::Chat { user: 0, .. },
            ..
        }
    ));

    // roominfo / nexthost 不 panic
    assert!(process_command(&ctx, "roominfo R1"));
    assert!(process_command(&ctx, "nexthost R1 1"));

    // 参数错误不 panic
    assert!(process_command(&ctx, "ban"));
    assert!(process_command(&ctx, "lock R1 maybe"));
    assert!(process_command(&ctx, "maxusers"));
    assert!(process_command(&ctx, "roomsay R1"));

    // 未注册的未知命令走扩展事件 → unknown（返回 false）
    assert!(!process_command(&ctx, "frobnicate"));
}
