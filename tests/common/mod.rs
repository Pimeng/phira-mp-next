//! 集成测试共享辅助：TestPlayer（无连接内存玩家）、帧编解码工具、
//! Mock Phira HTTP 服务、真实 TCP 测试客户端、服务端启动辅助。
//!
//! 说明：
//! - `TestPlayer` 实现 `Player` trait 但不持有连接（integration test 无法访问
//!   `ConnectionHandle::new_for_test`），收到的广播帧被收集供断言。
//! - 启动真实服务端的测试文件需先取 `SERIAL` 锁（进程级 GLOBAL_CTX 单例）。
//!
//! 每个测试二进制只会用到其中一部分辅助，故整体允许 dead_code。

#![allow(dead_code)]

use phira_mp::packet::clientbound::{ClientBoundPacket, SharedFrame};
use phira_mp::packet::serverbound::ServerBoundPacket;
use phira_mp::phira::UserInfo;
use phira_mp::player::Player;
use phira_mp::server::{run, ServerArgs};
use serde_json::json;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};

/// 进程级串行锁：GLOBAL_CTX 是单例，启动服务端的测试必须串行执行。
/// 每个测试二进制（独立进程）有各自的 GLOBAL_CTX，跨文件并行安全；
/// 同一文件内多测试并发则靠本锁串行化。
#[allow(dead_code)]
pub static SERIAL: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

// ---------------- TestPlayer ----------------

/// 纯内存测试玩家：可控制在线状态、可收集广播帧、可记录踢出。
pub struct TestPlayer {
    pub id: i32,
    pub name: String,
    pub info: Arc<UserInfo>,
    pub online: AtomicBool,
    pub kicked: AtomicBool,
    pub frames: Mutex<Vec<SharedFrame>>,
}

impl TestPlayer {
    pub fn new(id: i32, name: &str) -> Arc<Self> {
        Arc::new(Self {
            id,
            name: name.to_string(),
            info: Arc::new(UserInfo {
                id,
                name: name.to_string(),
                ..Default::default()
            }),
            online: AtomicBool::new(true),
            kicked: AtomicBool::new(false),
            frames: Mutex::new(Vec::new()),
        })
    }

    pub fn set_online(&self, v: bool) {
        self.online.store(v, Ordering::SeqCst);
    }

    /// 收到的全部包（按发送顺序，去帧头）。
    pub fn packets(&self) -> Vec<ClientBoundPacket> {
        self.frames
            .lock()
            .unwrap()
            .iter()
            .filter_map(decode_frame_payload)
            .collect()
    }

    /// 收到的 Message 包。
    pub fn messages(&self) -> Vec<phira_mp::packet::message::Message> {
        self.packets()
            .into_iter()
            .filter_map(|p| match p {
                ClientBoundPacket::Message { message, .. } => Some(message),
                _ => None,
            })
            .collect()
    }

    /// 取走并清空收集的帧（增量断言用）。
    pub fn take_frames(&self) -> Vec<SharedFrame> {
        std::mem::take(&mut *self.frames.lock().unwrap())
    }
}

impl Player for TestPlayer {
    fn id(&self) -> i32 {
        self.id
    }

    fn name(&self) -> String {
        self.name.clone()
    }

    fn user_info(&self) -> Arc<UserInfo> {
        self.info.clone()
    }

    fn as_any(&self) -> &dyn std::any::Any {
        self
    }

    fn is_online(&self) -> bool {
        self.online.load(Ordering::SeqCst)
    }

    fn send_frame<'a>(&'a self, frame: SharedFrame) -> futures::future::BoxFuture<'a, ()> {
        Box::pin(async move {
            self.frames.lock().unwrap().push(frame);
        })
    }

    fn kick(&self) {
        self.kicked.store(true, Ordering::SeqCst);
    }
}

// ---------------- 帧解码工具 ----------------

/// 从完整帧字节（含 VarInt 帧头）中取出 payload。
pub fn take_one_frame(buf: &mut Vec<u8>) -> Option<Vec<u8>> {
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

/// 剥掉 VarInt 帧头后解码 ClientBoundPacket。
pub fn decode_frame_payload(frame: &SharedFrame) -> Option<ClientBoundPacket> {
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

// ---------------- Mock Phira API ----------------

pub struct MockPhira {
    pub addr: String,
    pub shutdown: tokio::sync::oneshot::Sender<()>,
}

#[allow(dead_code)]
pub async fn start_mock_phira() -> MockPhira {
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

#[allow(dead_code)]
pub fn mock_response(path: &str, token: Option<&str>) -> (&'static str, String) {
    if token == Some("banned-token") {
        return ("401 Unauthorized", "{}".to_string());
    }
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
    if let Some(rest) = seg.strip_prefix("record/") {
        let id: i32 = rest.parse().unwrap_or(0);
        return ("200 OK", json!({
            "id": id, "player": 1, "chart": 42, "score": 998765, "accuracy": 99.87,
            "perfect": 900, "good": 5, "bad": 1, "miss": 0,
            "fullCombo": true, "maxCombo": 906, "speed": 1.0
        }).to_string());
    }
    ("404 Not Found", "{}".to_string())
}

// ---------------- 测试客户端（真实 TCP） ----------------

pub struct TestClient {
    pub stream: TcpStream,
    pub rbuf: Vec<u8>,
}

impl TestClient {
    #[allow(dead_code)]
    pub async fn connect(addr: &str) -> Self {
        Self::connect_with_prefix(addr, &[]).await
    }

    /// 连接并在握手字节前发送任意前缀（用于 PROXY 协议头）。
    #[allow(dead_code)]
    pub async fn connect_with_prefix(addr: &str, prefix: &[u8]) -> Self {
        let mut stream = TcpStream::connect(addr).await.unwrap();
        stream.set_nodelay(true).unwrap();
        if !prefix.is_empty() {
            stream.write_all(prefix).await.unwrap();
        }
        stream.write_all(&[0x01]).await.unwrap(); // 握手
        TestClient { stream, rbuf: Vec::new() }
    }

    /// 发送已编码的完整帧字节。
    #[allow(dead_code)]
    pub async fn send_raw(&mut self, frame: &[u8]) {
        self.stream.write_all(frame).await.unwrap();
    }

    #[allow(dead_code)]
    pub async fn send(&mut self, packet: &ServerBoundPacket) {
        use phira_mp::bytes::Encode;
        let mut body = bytes::BytesMut::new();
        packet.encode(&mut body);
        let frame = phira_mp::frame::encode_frame(&body);
        self.stream.write_all(&frame).await.unwrap();
    }

    #[allow(dead_code)]
    pub async fn recv(&mut self) -> ClientBoundPacket {
        let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(5);
        loop {
            if let Some(payload) = take_one_frame(&mut self.rbuf) {
                return ClientBoundPacket::decode_frame(&payload).expect("decode clientbound");
            }
            if tokio::time::Instant::now() > deadline {
                panic!(
                    "recv timeout; buffered={} bytes={:02x?}",
                    self.rbuf.len(),
                    &self.rbuf[..self.rbuf.len().min(64)]
                );
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

    #[allow(dead_code)]
    pub async fn recv_until(&mut self, pred: impl Fn(&ClientBoundPacket) -> bool) -> ClientBoundPacket {
        for _ in 0..50 {
            let p = self.recv().await;
            if pred(&p) {
                return p;
            }
        }
        panic!("expected packet not received");
    }

    #[allow(dead_code)]
    pub async fn expect_pong(&mut self) {
        let p = self.recv().await;
        assert!(matches!(p, ClientBoundPacket::Pong), "expected Pong, got id={}", p.id());
    }

    /// 期望连接被对端关闭（读到 EOF 或 RST）。
    #[allow(dead_code)]
    pub async fn expect_closed(&mut self) {
        let mut tmp = [0u8; 64];
        let n = tokio::time::timeout(std::time::Duration::from_secs(6), self.stream.read(&mut tmp))
            .await
            .expect("close timeout")
            .unwrap_or(1);
        assert_eq!(n, 0, "expected connection closed");
    }
}

// ---------------- 服务端启动 ----------------

#[allow(dead_code)]
pub async fn start_server(phira_addr: &str) -> (Arc<phira_mp::server::ServerContext>, String) {
    let old_addr = phira_mp::server::test_listen_addr();
    let args = ServerArgs {
        port: 0,
        host: "127.0.0.1".into(),
        proxy_protocol: false,
        http_port: 0,
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
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }
    assert!(!addr.is_empty(), "server did not start");
    let global = phira_mp::server::test_global_ctx().expect("global ctx");
    (global, addr)
}

#[allow(dead_code)]
pub fn auth_token(n: i32) -> String {
    format!("test-token-{n}")
}

/// 认证并断言成功，返回 AuthenticateData。
#[allow(dead_code)]
pub async fn authenticate(
    client: &mut TestClient,
    n: i32,
) -> phira_mp::packet::clientbound::AuthenticateData {
    use phira_mp::packet::PacketResult;
    client
        .send(&ServerBoundPacket::Authenticate { token: auth_token(n), trailer: None })
        .await;
    let p = client
        .recv_until(|p| matches!(p, ClientBoundPacket::Authenticate { .. }))
        .await;
    match p {
        ClientBoundPacket::Authenticate { result: PacketResult::Success(data), .. } => data,
        ClientBoundPacket::Authenticate { result: PacketResult::Failed(msg), .. } => {
            panic!("auth failed: {msg}")
        }
        _ => unreachable!(),
    }
}
