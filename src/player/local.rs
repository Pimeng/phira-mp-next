//! 本地玩家与可重绑连接引用。

use super::Player;
use crate::network::connection::ConnectionHandle;
use crate::packet::clientbound::SharedFrame;
use crate::phira::UserInfo;
use std::any::Any;
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, RwLock};

/// 可重绑连接引用。顶号/断线重连时原子换绑，并处理旧连接的善后。
#[derive(Clone)]
pub struct ConnectionReference {
    inner: Arc<RwLock<ConnectionHandle>>,
}

impl ConnectionReference {
    pub fn new(conn: ConnectionHandle) -> Self {
        Self {
            inner: Arc::new(RwLock::new(conn)),
        }
    }

    /// 当前连接。
    pub fn get(&self) -> ConnectionHandle {
        self.inner.read().unwrap().clone()
    }

    /// 换绑到新连接；返回被替换的旧连接（调用方负责善后，如发「他处登录」）。
    pub fn resume(&self, new_conn: ConnectionHandle) -> ConnectionHandle {
        std::mem::replace(&mut *self.inner.write().unwrap(), new_conn)
    }

    /// 仅当当前连接与期望连接是同一实例时替换（防旧连接的清理覆盖新连接）。
    pub fn compare_exchange(&self, expect: &ConnectionHandle, new_conn: ConnectionHandle) -> bool {
        let mut current = self.inner.write().unwrap();
        if current.same_connection(expect) {
            *current = new_conn;
            true
        } else {
            false
        }
    }
}

pub struct LocalPlayer {
    info: Arc<UserInfo>,
    connection: ConnectionReference,
    kicked: AtomicBool,
    /// 扩展数据槽：key = TypeId。
    extensions: RwLock<HashMap<std::any::TypeId, Arc<dyn Any + Send + Sync>>>,
}

impl std::fmt::Debug for LocalPlayer {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LocalPlayer")
            .field("id", &self.info.id)
            .field("name", &self.info.name)
            .finish()
    }
}

impl LocalPlayer {
    pub fn new(info: Arc<UserInfo>, conn: ConnectionHandle) -> Arc<Self> {
        Arc::new(Self {
            info,
            connection: ConnectionReference::new(conn),
            kicked: AtomicBool::new(false),
            extensions: RwLock::new(HashMap::new()),
        })
    }

    // ---- 连接（经 ConnectionReference，支持换绑/顶号） ----

    pub fn connection(&self) -> ConnectionHandle {
        self.connection.get()
    }

    pub fn connection_ref(&self) -> &ConnectionReference {
        &self.connection
    }

    /// 换绑连接（断线重连 = 连接换绑）。返回旧连接。
    pub fn bind_connection(&self, conn: ConnectionHandle) -> ConnectionHandle {
        self.connection.resume(conn)
    }

    pub fn is_kicked(&self) -> bool {
        self.kicked.load(Ordering::SeqCst)
    }

    // ---- 发包 ----

    pub async fn send(&self, packet: crate::packet::clientbound::ClientBoundPacket) {
        self.connection().send(packet).await;
    }
}

impl Player for LocalPlayer {
    fn id(&self) -> i32 {
        self.info.id
    }

    fn name(&self) -> String {
        self.info.name.clone()
    }

    fn user_info(&self) -> Arc<UserInfo> {
        self.info.clone()
    }

    fn as_any(&self) -> &dyn Any {
        self
    }

    fn set_extension_typed(&self, type_id: std::any::TypeId, ext: Arc<dyn Any + Send + Sync>) {
        self.extensions.write().unwrap().insert(type_id, ext);
    }

    fn extension_typed(&self, type_id: std::any::TypeId) -> Option<Arc<dyn Any + Send + Sync>> {
        self.extensions.read().unwrap().get(&type_id).cloned()
    }

    fn bound_connection(&self) -> Option<ConnectionHandle> {
        Some(self.connection())
    }

    fn is_online(&self) -> bool {
        !self.connection().is_closed()
    }

    fn can_suspend(&self) -> bool {
        true
    }

    fn send_frame<'a>(&'a self, frame: SharedFrame) -> futures::future::BoxFuture<'a, ()> {
        let conn = self.connection();
        Box::pin(async move { conn.send_frame(frame).await })
    }

    fn kick(&self) {
        self.kicked.store(true, Ordering::SeqCst);
        let conn = self.connection();
        conn.mark_kicked();
    }
}

/// downcast：`Arc<dyn Player>` → `Arc<LocalPlayer>`（本地玩家专属操作入口）。
pub fn local_of(player: &Arc<dyn Player>) -> Option<Arc<LocalPlayer>> {
    if player.as_any().is::<LocalPlayer>() {
        // 安全：背面类型已确认为 LocalPlayer，Arc 布局一致（无 fat → fat 转换）。
        let raw = Arc::as_ptr(player) as *const LocalPlayer;
        unsafe {
            Arc::increment_strong_count(raw);
            Some(Arc::from_raw(raw))
        }
    } else {
        None
    }
}
