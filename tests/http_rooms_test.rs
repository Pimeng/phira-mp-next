//! HTTP API 集成测试：`GET /api/rooms`。
//!
//! 覆盖：建房 → 入房 → 选曲后，接口返回结构符合约定（roomid/cycle/lock/host/
//! state/chart/players），且以下划线 `_` 开头的房间被过滤。

use phira_mp::packet::clientbound::{ClientBoundPacket, JoinRoomData};
use phira_mp::packet::serverbound::ServerBoundPacket;
use phira_mp::packet::state::GameState;
use phira_mp::packet::PacketResult;
use phira_mp::server::{run, ServerArgs};
use serde_json::json;
use std::sync::Arc;
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};

/// 全局串行锁：GLOBAL_CTX 是进程级单例，测试必须串行执行。
static SERIAL: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

// ---------------- Mock Phira API（简化版，仅 /me /user/* /chart/*） ----------------

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
                        let token = req.lines().find_map(|l| {
                            let lower = l.to_ascii_lowercase();
                            lower.strip_prefix("authorization: bearer ").map(|_| {
                                l["Authorization: Bearer ".len()..].trim().to_string()
                            })
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
        return ("200 OK", json!({
            "id": id, "name": format!("Tester{id}"), "language": "zh-CN",
            "rks": 15.0, "banned": false, "loginBanned": false
        }).to_string());
    }
    if let Some(rest) = seg.strip_prefix("user/") {
        let id: i32 = rest.parse().unwrap_or(0);
        return ("200 OK", json!({
            "id": id, "name": format!("User{id}"), "language": "zh-CN",
            "rks": 14.0, "banned": false, "loginBanned": false
        }).to_string());
    }
    if let Some(rest) = seg.strip_prefix("chart/") {
        let id: i32 = rest.parse().unwrap_or(0);
        return ("200 OK", json!({
            "id": id, "name": format!("Chart{id}"), "level": "AT",
            "difficulty": 16.4, "charter": "charter", "composer": "composer",
            "ranked": true, "uploader": 1
        }).to_string());
    }
    ("404 Not Found", "{}".to_string())
}

// ---------------- 测试客户端（最小版） ----------------

struct TestClient {
    stream: TcpStream,
    rbuf: Vec<u8>,
}

impl TestClient {
    async fn connect(addr: &str) -> Self {
        let mut stream = TcpStream::connect(addr).await.unwrap();
        stream.set_nodelay(true).unwrap();
        stream.write_all(&[0x01]).await.unwrap(); // 握手
        TestClient { stream, rbuf: Vec::new() }
    }

    async fn send(&mut self, packet: &ServerBoundPacket) {
        use phira_mp::bytes::Encode;
        let mut body = bytes::BytesMut::new();
        packet.encode(&mut body);
        let frame = phira_mp::frame::encode_frame(&body);
        self.stream.write_all(&frame).await.unwrap();
    }

    async fn recv_until(&mut self, pred: impl Fn(&ClientBoundPacket) -> bool) -> ClientBoundPacket {
        for _ in 0..50 {
            let p = self.recv().await;
            if pred(&p) {
                return p;
            }
        }
        panic!("expected packet not received");
    }

    async fn recv(&mut self) -> ClientBoundPacket {
        let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
        loop {
            if let Some(payload) = take_one_frame(&mut self.rbuf) {
                return ClientBoundPacket::decode_frame(&payload).expect("decode clientbound");
            }
            let mut tmp = [0u8; 4096];
            let n = tokio::time::timeout_at(deadline, self.stream.read(&mut tmp))
                .await
                .expect("read timeout")
                .expect("read error");
            if n == 0 {
                panic!("connection closed");
            }
            self.rbuf.extend_from_slice(&tmp[..n]);
        }
    }
}

fn take_one_frame(buf: &mut Vec<u8>) -> Option<Vec<u8>> {
    let nul_end = buf.iter().position(|&b| b != 0).unwrap_or(buf.len());
    if nul_end == buf.len() {
        buf.clear();
        return None;
    }
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
    buf.drain(..nul_end);
    let varint_len = i - nul_end;
    buf.drain(..varint_len);
    Some(buf.drain(..payload_len).collect())
}

// ---------------- 测试辅助 ----------------

fn auth_token(n: i32) -> String {
    format!("test-token-{n}")
}

async fn authenticate(client: &mut TestClient, n: i32) {
    client
        .send(&ServerBoundPacket::Authenticate { token: auth_token(n), trailer: None })
        .await;
    let p = client
        .recv_until(|p| matches!(p, ClientBoundPacket::Authenticate { .. }))
        .await;
    match p {
        ClientBoundPacket::Authenticate { result: PacketResult::Success(_), .. } => {}
        ClientBoundPacket::Authenticate { result: PacketResult::Failed(msg), .. } => {
            panic!("auth failed: {msg}")
        }
        _ => unreachable!(),
    }
}

async fn start_server_with_http(phira_addr: &str) -> (Arc<phira_mp::server::ServerContext>, String, u16) {
    // 预占一个空闲端口作为 HTTP 端口
    let probe = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let http_port = probe.local_addr().unwrap().port();
    drop(probe);

    let old_addr = phira_mp::server::test_listen_addr();
    let args = ServerArgs {
        port: 0,
        host: "127.0.0.1".into(),
        proxy_protocol: false,
        http_port,
        http_host: "127.0.0.1".into(),
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
    let ctx = phira_mp::server::test_global_ctx().expect("global ctx");
    // 等待 HTTP 服务就绪
    for _ in 0..300 {
        if ctx.http_addr.read().unwrap().is_some() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    assert!(ctx.http_addr.read().unwrap().is_some(), "http did not start");
    (ctx, addr, http_port)
}

async fn get_rooms(http_port: u16) -> serde_json::Value {
    let resp = reqwest::get(format!("http://127.0.0.1:{http_port}/api/rooms"))
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    resp.json().await.unwrap()
}

// ---------------- 测试用例 ----------------

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn http_rooms_list_structure() {
    let _guard = SERIAL.lock().await;
    let phira = start_mock_phira().await;
    let (ctx, addr, http_port) = start_server_with_http(&phira.addr).await;

    // 空列表
    let body = get_rooms(http_port).await;
    assert_eq!(body["total"], 0);
    assert_eq!(body["rooms"].as_array().unwrap().len(), 0);

    // 房主建房 room1
    let mut c1 = TestClient::connect(&addr).await;
    authenticate(&mut c1, 1).await;
    c1.send(&ServerBoundPacket::CreateRoom { room_id: "room1".into(), trailer: None }).await;
    c1.recv_until(|p| matches!(p, ClientBoundPacket::CreateRoom { result: PacketResult::Success(()), .. })).await;

    // 隐藏房间（_ 开头）不应出现在列表
    let mut c0 = TestClient::connect(&addr).await;
    authenticate(&mut c0, 3).await;
    c0.send(&ServerBoundPacket::CreateRoom { room_id: "_internal".into(), trailer: None }).await;
    c0.recv_until(|p| matches!(p, ClientBoundPacket::CreateRoom { result: PacketResult::Success(()), .. })).await;

    // 玩家2 加入 room1
    let mut c2 = TestClient::connect(&addr).await;
    authenticate(&mut c2, 2).await;
    c2.send(&ServerBoundPacket::JoinRoom { room_id: "room1".into(), monitor: false, trailer: None }).await;
    c2.recv_until(|p| matches!(p,
        ClientBoundPacket::JoinRoom { result: PacketResult::Success(JoinRoomData { users, .. }), .. } if users.len() == 2
    )).await;
    c1.recv_until(|p| matches!(p, ClientBoundPacket::OnJoinRoom { user_profile, .. } if user_profile.user_id == 2)).await;

    // 房主选曲 42
    c1.send(&ServerBoundPacket::SelectChart { id: 42, trailer: None }).await;
    c1.recv_until(|p| matches!(p, ClientBoundPacket::SelectChart { result: PacketResult::Success(()), .. })).await;
    c2.recv_until(|p| matches!(p, ClientBoundPacket::ChangeState { game_state: GameState::SelectChart { chart_id: Some(42) }, .. })).await;

    let body = get_rooms(http_port).await;
    assert_eq!(body["total"], 1, "hidden room must be filtered: {body}");
    let room = &body["rooms"][0];
    assert_eq!(room["roomid"], "room1");
    assert_eq!(room["cycle"], false);
    assert_eq!(room["lock"], false);
    assert_eq!(room["state"], "select_chart");
    // host：房主 alice，id 为字符串
    assert_eq!(room["host"]["name"], "Tester1");
    assert_eq!(room["host"]["id"], "1");
    // chart：选曲 42，id 为字符串
    assert_eq!(room["chart"]["name"], "Chart42");
    assert_eq!(room["chart"]["id"], "42");
    // players：两个成员，id 为数字
    let players = room["players"].as_array().unwrap();
    assert_eq!(players.len(), 2);
    assert_eq!(players[0]["name"], "Tester1");
    assert_eq!(players[0]["id"], 1);
    assert_eq!(players[1]["name"], "Tester2");
    assert_eq!(players[1]["id"], 2);

    ctx.request_shutdown();
    ctx.wait_stopped().await;
    let _ = phira.shutdown.send(());
}
