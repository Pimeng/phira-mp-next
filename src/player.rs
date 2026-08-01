//! 玩家与全局注册表（5.2、7.4 节）。
//!
//! - `Player`：持 UserInfo + ConnectionReference（可换绑）。
//! - 「是否在房间」由 handler 位置决定（RoomHandler 携带 Room 引用）。
//! - `PlayerRegistry`：userId 全局唯一，create-or-resume 原子化。

use crate::network::connection::ConnectionHandle;
use crate::network::handler::PacketHandler;
use crate::packet::data::RoomInfo;
use crate::phira::UserInfo;
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, RwLock};

pub struct Player {
    info: Arc<UserInfo>,
    connection: RwLock<ConnectionHandle>,
    kicked: AtomicBool,
}

impl std::fmt::Debug for Player {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Player")
            .field("id", &self.info.id)
            .field("name", &self.info.name)
            .finish()
    }
}

impl Player {
    pub fn new(info: Arc<UserInfo>, conn: ConnectionHandle) -> Arc<Self> {
        Arc::new(Self {
            info,
            connection: RwLock::new(conn),
            kicked: AtomicBool::new(false),
        })
    }

    pub fn id(&self) -> i32 {
        self.info.id
    }

    pub fn name(&self) -> String {
        self.info.name.clone()
    }

    pub fn language(&self) -> Option<String> {
        self.info.language.clone()
    }

    pub fn user_info(&self) -> Arc<UserInfo> {
        self.info.clone()
    }

    pub fn connection(&self) -> ConnectionHandle {
        self.connection.read().unwrap().clone()
    }

    /// 换绑连接（断线重连 = 连接换绑）。
    pub fn bind_connection(&self, conn: ConnectionHandle) {
        *self.connection.write().unwrap() = conn;
    }

    pub fn is_online(&self) -> bool {
        !self.connection().is_closed()
    }

    /// 踢人：标记后由 room/handler 层调用 leave + close。
    pub fn kick(&self) {
        self.kicked.store(true, Ordering::SeqCst);
        let conn = self.connection();
        conn.mark_kicked();
    }

    pub fn is_kicked(&self) -> bool {
        self.kicked.load(Ordering::SeqCst)
    }

    pub async fn send(&self, packet: crate::packet::clientbound::ClientBoundPacket) {
        self.connection().send(packet).await;
    }

    /// RoomInfo 快照（认证响应用；viewer 即本人）。
    /// 「是否在房间」由外部（handler 链）传入。
    pub fn get_room_info(&self, viewer: &Player) -> Option<RoomInfo> {
        let room = self.current_room()?;
        Some(room.build_room_info(viewer))
    }

    /// 当前房间：从会话管理器/房间注册表反查。
    pub fn current_room(&self) -> Option<Arc<crate::room::Room>> {
        crate::server::current_room_of(self.id())
    }
}

/// resolve 结果。
pub struct ResolveResult {
    pub player: Arc<Player>,
    pub created: bool,
    /// 恢复（Resume）时复用的旧 handler（可能是 RoomHandler）。
    pub next_handler: Option<Box<dyn PacketHandler>>,
}

/// 全局玩家注册表（ConcurrentHashMap 等价物）。
pub struct PlayerRegistry {
    players: Mutex<HashMap<i32, Arc<Player>>>,
}

impl Default for PlayerRegistry {
    fn default() -> Self {
        Self::new()
    }
}

impl PlayerRegistry {
    pub fn new() -> Self {
        Self {
            players: Mutex::new(HashMap::new()),
        }
    }

    /// 注册或恢复。原子化（锁内完成）。
    ///
    /// 返回 (ResolveResult, 旧连接)。旧连接若存在且未关闭，调用方负责踢旧。
    pub async fn resolve_or_resume(
        &self,
        info: Arc<UserInfo>,
        conn: &ConnectionHandle,
    ) -> Result<(ResolveResult, Option<ConnectionHandle>), String> {
        // 先在锁内完成注册/换绑决策
        let (player, created, old_conn, suspended) = {
            let mut map = self.players.lock().unwrap();
            match map.get(&info.id) {
                None => {
                    let player = Player::new(info, conn.clone());
                    map.insert(player.id(), player.clone());
                    (player, true, None, None)
                }
                Some(existing) => {
                    let old = existing.connection();
                    if old.id() == conn.id() {
                        // 同一连接重复认证，不应发生（AuthenticateHandler 只执行一次）
                        (existing.clone(), false, None, None)
                    } else {
                        // 换绑
                        existing.bind_connection(conn.clone());
                        let suspended = crate::server::with_server_ctx(|ctx| {
                            ctx.sessions.take_suspended(existing.id())
                        })
                        .flatten();
                        (existing.clone(), false, Some(old), suspended)
                    }
                }
            }
        };

        // 恢复挂起会话（返回旧 handler 供连接直接复用房间上下文）
        let next_handler = if let Some(session) = suspended {
            // 校验玩家仍在原房间
            if session.room.contains_member(player.id()) {
                Some(Box::new(crate::network::room_handler::RoomHandler::new(
                    player.clone(),
                    session.room.clone(),
                )) as Box<dyn PacketHandler>)
            } else {
                return Err("error.session_expired".to_string());
            }
        } else {
            None
        };

        Ok((
            ResolveResult {
                player,
                created,
                next_handler,
            },
            old_conn,
        ))
    }

    /// 移除注册（挂起失败/会话超时/踢出时调用）。仅当注册的连接仍是指定连接时移除。
    pub fn remove_if_bound(&self, user_id: i32, conn_id: u64) {
        let mut map = self.players.lock().unwrap();
        if let Some(p) = map.get(&user_id) {
            if p.connection().id() == conn_id {
                map.remove(&user_id);
            }
        }
    }

    pub fn remove(&self, user_id: i32) {
        self.players.lock().unwrap().remove(&user_id);
    }

    pub fn get(&self, user_id: i32) -> Option<Arc<Player>> {
        self.players.lock().unwrap().get(&user_id).cloned()
    }

    pub fn is_online(&self, user_id: i32) -> bool {
        self.get(user_id).map(|p| p.is_online()).unwrap_or(false)
    }

    pub fn online_players(&self) -> Vec<Arc<Player>> {
        self.players
            .lock()
            .unwrap()
            .values()
            .filter(|p| p.is_online())
            .cloned()
            .collect()
    }

    pub fn all_players(&self) -> Vec<Arc<Player>> {
        self.players.lock().unwrap().values().cloned().collect()
    }
}
