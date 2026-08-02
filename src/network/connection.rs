//! PlayerConnection（2.4、5.4、7.1 节）。
//!
//! - 每连接一个 writer 任务（mpsc 队列），关闭时先 drain 避免丢帧。
//! - 读循环：帧解码 → `PacketReceiveEvent`（可取消）→ 串行交给当前
//!   `PacketHandler`（take/put 模式，避免锁/MutexGuard 跨 await）。
//! - 出站：`PacketSendEvent`（可取消）→ writer。
//! - 初始 handler 由 [`spawn_connection`] 工厂参数决定（默认 `AuthenticateHandler`）；
//!   阶段切换抛 `PlayerSwitchPacketHandlerEvent`（可替换/装饰新 handler）。
//! - 断开原因（ConnectState）：QUIT / KICK / TIMEOUT / DUPLICATE / ERROR。

use crate::frame::FrameDecoder;
use crate::network::handler::{HandleOutcome, HandlerContext, PacketHandler};
use crate::network::{HANDSHAKE_TIMEOUT, PROTOCOL_VERSION, READ_TIMEOUT};
use crate::packet::clientbound::{ClientBoundPacket, SharedFrame};
use crate::packet::serverbound::ServerBoundPacket;
use ::bytes::Bytes;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicU8, Ordering};
use std::sync::Arc;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;
use tokio::sync::{mpsc, Mutex, Notify};
use tokio::time::timeout;
use tracing::{debug, info, warn};

/// 断开原因（5.4 节 DisconnectReason）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum DisconnectReason {
    Quit = 0,
    Kick = 1,
    Timeout = 2,
    Duplicate = 3,
    Error = 4,
}

impl DisconnectReason {
    fn from_state(state: u8) -> Self {
        match state {
            1 => DisconnectReason::Kick,
            2 => DisconnectReason::Timeout,
            3 => DisconnectReason::Duplicate,
            4 => DisconnectReason::Error,
            _ => DisconnectReason::Quit,
        }
    }
}

static NEXT_CONN_ID: AtomicU64 = AtomicU64::new(1);

pub(crate) enum WriterMsg {
    Frame(Bytes),
    Shared(SharedFrame),
    Close,
}

/// 连接句柄（可换绑、可跨任务共享）。
#[derive(Clone)]
pub struct ConnectionHandle {
    inner: Arc<Inner>,
}

struct Inner {
    id: u64,
    tx: mpsc::UnboundedSender<WriterMsg>,
    /// ConnectState：0=ACTIVE 1=KICK 2=TIMEOUT 3=DUPLICATE 4=ERROR
    state: AtomicU8,
    closed: AtomicBool,
    close_notify: Notify,
    peer_addr: Mutex<String>,
}

impl ConnectionHandle {
    pub fn id(&self) -> u64 {
        self.inner.id
    }

    /// 发送一个包（经 `PacketSendEvent`，可取消）。
    pub async fn send(&self, packet: ClientBoundPacket) {
        if self.inner.closed.load(Ordering::SeqCst) {
            return;
        }
        // PacketSendEvent：可取消（对应 Java PlayerConnection.send 的 PacketSendEvent）
        if let Some(ctx) = crate::server::global_ctx() {
            let ev = crate::events::PacketSendEvent {
                packet: packet.clone(),
                cancel_reason: None,
            };
            let ev = ctx.events.post_mut(crate::events::PACKET_SEND, ev).await;
            if ev.is_cancelled() {
                return;
            }
        }
        let frame = crate::packet::clientbound::encode_packet(&packet);
        let _ = self.inner.tx.send(WriterMsg::Frame(frame));
    }

    /// 发送预编码共享帧（广播零拷贝路径；不触发 PacketSendEvent——
    /// 广播已在事件层决策过，此处是纯字节转发）。
    pub async fn send_frame(&self, frame: SharedFrame) {
        if self.inner.closed.load(Ordering::SeqCst) {
            return;
        }
        let _ = self.inner.tx.send(WriterMsg::Shared(frame));
    }

    /// 主动关闭（踢人/认证失败等）。不立即改 state，由调用方先标记。
    pub async fn close(&self) {
        let _ = self.inner.tx.send(WriterMsg::Close);
    }

    pub fn mark_kicked(&self) {
        self.inner.state.store(1, Ordering::SeqCst);
    }

    pub fn mark_duplicate(&self) {
        self.inner.state.store(3, Ordering::SeqCst);
    }

    /// 连接是否已关闭（writer 任务退出置位，或对端断开导致 tx 失败）。
    pub fn is_closed(&self) -> bool {
        self.inner.closed.load(Ordering::SeqCst) || self.inner.tx.is_closed()
    }

    pub fn disconnect_reason(&self) -> DisconnectReason {
        DisconnectReason::from_state(self.inner.state.load(Ordering::SeqCst))
    }

    /// 等待连接关闭（读循环结束时触发）。
    pub async fn closed_wait(&self) {
        if self.is_closed() {
            return;
        }
        self.inner.close_notify.notified().await;
    }

    pub async fn peer_addr(&self) -> String {
        self.inner.peer_addr.lock().await.clone()
    }

    pub async fn set_peer_addr(&self, addr: String) {
        *self.inner.peer_addr.lock().await = addr;
    }

    /// 测试用构造器：从外部传入 writer 通道。
    #[cfg(test)]
    pub(crate) fn new_for_test(tx: mpsc::UnboundedSender<WriterMsg>) -> Self {
        ConnectionHandle {
            inner: Arc::new(Inner {
                id: NEXT_CONN_ID.fetch_add(1, Ordering::Relaxed),
                tx,
                state: AtomicU8::new(0),
                closed: AtomicBool::new(false),
                close_notify: Notify::new(),
                peer_addr: Mutex::new("test".into()),
            }),
        }
    }
}

/// 连接处理入口：装配（可选 PROXY）→ 握手 → 帧/包循环 → 清理。
///
/// `initial_handler` 决定第一个阶段处理器（对应 Java 接管连接 handler 的能力）。
pub async fn spawn_connection(
    mut stream: TcpStream,
    ctx: Arc<crate::server::ServerContext>,
    proxy_protocol: bool,
    initial_handler: Box<dyn PacketHandler>,
) {
    let _ = stream.set_nodelay(true);
    let conn_id = NEXT_CONN_ID.fetch_add(1, Ordering::Relaxed);
    let peer = stream
        .peer_addr()
        .map(|a| a.to_string())
        .unwrap_or_else(|_| "unknown".into());
    debug!(conn_id, %peer, "new connection");

    // 1. 可选 PROXY 协议
    let mut pending: Vec<u8> = Vec::new();
    let mut real_peer = peer.clone();
    if proxy_protocol {
        match crate::network::proxy::parse_proxy(&mut stream).await {
            Ok(res) => {
                if let Some(addr) = res.real_addr {
                    real_peer = addr.to_string();
                }
                pending = res.pending;
            }
            Err(e) => {
                warn!(conn_id, "proxy protocol failed: {e}");
                return;
            }
        }
    }

    // 2. 握手：读 1 字节协议版本（先消费 pending）
    let version = match read_version(&mut stream, &mut pending).await {
        Ok(v) => v,
        Err(e) => {
            debug!(conn_id, "handshake failed: {e}");
            return;
        }
    };
    if version != PROTOCOL_VERSION {
        debug!(conn_id, "bad protocol version {version:#04x}");
        return;
    }

    // 3. 建立连接对象
    let (tx, mut rx) = mpsc::unbounded_channel::<WriterMsg>();
    let inner = Arc::new(Inner {
        id: conn_id,
        tx,
        state: AtomicU8::new(0),
        closed: AtomicBool::new(false),
        close_notify: Notify::new(),
        peer_addr: Mutex::new(real_peer.clone()),
    });
    let conn = ConnectionHandle { inner: inner.clone() };

    let (mut read_half, mut write_half) = stream.into_split();

    // writer 任务：关闭信号优先 drain 已排队帧（避免丢帧）
    let writer_inner = inner.clone();
    tokio::spawn(async move {
        loop {
            match rx.recv().await {
                Some(WriterMsg::Frame(data)) => {
                    tracing::trace!(conn_id, len = data.len(), "writer frame");
                    if write_half.write_all(&data).await.is_err() {
                        break;
                    }
                }
                Some(WriterMsg::Shared(data)) => {
                    tracing::trace!(conn_id, len = data.len(), "writer shared frame");
                    if write_half.write_all(&data).await.is_err() {
                        break;
                    }
                }
                Some(WriterMsg::Close) => {
                    // drain 剩余帧
                    while let Ok(msg) = rx.try_recv() {
                        match msg {
                            WriterMsg::Frame(data) => {
                                let _ = write_half.write_all(&data).await;
                            }
                            WriterMsg::Shared(data) => {
                                let _ = write_half.write_all(&data).await;
                            }
                            WriterMsg::Close => {}
                        }
                    }
                    break;
                }
                None => break,
            }
        }
        let _ = write_half.shutdown().await;
        writer_inner.closed.store(true, Ordering::SeqCst);
        writer_inner.close_notify.notify_waiters();
    });

    // 4. 读循环：帧 → 包 → handler（串行）
    let handler_ctx = Arc::new(HandlerContext {
        conn: conn.clone(),
        server: ctx.clone(),
    });
    let mut handler: Option<Box<dyn PacketHandler>> = Some(initial_handler);
    let mut frame_dec = FrameDecoder::new();
    if !pending.is_empty() {
        frame_dec.feed(&pending);
    }
    let mut read_buf = [0u8; 8192];

    loop {
        match frame_dec.next_frame() {
            Ok(Some(frame)) => {
                if !dispatch(frame, &mut handler, &handler_ctx).await {
                    break;
                }
                continue;
            }
            Ok(None) => {}
            Err(e) => {
                debug!(conn_id, "frame error: {e}");
                break;
            }
        }

        let read = timeout(READ_TIMEOUT, read_half.read(&mut read_buf)).await;
        match read {
            Ok(Ok(0)) => break, // EOF
            Ok(Ok(n)) => {
                frame_dec.feed(&read_buf[..n]);
            }
            Ok(Err(e)) => {
                inner.state.store(4, Ordering::SeqCst); // ERROR
                debug!(conn_id, "read error: {e}");
                break;
            }
            Err(_) => {
                if inner.state.load(Ordering::SeqCst) == 0 {
                    inner.state.store(2, Ordering::SeqCst); // TIMEOUT
                }
                break;
            }
        }
    }

    // 5. 清理
    let already_closed = inner.closed.swap(true, Ordering::SeqCst);
    inner.close_notify.notify_waiters();
    if !already_closed {
        let _ = inner.tx.send(WriterMsg::Close);
    }

    // 断线清理（对应 Java resolvePlayer closeBinder 注册的 onClose 回调）：
    // PlayerDisconnectEvent → 换绑跳过 → 未入房移除 → Monitor 离开 → 可挂起则挂起。
    let reason = conn.disconnect_reason();
    if let Some(h) = handler.as_ref()
        && let Some(player) = h.player_ref() {
            on_connection_closed(&ctx, &conn, &player, h.room_ref(), h.is_suspendable_room_holder(), reason).await;
        }
    info!(conn_id, ?reason, "connection closed");
}

/// 断线清理（5.3/5.4 节；对应 Java onClose 回调链）。
async fn on_connection_closed(
    ctx: &Arc<crate::server::ServerContext>,
    conn: &ConnectionHandle,
    player: &Arc<dyn crate::player::Player>,
    room: Option<Arc<dyn crate::room::Room>>,
    suspendable: bool,
    reason: DisconnectReason,
) {
    // PlayerDisconnectEvent
    ctx.events
        .post(crate::events::PLAYER_DISCONNECT, crate::events::PlayerDisconnectEvent {
            player: player.clone(),
            reason,
        })
        .await;

    // 已换绑/顶号：本连接不再代表该玩家 → 跳过
    let same_conn = crate::player::local_of(player)
        .map(|l| l.connection().id() == conn.id())
        .unwrap_or(true);
    if !same_conn {
        return;
    }

    let Some(room) = room else {
        // 未入房：直接移除注册
        ctx.players.remove_if_bound(player.id(), conn.id());
        return;
    };

    // Monitor 不挂起，直接离开（对应 Java SuspendableRoomHolder 为 false）
    if !suspendable || room.is_monitor_user(player.id()) {
        let (_l, plan, _d) = room.leave(player.id());
        crate::room::send_broadcasts(plan).await;
        ctx.players.remove_if_bound(player.id(), conn.id());
        return;
    }

    // 可挂起：PlayerSessionSuspendEvent（可取消→直接退房）→ 挂起
    let ev = crate::events::PlayerSessionSuspendEvent {
        player: player.clone(),
        room: room.clone(),
        cancel_reason: None,
    };
    let ev = ctx
        .events
        .post_mut(crate::events::PLAYER_SESSION_SUSPEND, ev)
        .await;
    if ev.is_cancelled() {
        let (_l, plan, _d) = room.leave(player.id());
        crate::room::send_broadcasts(plan).await;
        ctx.players.remove_if_bound(player.id(), conn.id());
        return;
    }

    let uid = player.id();
    let cid = conn.id();
    let ctx2 = ctx.clone();
    let remover = move || {
        ctx2.players.remove_if_bound(uid, cid);
    };
    if ctx.sessions.suspend(player.clone(), room, remover).await.is_err() {
        ctx.players.remove_if_bound(player.id(), conn.id());
        return;
    }
    info!(user_id = player.id(), "session suspended");
}

/// 处理单个帧。返回 false 表示应终止读循环。
async fn dispatch(
    frame: Bytes,
    handler: &mut Option<Box<dyn PacketHandler>>,
    ctx: &Arc<HandlerContext>,
) -> bool {
    let packet = match ServerBoundPacket::decode_frame(&frame) {
        Ok(p) => p,
        Err(e) => {
            debug!("packet decode error: {e}");
            return false; // 未知包/解码失败 → 关闭连接
        }
    };

    // PacketReceiveEvent：可取消（对应 Java PlayerConnection.channelRead 的 PacketReceiveEvent）
    let packet = {
        let ev = crate::events::PacketReceiveEvent {
            packet,
            cancel_reason: None,
        };
        let ev = ctx
            .server
            .events
            .post_mut(crate::events::PACKET_RECEIVE, ev)
            .await;
        if ev.is_cancelled() {
            return true; // 丢弃包，连接保持
        }
        ev.packet
    };

    // take/put 模式：避免借用跨 await 问题，同时支持 Switch 语义
    let mut h = match handler.take() {
        Some(h) => h,
        None => return false,
    };
    let outcome = h.handle(ctx, packet).await;
    match outcome {
        HandleOutcome::Ok | HandleOutcome::Failed => {
            *handler = Some(h);
            true
        }
        HandleOutcome::Switch(next) => {
            // PlayerSwitchPacketHandlerEvent：可替换/装饰新 handler；
            // 事件消费了 handler（置空）→ 保持旧 handler。
            let player = next.player_ref();
            let ev = crate::events::PlayerSwitchPacketHandlerEvent {
                player,
                new_handler: Some(next),
            };
            let mut ev = ctx
                .server
                .events
                .post_mut(crate::events::PLAYER_SWITCH_PACKET_HANDLER, ev)
                .await;
            *handler = Some(ev.new_handler.take().unwrap_or(h));
            true
        }
        HandleOutcome::Close => {
            ctx.conn.close().await;
            *handler = Some(h); // 放回，保证清理阶段能取到 player/room
            false
        }
    }
}

/// 读 1 字节协议版本：先消费 pending，再读 socket，5 秒超时。
async fn read_version(stream: &mut TcpStream, pending: &mut Vec<u8>) -> std::io::Result<u8> {
    if !pending.is_empty() {
        return Ok(pending.remove(0));
    }
    let mut b = [0u8; 1];
    timeout(HANDSHAKE_TIMEOUT, stream.read_exact(&mut b))
        .await
        .map_err(|_| std::io::Error::new(std::io::ErrorKind::TimedOut, "handshake timeout"))??;
    Ok(b[0])
}
