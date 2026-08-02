//! 端到端极端/边界协议测试：真实 TCP + Mock Phira。
//!
//! 覆盖：超长字段（token/room_id/chat）解码断连、未知包 ID、
//! 未认证发包踢出、重复认证踢出、大厅阶段发房间包踢出、
//! 房间阶段发大厅包踢出、空房间 ID、重复建房、建房冷却、
//! 加入不存在房间、锁定房间拒绝、非房主权限、未选曲开局、
//! monitor 转发、坏 token 认证失败、房间内 Ping。

mod common;

use bytes::BufMut;
use common::{SERIAL, TestClient, authenticate, start_mock_phira, start_server};
use phira_mp::packet::PacketResult;
use phira_mp::packet::clientbound::{ClientBoundPacket, JoinRoomData};
use phira_mp::packet::data::{CompactPos, TouchFrame, TouchPoint};
use phira_mp::packet::serverbound::ServerBoundPacket;
use phira_mp::packet::state::GameState;
use std::time::Duration;

/// 构造「id + 字段体」的完整帧字节。
fn raw_frame(id: u8, body: &[u8]) -> bytes::BytesMut {
    let mut payload = vec![id];
    payload.extend_from_slice(body);
    phira_mp::frame::encode_frame(&payload)
}

fn varint_buf(v: i32) -> bytes::BytesMut {
    let mut b = bytes::BytesMut::new();
    phira_mp::bytes::write_varint(&mut b, v);
    b
}

// ---------------- 解码层断连（协议违规） ----------------

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn oversized_token_disconnects() {
    let _guard = SERIAL.lock().await;
    let phira = start_mock_phira().await;
    let (_ctx, addr) = start_server(&phira.addr).await;

    // token 33 字节 > 协议上限 32 → 帧解码失败 → 断连
    let mut c = TestClient::connect(&addr).await;
    let mut body = bytes::BytesMut::new();
    body.put_u8(0x01);
    body.extend_from_slice(&varint_buf(33));
    body.extend_from_slice(&[b'x'; 33]);
    c.send_raw(&phira_mp::frame::encode_frame(&body)).await;
    c.expect_closed().await;
    let _ = phira.shutdown.send(());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn oversized_room_id_disconnects() {
    let _guard = SERIAL.lock().await;
    let phira = start_mock_phira().await;
    let (_ctx, addr) = start_server(&phira.addr).await;

    // room_id 21 字节 > 20 → 解码失败断连
    let mut c = TestClient::connect(&addr).await;
    let mut body = bytes::BytesMut::new();
    body.put_u8(0x05);
    body.extend_from_slice(&varint_buf(21));
    body.extend_from_slice(&[b'r'; 21]);
    c.send_raw(&phira_mp::frame::encode_frame(&body)).await;
    c.expect_closed().await;
    let _ = phira.shutdown.send(());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn oversized_chat_disconnects() {
    let _guard = SERIAL.lock().await;
    let phira = start_mock_phira().await;
    let (_ctx, addr) = start_server(&phira.addr).await;

    // 先正常认证（认证阶段 chat 包会直接被踢，必须先进房间）
    let mut c = TestClient::connect(&addr).await;
    authenticate(&mut c, 1).await;
    c.send(&ServerBoundPacket::CreateRoom {
        room_id: "OC".into(),
        trailer: None,
    })
    .await;
    c.recv_until(|p| {
        matches!(
            p,
            ClientBoundPacket::CreateRoom {
                result: PacketResult::Success(()),
                ..
            }
        )
    })
    .await;

    // chat 201 字节 > 200 → 解码失败断连
    let mut body = bytes::BytesMut::new();
    body.put_u8(0x02);
    body.extend_from_slice(&varint_buf(201));
    body.extend_from_slice(&[b'c'; 201]);
    c.send_raw(&phira_mp::frame::encode_frame(&body)).await;
    c.expect_closed().await;
    let _ = phira.shutdown.send(());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn unknown_packet_id_disconnects() {
    let _guard = SERIAL.lock().await;
    let phira = start_mock_phira().await;
    let (_ctx, addr) = start_server(&phira.addr).await;

    let mut c = TestClient::connect(&addr).await;
    c.send_raw(&raw_frame(0x7F, &[])).await;
    c.expect_closed().await;
    let _ = phira.shutdown.send(());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn handshake_wrong_version_disconnects() {
    let _guard = SERIAL.lock().await;
    let phira = start_mock_phira().await;
    let (_ctx, addr) = start_server(&phira.addr).await;

    // 握手版本 0x02 ≠ 0x01 → 直接断开
    let mut stream = tokio::net::TcpStream::connect(&addr).await.unwrap();
    use tokio::io::AsyncWriteExt;
    stream.write_all(&[0x02]).await.unwrap();
    use tokio::io::AsyncReadExt;
    let mut tmp = [0u8; 16];
    let n = tokio::time::timeout(Duration::from_secs(3), stream.read(&mut tmp))
        .await
        .expect("read timeout")
        .unwrap_or(1);
    assert_eq!(n, 0, "错误版本应断开");
    let _ = phira.shutdown.send(());
}

// ---------------- 阶段机踢出 ----------------

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn unauthenticated_packet_kicks() {
    let _guard = SERIAL.lock().await;
    let phira = start_mock_phira().await;
    let (_ctx, addr) = start_server(&phira.addr).await;

    // 未认证发 CreateRoom → 关闭（不发任何包）
    let mut c = TestClient::connect(&addr).await;
    c.send(&ServerBoundPacket::CreateRoom {
        room_id: "X".into(),
        trailer: None,
    })
    .await;
    c.expect_closed().await;
    let _ = phira.shutdown.send(());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn duplicate_authenticate_kicks() {
    let _guard = SERIAL.lock().await;
    let phira = start_mock_phira().await;
    let (_ctx, addr) = start_server(&phira.addr).await;

    let mut c = TestClient::connect(&addr).await;
    authenticate(&mut c, 1).await;
    // 认证后再认证 → PlayHandler 阶段无 Authenticate 处理 → 踢
    c.send(&ServerBoundPacket::Authenticate {
        token: common::auth_token(1),
        trailer: None,
    })
    .await;
    c.expect_closed().await;
    let _ = phira.shutdown.send(());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn lobby_stage_room_packet_kicks() {
    let _guard = SERIAL.lock().await;
    let phira = start_mock_phira().await;
    let (_ctx, addr) = start_server(&phira.addr).await;

    let mut c = TestClient::connect(&addr).await;
    authenticate(&mut c, 1).await;
    // 大厅阶段发房间内操作包（Chat/LeaveRoom）→ 踢
    c.send(&ServerBoundPacket::Chat {
        message: "hi".into(),
        trailer: None,
    })
    .await;
    c.expect_closed().await;
    let _ = phira.shutdown.send(());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn room_stage_lobby_packet_kicks() {
    let _guard = SERIAL.lock().await;
    let phira = start_mock_phira().await;
    let (_ctx, addr) = start_server(&phira.addr).await;

    let mut c = TestClient::connect(&addr).await;
    authenticate(&mut c, 1).await;
    c.send(&ServerBoundPacket::CreateRoom {
        room_id: "RK".into(),
        trailer: None,
    })
    .await;
    c.recv_until(|p| {
        matches!(
            p,
            ClientBoundPacket::CreateRoom {
                result: PacketResult::Success(()),
                ..
            }
        )
    })
    .await;
    // 房间内发 CreateRoom/JoinRoom/Authenticate → 踢
    c.send(&ServerBoundPacket::CreateRoom {
        room_id: "ANOTHER".into(),
        trailer: None,
    })
    .await;
    c.expect_closed().await;
    let _ = phira.shutdown.send(());
}

// ---------------- 建房边界 ----------------

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn create_room_empty_id_fails() {
    let _guard = SERIAL.lock().await;
    let phira = start_mock_phira().await;
    let (_ctx, addr) = start_server(&phira.addr).await;

    let mut c = TestClient::connect(&addr).await;
    authenticate(&mut c, 1).await;
    c.send(&ServerBoundPacket::CreateRoom {
        room_id: String::new(),
        trailer: None,
    })
    .await;
    c.recv_until(|p| {
        matches!(
            p,
            ClientBoundPacket::CreateRoom {
                result: PacketResult::Failed(_),
                ..
            }
        )
    })
    .await;
    // 连接保持
    c.send(&ServerBoundPacket::Ping).await;
    c.expect_pong().await;
    let _ = phira.shutdown.send(());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn create_duplicate_room_id_fails() {
    let _guard = SERIAL.lock().await;
    let phira = start_mock_phira().await;
    let (_ctx, addr) = start_server(&phira.addr).await;

    let mut c1 = TestClient::connect(&addr).await;
    authenticate(&mut c1, 1).await;
    c1.send(&ServerBoundPacket::CreateRoom {
        room_id: "DUP".into(),
        trailer: None,
    })
    .await;
    c1.recv_until(|p| {
        matches!(
            p,
            ClientBoundPacket::CreateRoom {
                result: PacketResult::Success(()),
                ..
            }
        )
    })
    .await;

    let mut c2 = TestClient::connect(&addr).await;
    authenticate(&mut c2, 2).await;
    c2.send(&ServerBoundPacket::CreateRoom {
        room_id: "DUP".into(),
        trailer: None,
    })
    .await;
    c2.recv_until(|p| {
        matches!(
            p,
            ClientBoundPacket::CreateRoom {
                result: PacketResult::Failed(_),
                ..
            }
        )
    })
    .await;
    // c2 仍可正常使用
    c2.send(&ServerBoundPacket::Ping).await;
    c2.expect_pong().await;
    let _ = phira.shutdown.send(());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn create_room_after_leave_succeeds() {
    let _guard = SERIAL.lock().await;
    let phira = start_mock_phira().await;
    let (_ctx, addr) = start_server(&phira.addr).await;

    let mut c = TestClient::connect(&addr).await;
    authenticate(&mut c, 1).await;
    c.send(&ServerBoundPacket::CreateRoom {
        room_id: "A1".into(),
        trailer: None,
    })
    .await;
    c.recv_until(|p| {
        matches!(
            p,
            ClientBoundPacket::CreateRoom {
                result: PacketResult::Success(()),
                ..
            }
        )
    })
    .await;
    // 离房回大厅（fallback PlayHandler 实例冷却已过期）
    c.send(&ServerBoundPacket::LeaveRoom { trailer: None })
        .await;
    c.recv_until(|p| {
        matches!(
            p,
            ClientBoundPacket::LeaveRoom {
                result: PacketResult::Success(()),
                ..
            }
        )
    })
    .await;
    // 立即再次建房 → 成功（每个 PlayHandler 实例的建房冷却独立）
    c.send(&ServerBoundPacket::CreateRoom {
        room_id: "A2".into(),
        trailer: None,
    })
    .await;
    c.recv_until(|p| {
        matches!(
            p,
            ClientBoundPacket::CreateRoom {
                result: PacketResult::Success(()),
                ..
            }
        )
    })
    .await;
    let _ = phira.shutdown.send(());
}

// ---------------- 入房边界 ----------------

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn join_nonexistent_room_fails() {
    let _guard = SERIAL.lock().await;
    let phira = start_mock_phira().await;
    let (_ctx, addr) = start_server(&phira.addr).await;

    let mut c = TestClient::connect(&addr).await;
    authenticate(&mut c, 1).await;
    c.send(&ServerBoundPacket::JoinRoom {
        room_id: "NOPE".into(),
        monitor: false,
        trailer: None,
    })
    .await;
    c.recv_until(|p| {
        matches!(
            p,
            ClientBoundPacket::JoinRoom {
                result: PacketResult::Failed(_),
                ..
            }
        )
    })
    .await;
    c.send(&ServerBoundPacket::Ping).await;
    c.expect_pong().await;
    let _ = phira.shutdown.send(());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn locked_room_rejects_join() {
    let _guard = SERIAL.lock().await;
    let phira = start_mock_phira().await;
    let (_ctx, addr) = start_server(&phira.addr).await;

    let mut c1 = TestClient::connect(&addr).await;
    authenticate(&mut c1, 1).await;
    c1.send(&ServerBoundPacket::CreateRoom {
        room_id: "LK".into(),
        trailer: None,
    })
    .await;
    c1.recv_until(|p| {
        matches!(
            p,
            ClientBoundPacket::CreateRoom {
                result: PacketResult::Success(()),
                ..
            }
        )
    })
    .await;
    c1.send(&ServerBoundPacket::LockRoom {
        lock: true,
        trailer: None,
    })
    .await;
    c1.recv_until(|p| {
        matches!(
            p,
            ClientBoundPacket::LockRoom {
                result: PacketResult::Success(()),
                ..
            }
        )
    })
    .await;

    let mut c2 = TestClient::connect(&addr).await;
    authenticate(&mut c2, 2).await;
    c2.send(&ServerBoundPacket::JoinRoom {
        room_id: "LK".into(),
        monitor: false,
        trailer: None,
    })
    .await;
    c2.recv_until(|p| {
        matches!(
            p,
            ClientBoundPacket::JoinRoom {
                result: PacketResult::Failed(_),
                ..
            }
        )
    })
    .await;
    // monitor 可进
    c2.send(&ServerBoundPacket::JoinRoom {
        room_id: "LK".into(),
        monitor: true,
        trailer: None,
    })
    .await;
    let p = c2
        .recv_until(|p| {
            matches!(
                p,
                ClientBoundPacket::JoinRoom {
                    result: PacketResult::Success(_),
                    ..
                }
            )
        })
        .await;
    if let ClientBoundPacket::JoinRoom {
        result: PacketResult::Success(JoinRoomData { users, .. }),
        ..
    } = p
    {
        assert!(users.iter().any(|u| u.user_id == 2 && u.monitor));
    } else {
        panic!("monitor join should succeed");
    }
    let _ = phira.shutdown.send(());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn room_stage_join_packet_kicks() {
    let _guard = SERIAL.lock().await;
    let phira = start_mock_phira().await;
    let (_ctx, addr) = start_server(&phira.addr).await;

    let mut c1 = TestClient::connect(&addr).await;
    authenticate(&mut c1, 1).await;
    c1.send(&ServerBoundPacket::CreateRoom {
        room_id: "AG".into(),
        trailer: None,
    })
    .await;
    c1.recv_until(|p| {
        matches!(
            p,
            ClientBoundPacket::CreateRoom {
                result: PacketResult::Success(()),
                ..
            }
        )
    })
    .await;
    // 房间内再发 JoinRoom → RoomHandler 阶段大厅包 → 踢
    c1.send(&ServerBoundPacket::JoinRoom {
        room_id: "AG".into(),
        monitor: false,
        trailer: None,
    })
    .await;
    c1.expect_closed().await;
    let _ = phira.shutdown.send(());
}

// ---------------- 权限 ----------------

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn non_host_operations_denied() {
    let _guard = SERIAL.lock().await;
    let phira = start_mock_phira().await;
    let (_ctx, addr) = start_server(&phira.addr).await;

    let mut c1 = TestClient::connect(&addr).await;
    authenticate(&mut c1, 1).await;
    c1.send(&ServerBoundPacket::CreateRoom {
        room_id: "NH".into(),
        trailer: None,
    })
    .await;
    c1.recv_until(|p| {
        matches!(
            p,
            ClientBoundPacket::CreateRoom {
                result: PacketResult::Success(()),
                ..
            }
        )
    })
    .await;

    let mut c2 = TestClient::connect(&addr).await;
    authenticate(&mut c2, 2).await;
    c2.send(&ServerBoundPacket::JoinRoom {
        room_id: "NH".into(),
        monitor: false,
        trailer: None,
    })
    .await;
    c2.recv_until(|p| {
        matches!(
            p,
            ClientBoundPacket::JoinRoom {
                result: PacketResult::Success(_),
                ..
            }
        )
    })
    .await;

    // 非房主选曲/开始 → 权限拒绝
    c2.send(&ServerBoundPacket::SelectChart {
        id: 42,
        trailer: None,
    })
    .await;
    c2.recv_until(|p| {
        matches!(
            p,
            ClientBoundPacket::SelectChart {
                result: PacketResult::Failed(_),
                ..
            }
        )
    })
    .await;
    c2.send(&ServerBoundPacket::RequestStart { trailer: None })
        .await;
    c2.recv_until(|p| {
        matches!(
            p,
            ClientBoundPacket::RequestStart {
                result: PacketResult::Failed(_),
                ..
            }
        )
    })
    .await;
    c2.send(&ServerBoundPacket::LockRoom {
        lock: true,
        trailer: None,
    })
    .await;
    c2.recv_until(|p| {
        matches!(
            p,
            ClientBoundPacket::LockRoom {
                result: PacketResult::Failed(_),
                ..
            }
        )
    })
    .await;

    // 房主选曲成功
    c1.send(&ServerBoundPacket::SelectChart {
        id: 42,
        trailer: None,
    })
    .await;
    c1.recv_until(|p| {
        matches!(
            p,
            ClientBoundPacket::SelectChart {
                result: PacketResult::Success(()),
                ..
            }
        )
    })
    .await;
    c2.recv_until(|p| {
        matches!(
            p,
            ClientBoundPacket::ChangeState {
                game_state: GameState::SelectChart { chart_id: Some(42) },
                ..
            }
        )
    })
    .await;
    let _ = phira.shutdown.send(());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn request_start_without_chart_solo_plays() {
    let _guard = SERIAL.lock().await;
    let phira = start_mock_phira().await;
    let (_ctx, addr) = start_server(&phira.addr).await;

    let mut c1 = TestClient::connect(&addr).await;
    authenticate(&mut c1, 1).await;
    c1.send(&ServerBoundPacket::CreateRoom {
        room_id: "NC".into(),
        trailer: None,
    })
    .await;
    c1.recv_until(|p| {
        matches!(
            p,
            ClientBoundPacket::CreateRoom {
                result: PacketResult::Success(()),
                ..
            }
        )
    })
    .await;
    // 未选曲直接开始（服务端不校验 chart_id）
    c1.send(&ServerBoundPacket::RequestStart { trailer: None })
        .await;
    c1.recv_until(|p| {
        matches!(
            p,
            ClientBoundPacket::RequestStart {
                result: PacketResult::Success(()),
                ..
            }
        )
    })
    .await;
    c1.recv_until(|p| {
        matches!(
            p,
            ClientBoundPacket::ChangeState {
                game_state: GameState::Playing,
                ..
            }
        )
    })
    .await;
    let _ = phira.shutdown.send(());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn ready_before_start_fails() {
    let _guard = SERIAL.lock().await;
    let phira = start_mock_phira().await;
    let (_ctx, addr) = start_server(&phira.addr).await;

    let mut c1 = TestClient::connect(&addr).await;
    authenticate(&mut c1, 1).await;
    c1.send(&ServerBoundPacket::CreateRoom {
        room_id: "RB".into(),
        trailer: None,
    })
    .await;
    c1.recv_until(|p| {
        matches!(
            p,
            ClientBoundPacket::CreateRoom {
                result: PacketResult::Success(()),
                ..
            }
        )
    })
    .await;
    // SelectChart 状态下 ready → 非法状态
    c1.send(&ServerBoundPacket::Ready { trailer: None }).await;
    c1.recv_until(|p| {
        matches!(
            p,
            ClientBoundPacket::Ready {
                result: PacketResult::Failed(_),
                ..
            }
        )
    })
    .await;
    let _ = phira.shutdown.send(());
}

// ---------------- 认证失败 ----------------

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn bad_token_auth_fails_and_closes() {
    let _guard = SERIAL.lock().await;
    let phira = start_mock_phira().await;
    let (_ctx, addr) = start_server(&phira.addr).await;

    let mut c = TestClient::connect(&addr).await;
    c.send(&ServerBoundPacket::Authenticate {
        token: "banned-token".into(),
        trailer: None,
    })
    .await;
    // 先收到 Authenticate Failed，随后连接关闭
    c.recv_until(|p| {
        matches!(
            p,
            ClientBoundPacket::Authenticate {
                result: PacketResult::Failed(_),
                ..
            }
        )
    })
    .await;
    c.expect_closed().await;
    let _ = phira.shutdown.send(());
}

// ---------------- monitor 转发 ----------------

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn monitor_receives_touch_forward() {
    let _guard = SERIAL.lock().await;
    let phira = start_mock_phira().await;
    let (_ctx, addr) = start_server(&phira.addr).await;

    let mut c1 = TestClient::connect(&addr).await;
    authenticate(&mut c1, 1).await;
    c1.send(&ServerBoundPacket::CreateRoom {
        room_id: "MT".into(),
        trailer: None,
    })
    .await;
    c1.recv_until(|p| {
        matches!(
            p,
            ClientBoundPacket::CreateRoom {
                result: PacketResult::Success(()),
                ..
            }
        )
    })
    .await;
    c1.send(&ServerBoundPacket::SelectChart {
        id: 42,
        trailer: None,
    })
    .await;
    c1.recv_until(|p| {
        matches!(
            p,
            ClientBoundPacket::SelectChart {
                result: PacketResult::Success(()),
                ..
            }
        )
    })
    .await;

    // monitor 加入（含 player 共 2 人 → WaitForReady）
    let mut c2 = TestClient::connect(&addr).await;
    authenticate(&mut c2, 2).await;
    c2.send(&ServerBoundPacket::JoinRoom {
        room_id: "MT".into(),
        monitor: true,
        trailer: None,
    })
    .await;
    c2.recv_until(|p| {
        matches!(
            p,
            ClientBoundPacket::JoinRoom {
                result: PacketResult::Success(_),
                ..
            }
        )
    })
    .await;
    c1.recv_until(
        |p| matches!(p, ClientBoundPacket::OnJoinRoom { user_profile, .. } if user_profile.monitor),
    )
    .await;

    c1.send(&ServerBoundPacket::RequestStart { trailer: None })
        .await;
    c1.recv_until(|p| {
        matches!(
            p,
            ClientBoundPacket::RequestStart {
                result: PacketResult::Success(()),
                ..
            }
        )
    })
    .await;
    c2.send(&ServerBoundPacket::Ready { trailer: None }).await;
    c2.recv_until(|p| {
        matches!(
            p,
            ClientBoundPacket::Ready {
                result: PacketResult::Success(()),
                ..
            }
        )
    })
    .await;
    c1.recv_until(|p| {
        matches!(
            p,
            ClientBoundPacket::ChangeState {
                game_state: GameState::Playing,
                ..
            }
        )
    })
    .await;
    c2.recv_until(|p| {
        matches!(
            p,
            ClientBoundPacket::ChangeState {
                game_state: GameState::Playing,
                ..
            }
        )
    })
    .await;

    // 玩家 touch → monitor 收到转发
    c1.send(&ServerBoundPacket::Touches {
        frames: vec![TouchFrame {
            time: 1.0,
            points: vec![TouchPoint {
                id: 0,
                pos: CompactPos::from_f32(0.5, 0.5),
            }],
        }],
        trailer: None,
    })
    .await;
    let p = c2
        .recv_until(|p| {
            matches!(
                p,
                ClientBoundPacket::Touches {
                    from_player_id: 1,
                    ..
                }
            )
        })
        .await;
    if let ClientBoundPacket::Touches {
        from_player_id,
        frames,
        ..
    } = p
    {
        assert_eq!(from_player_id, 1);
        assert_eq!(frames.len(), 1);
    }
    let _ = phira.shutdown.send(());
}

// ---------------- 房间内 Ping / 聊天 ----------------

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn ping_and_chat_inside_room() {
    let _guard = SERIAL.lock().await;
    let phira = start_mock_phira().await;
    let (_ctx, addr) = start_server(&phira.addr).await;

    let mut c1 = TestClient::connect(&addr).await;
    authenticate(&mut c1, 1).await;
    c1.send(&ServerBoundPacket::CreateRoom {
        room_id: "PC".into(),
        trailer: None,
    })
    .await;
    c1.recv_until(|p| {
        matches!(
            p,
            ClientBoundPacket::CreateRoom {
                result: PacketResult::Success(()),
                ..
            }
        )
    })
    .await;

    c1.send(&ServerBoundPacket::Ping).await;
    c1.expect_pong().await;
    c1.send(&ServerBoundPacket::Chat {
        message: "hi".into(),
        trailer: None,
    })
    .await;
    c1.recv_until(|p| {
        matches!(
            p,
            ClientBoundPacket::Chat {
                result: PacketResult::Success(()),
                ..
            }
        )
    })
    .await;
    c1.recv_until(|p| {
        matches!(
            p,
            ClientBoundPacket::Message {
                message: phira_mp::packet::message::Message::Chat { user: 1, .. },
                ..
            }
        )
    })
    .await;
    let _ = phira.shutdown.send(());
}
