//! 端到端冒烟测试：本地 mock Phira API + 真实 TCP 客户端。
//!
//! 覆盖：握手 → 认证 → 建房 → 入房 → 选谱 → ready 开局 → played → 对局结束 →
//! 断线重连（会话恢复）→ 认证失败/踢出语义。

use phira_mp::packet::PacketResult;
use phira_mp::packet::clientbound::{AuthenticateData, ClientBoundPacket, JoinRoomData};
use phira_mp::packet::data::{CompactPos, JudgeEvent, Judgement, TouchFrame, TouchPoint};
use phira_mp::packet::message::Message;
use phira_mp::packet::serverbound::ServerBoundPacket;
use phira_mp::packet::state::GameState;
use phira_mp::server::{ServerArgs, run};
use serde_json::json;
use std::sync::Arc;
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};

/// 全局串行锁：GLOBAL_CTX 是进程级单例，冒烟测试必须串行执行。
/// 用 tokio Mutex（可跨 .await 持有，std Mutex 会阻塞 executor 线程）。
static SERIAL: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

// ---------------- Mock Phira API ----------------

struct MockPhira {
    addr: String,
    shutdown: tokio::sync::oneshot::Sender<()>,
}

async fn start_mock_phira() -> MockPhira {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap().to_string();
    let (tx, mut rx) = tokio::sync::oneshot::channel::<()>();

    tokio::spawn(async move {
        loop {
            tokio::select! {
                accept = listener.accept() => {
                    let Ok((mut stream, _)) = accept else { continue };
                    tokio::spawn(async move {
                        let mut buf = vec![0u8; 8192];
                        let n = stream.read(&mut buf).await.unwrap_or(0);
                        let req = String::from_utf8_lossy(&buf[..n]).to_string();
                        let path = req.split_whitespace().nth(1).unwrap_or("/").to_string();
                        let token = req
                            .lines()
                            .find_map(|l| {
                                let lower = l.to_ascii_lowercase();
                                lower
                                    .strip_prefix("authorization: bearer ")
                                    .map(|_| l["Authorization: Bearer ".len()..].trim().to_string())
                            });
                        let (status, body) = mock_response(&path, token.as_deref());
                        let resp = format!(
                            "HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                            body.len()
                        );
                        let _ = stream.write_all(resp.as_bytes()).await;
                        let _ = stream.shutdown().await;
                    });
                }
                _ = &mut rx => break,
            }
        }
    });

    MockPhira { addr, shutdown: tx }
}

fn mock_response(path: &str, token: Option<&str>) -> (&'static str, String) {
    let seg = path.trim_start_matches('/');
    if seg == "me" {
        let id: i32 = token
            .and_then(|t| t.strip_prefix("test-token-"))
            .and_then(|s| s.parse().ok())
            .unwrap_or(1);
        return (
            "200 OK",
            json!({
                "id": id, "name": format!("Tester{id}"), "language": "zh-CN",
                "rks": 15.0, "banned": false, "loginBanned": false
            })
            .to_string(),
        );
    }
    if let Some(rest) = seg.strip_prefix("user/") {
        let id: i32 = rest.parse().unwrap_or(0);
        return (
            "200 OK",
            json!({
                "id": id, "name": format!("User{id}"), "language": "zh-CN",
                "rks": 14.0, "banned": false, "loginBanned": false
            })
            .to_string(),
        );
    }
    if let Some(rest) = seg.strip_prefix("chart/") {
        let id: i32 = rest.parse().unwrap_or(0);
        return (
            "200 OK",
            json!({
                "id": id, "name": format!("Chart{id}"), "level": "AT",
                "difficulty": 16.4, "charter": "charter", "composer": "composer",
                "ranked": true, "uploader": 1
            })
            .to_string(),
        );
    }
    if let Some(rest) = seg.strip_prefix("record/") {
        let id: i32 = rest.parse().unwrap_or(0);
        return (
            "200 OK",
            json!({
                "id": id, "player": 1, "chart": 42, "score": 998765, "accuracy": 99.87,
                "perfect": 900, "good": 5, "bad": 1, "miss": 0,
                "fullCombo": true, "maxCombo": 906, "speed": 1.0
            })
            .to_string(),
        );
    }
    ("404 Not Found", "{}".to_string())
}

// ---------------- 测试客户端 ----------------

struct TestClient {
    stream: TcpStream,
    rbuf: Vec<u8>,
    inbox: Vec<ClientBoundPacket>,
}

impl TestClient {
    async fn connect(addr: &str) -> Self {
        Self::connect_with_prefix(addr, &[]).await
    }

    /// 连接并在握手字节前发送任意前缀（用于 PROXY 协议头）。
    async fn connect_with_prefix(addr: &str, prefix: &[u8]) -> Self {
        let mut stream = TcpStream::connect(addr).await.unwrap();
        stream.set_nodelay(true).unwrap();
        if !prefix.is_empty() {
            stream.write_all(prefix).await.unwrap();
        }
        stream.write_all(&[0x01]).await.unwrap(); // 握手
        TestClient {
            stream,
            rbuf: Vec::new(),
            inbox: Vec::new(),
        }
    }

    async fn send(&mut self, packet: &ServerBoundPacket) {
        use phira_mp::bytes::Encode;
        let mut body = bytes::BytesMut::new();
        packet.encode(&mut body);
        let frame = phira_mp::frame::encode_frame(&body);
        self.stream.write_all(&frame).await.unwrap();
    }

    async fn recv(&mut self) -> ClientBoundPacket {
        let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
        loop {
            if let Some(payload) = take_one_frame(&mut self.rbuf) {
                return ClientBoundPacket::decode_frame(&payload).expect("decode clientbound");
            }
            if tokio::time::Instant::now() > deadline {
                panic!(
                    "recv timeout; buffered={} bytes={:02x?}, inbox ids={:?}",
                    self.rbuf.len(),
                    &self.rbuf[..self.rbuf.len().min(64)],
                    self.inbox.iter().map(|p| p.id()).collect::<Vec<_>>()
                );
            }
            let mut tmp = [0u8; 4096];
            let n = tokio::time::timeout_at(deadline, self.stream.read(&mut tmp))
                .await
                .expect("read timeout")
                .expect("read error");
            if n == 0 {
                panic!(
                    "connection closed; inbox ids={:?}",
                    self.inbox.iter().map(|p| p.id()).collect::<Vec<_>>()
                );
            }
            self.rbuf.extend_from_slice(&tmp[..n]);
        }
    }

    async fn recv_until(&mut self, pred: impl Fn(&ClientBoundPacket) -> bool) -> ClientBoundPacket {
        // 先消费暂存的 inbox（之前被跳过的包）
        if let Some(pos) = self.inbox.iter().position(&pred) {
            return self.inbox.remove(pos);
        }
        for _ in 0..50 {
            let p = self.recv().await;
            if pred(&p) {
                return p;
            }
            self.inbox.push(p);
        }
        panic!(
            "expected packet not received; inbox={:?}",
            self.inbox.iter().map(|p| p.id()).collect::<Vec<_>>()
        );
    }

    async fn recv_message(&mut self, pred: impl Fn(&Message) -> bool) -> Message {
        let p = self
            .recv_until(
                |p| matches!(p, ClientBoundPacket::Message { message, .. } if pred(message)),
            )
            .await;
        match p {
            ClientBoundPacket::Message { message, .. } => message,
            _ => unreachable!(),
        }
    }

    async fn expect_pong(&mut self) {
        let p = self.recv().await;
        assert!(
            matches!(p, ClientBoundPacket::Pong),
            "expected Pong, got id={}",
            p.id()
        );
    }

    async fn expect_closed(&mut self) {
        let mut tmp = [0u8; 64];
        let n = tokio::time::timeout(Duration::from_secs(6), self.stream.read(&mut tmp))
            .await
            .expect("close timeout")
            .unwrap_or(1);
        assert_eq!(n, 0, "expected connection closed");
    }
}

/// 从缓冲取出一个完整帧的 payload（就地消费）。与服务端 FrameDecoder 语义一致。
fn take_one_frame(buf: &mut Vec<u8>) -> Option<Vec<u8>> {
    // 跳过前导 NUL
    let nul_end = buf.iter().position(|&b| b != 0).unwrap_or(buf.len());
    if nul_end == buf.len() {
        buf.clear();
        return None;
    }
    // varint
    let mut i = nul_end;
    let mut len: i64 = 0;
    let mut shift = 0u32;
    let mut got = false;
    for _ in 0..5 {
        if i >= buf.len() {
            return None;
        }
        let b = buf[i];
        i += 1;
        len |= ((b & 0x7F) as i64) << shift;
        if b & 0x80 == 0 {
            got = true;
            break;
        }
        shift += 7;
    }
    if !got || len < 0 {
        return None;
    }
    let payload_len = len as usize;
    if buf.len() < i + payload_len {
        return None;
    }
    // 消费 NUL 前缀 + varint + payload
    buf.drain(..nul_end);
    let varint_len = i - nul_end;
    buf.drain(..varint_len);
    let payload: Vec<u8> = buf.drain(..payload_len).collect();
    Some(payload)
}

// ---------------- 测试辅助 ----------------

async fn start_server_with_phira(
    phira_addr: &str,
) -> (Arc<phira_mp::server::ServerContext>, String) {
    start_server(phira_addr, false).await
}

async fn start_server(
    phira_addr: &str,
    proxy_protocol: bool,
) -> (Arc<phira_mp::server::ServerContext>, String) {
    // 记录旧地址，避免轮询时读到上一个已关闭 server 的残留地址
    let old_addr = phira_mp::server::test_listen_addr();
    let args = ServerArgs {
        port: 0,
        host: "127.0.0.1".into(),
        proxy_protocol,
        http_port: 0,
        language: "zh-CN".into(),
        session_timeout: 300,
        phira_api: format!("http://{phira_addr}/"),
        record_dir: None,
    };
    tokio::spawn(async move {
        let _ = run(args).await;
    });
    let mut addr = String::new();
    for _ in 0..300 {
        if let Some(a) = phira_mp::server::test_listen_addr()
            && Some(&a) != old_addr.as_ref()
        {
            addr = a;
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    assert!(!addr.is_empty(), "server did not start");
    let global = phira_mp::server::test_global_ctx().expect("global ctx");
    (global, addr)
}

fn auth_token(n: i32) -> String {
    format!("test-token-{n}")
}

async fn authenticate(client: &mut TestClient, n: i32) -> AuthenticateData {
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
            result: PacketResult::Success(data),
            ..
        } => data,
        ClientBoundPacket::Authenticate {
            result: PacketResult::Failed(msg),
            ..
        } => {
            panic!("auth failed: {msg}")
        }
        _ => unreachable!(),
    }
}

// ---------------- 测试用例 ----------------

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn smoke_full_game_flow() {
    let _guard = SERIAL.lock().await;
    let phira = start_mock_phira().await;
    let (ctx, addr) = start_server_with_phira(&phira.addr).await;

    let mut c1 = TestClient::connect(&addr).await;
    let auth = authenticate(&mut c1, 1).await;
    assert_eq!(auth.user_profile.user_id, 1);
    assert!(auth.room_info.is_none());

    c1.send(&ServerBoundPacket::Ping).await;
    c1.expect_pong().await;

    c1.send(&ServerBoundPacket::CreateRoom {
        room_id: "ROOM1".into(),
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
    let auth2 = authenticate(&mut c2, 2).await;
    assert_eq!(auth2.user_profile.user_id, 2);

    c2.send(&ServerBoundPacket::JoinRoom {
        room_id: "ROOM1".into(),
        monitor: false,
        trailer: None,
    })
    .await;
    c2.recv_until(|p| matches!(p,
        ClientBoundPacket::JoinRoom { result: PacketResult::Success(JoinRoomData { users, .. }), .. } if users.len() == 2
    )).await;
    c1.recv_until(|p| matches!(p, ClientBoundPacket::OnJoinRoom { user_profile, .. } if user_profile.user_id == 2)).await;
    c1.recv_message(|m| matches!(m, Message::JoinRoom { user: 2, .. }))
        .await;

    c2.send(&ServerBoundPacket::Chat {
        message: "hello".into(),
        trailer: None,
    })
    .await;
    c2.recv_until(|p| {
        matches!(
            p,
            ClientBoundPacket::Chat {
                result: PacketResult::Success(()),
                ..
            }
        )
    })
    .await;
    c1.recv_message(|m| matches!(m, Message::Chat { user: 2, content } if content == "hello"))
        .await;

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
    c1.recv_until(|p| {
        matches!(
            p,
            ClientBoundPacket::ChangeState {
                game_state: GameState::SelectChart { chart_id: Some(42) },
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
    c2.recv_message(|m| {
        matches!(
            m,
            Message::SelectChart {
                user: 1,
                id: 42,
                ..
            }
        )
    })
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
    c1.recv_until(|p| {
        matches!(
            p,
            ClientBoundPacket::ChangeState {
                game_state: GameState::WaitForReady,
                ..
            }
        )
    })
    .await;
    c2.recv_until(|p| {
        matches!(
            p,
            ClientBoundPacket::ChangeState {
                game_state: GameState::WaitForReady,
                ..
            }
        )
    })
    .await;
    c2.recv_message(|m| matches!(m, Message::GameStart { user: 1 }))
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
    c1.recv_message(|m| matches!(m, Message::StartPlaying))
        .await;

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
    c1.send(&ServerBoundPacket::Judges {
        judges: vec![JudgeEvent {
            time: 1.0,
            line_id: 0,
            note_id: 1,
            judgement: Judgement::Perfect,
        }],
        trailer: None,
    })
    .await;

    c1.send(&ServerBoundPacket::Played {
        record_id: 1001,
        trailer: None,
    })
    .await;
    c1.recv_until(|p| {
        matches!(
            p,
            ClientBoundPacket::Played {
                result: PacketResult::Success(()),
                ..
            }
        )
    })
    .await;
    c2.recv_message(|m| {
        matches!(
            m,
            Message::Played {
                user: 1,
                score: 998765,
                full_combo: true,
                ..
            }
        )
    })
    .await;

    c2.send(&ServerBoundPacket::Abort { trailer: None }).await;
    c2.recv_until(|p| {
        matches!(
            p,
            ClientBoundPacket::Abort {
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
                game_state: GameState::SelectChart { chart_id: Some(42) },
                ..
            }
        )
    })
    .await;
    c1.recv_message(|m| matches!(m, Message::GameEnd)).await;

    c2.send(&ServerBoundPacket::LeaveRoom { trailer: None })
        .await;
    c2.recv_until(|p| {
        matches!(
            p,
            ClientBoundPacket::LeaveRoom {
                result: PacketResult::Success(()),
                ..
            }
        )
    })
    .await;
    c1.recv_message(|m| matches!(m, Message::LeaveRoom { user: 2, .. }))
        .await;

    ctx.request_shutdown();
    ctx.wait_stopped().await;
    let _ = phira.shutdown.send(());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn smoke_reconnect_resume() {
    let _guard = SERIAL.lock().await;
    let phira = start_mock_phira().await;
    let (ctx, addr) = start_server_with_phira(&phira.addr).await;

    let mut c1 = TestClient::connect(&addr).await;
    authenticate(&mut c1, 1).await;
    c1.send(&ServerBoundPacket::CreateRoom {
        room_id: "RS".into(),
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

    drop(c1);
    for _ in 0..50 {
        if ctx.sessions.has_suspended(1) {
            break;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    assert!(ctx.sessions.has_suspended(1), "session should be suspended");

    let mut c1b = TestClient::connect(&addr).await;
    let auth = authenticate(&mut c1b, 1).await;
    let room_info = auth.room_info.expect("resume should carry room info");
    assert_eq!(room_info.room_id, "RS");
    assert!(room_info.is_host);
    assert!(!ctx.sessions.has_suspended(1), "session should be resumed");

    c1b.send(&ServerBoundPacket::Chat {
        message: "back".into(),
        trailer: None,
    })
    .await;
    c1b.recv_until(|p| {
        matches!(
            p,
            ClientBoundPacket::Chat {
                result: PacketResult::Success(()),
                ..
            }
        )
    })
    .await;

    ctx.request_shutdown();
    ctx.wait_stopped().await;
    let _ = phira.shutdown.send(());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn smoke_auth_failure_and_kick_semantics() {
    let _guard = SERIAL.lock().await;
    let phira = start_mock_phira().await;
    let (ctx, addr) = start_server_with_phira(&phira.addr).await;

    let mut bad = TestClient::connect(&addr).await;
    bad.send(&ServerBoundPacket::Ping).await;
    bad.expect_closed().await;

    let mut c = TestClient::connect(&addr).await;
    authenticate(&mut c, 1).await;
    c.send(&ServerBoundPacket::CreateRoom {
        room_id: "K1".into(),
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
    c.send(&ServerBoundPacket::Authenticate {
        token: auth_token(1),
        trailer: None,
    })
    .await;
    c.expect_closed().await;

    ctx.request_shutdown();
    ctx.wait_stopped().await;
    let _ = phira.shutdown.send(());
}

/// PROXY 协议实测：proxy_protocol=true 时，握手前先发 v1/v2 头应正常认证；
/// 垃圾头应直接断连。
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn smoke_proxy_protocol() {
    let _guard = SERIAL.lock().await;
    let phira = start_mock_phira().await;
    let (ctx, addr) = start_server(&phira.addr, true).await;

    // v1 文本头（PROXY TCP4 客户端 服务器 源端口 目的端口）+ 一次写完握手
    let mut c1 =
        TestClient::connect_with_prefix(&addr, b"PROXY TCP4 10.0.0.1 10.0.0.2 40000 12346\r\n")
            .await;
    authenticate(&mut c1, 1).await;
    c1.send(&ServerBoundPacket::Ping).await;
    c1.expect_pong().await;

    // v2 二进制头（sig + ver/cmd 0x21 + fam/proto 0x11 + len 12 + 地址）
    let mut v2 = b"\r\n\r\n\x00\r\nQUIT\n".to_vec();
    v2.extend_from_slice(&[0x21, 0x11, 0x00, 0x0C]);
    v2.extend_from_slice(&[10, 0, 0, 1]); // src ip
    v2.extend_from_slice(&[10, 0, 0, 2]); // dst ip
    v2.extend_from_slice(&40001u16.to_be_bytes());
    v2.extend_from_slice(&12346u16.to_be_bytes());
    let mut c2 = TestClient::connect_with_prefix(&addr, &v2).await;
    authenticate(&mut c2, 2).await;
    c2.send(&ServerBoundPacket::Ping).await;
    c2.expect_pong().await;

    // 垃圾头 → 连接被关闭（服务端解析失败应立即断开，可能读到 0 或对端 RST）
    let mut bad = TestClient::connect_with_prefix(&addr, b"GARBAGE HEADER\r\n").await;
    let mut closed = false;
    for _ in 0..60 {
        let mut tmp = [0u8; 64];
        match tokio::time::timeout(Duration::from_millis(100), bad.stream.read(&mut tmp)).await {
            Ok(Ok(0)) => {
                closed = true;
                break;
            }
            Ok(Ok(n)) => {
                eprintln!("[proxy-test] unexpected {n} bytes: {:02x?}", &tmp[..n]);
            }
            Ok(Err(_)) => {
                closed = true;
                break;
            } // RST → read error 也算关闭
            Err(_) => {} // 100ms 超时，重试
        }
    }
    assert!(closed, "expected connection closed");

    ctx.request_shutdown();
    ctx.wait_stopped().await;
    let _ = phira.shutdown.send(());
}
