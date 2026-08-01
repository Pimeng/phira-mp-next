//! 会话挂起/恢复（5.3 节）：「掉线不掉房」核心。

use crate::room::Room;
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;

/// 挂起中的房间会话。
pub struct SuspendedRoomSession {
    pub user_id: i32,
    pub room: Arc<Room>,
    pub suspended_at: std::time::Instant,
}

pub struct SessionManager {
    sessions: Mutex<HashMap<i32, SuspendedRoomSession>>,
    timeout: Mutex<Duration>,
}

impl Default for SessionManager {
    fn default() -> Self {
        Self::new()
    }
}

impl SessionManager {
    pub fn new() -> Self {
        Self {
            sessions: Mutex::new(HashMap::new()),
            timeout: Mutex::new(Duration::from_secs(300)), // 默认 5 分钟
        }
    }

    pub fn set_timeout(&self, timeout: Duration) {
        *self.timeout.lock().unwrap() = timeout;
    }

    pub fn timeout(&self) -> Duration {
        *self.timeout.lock().unwrap()
    }

    /// 挂起会话并启动超时任务（超时 → forceLeave）。
    pub fn suspend(
        self: &Arc<Self>,
        user_id: i32,
        room: Arc<Room>,
        player: Arc<crate::player::Player>,
    ) {
        {
            let mut map = self.sessions.lock().unwrap();
            map.insert(
                user_id,
                SuspendedRoomSession {
                    user_id,
                    room: room.clone(),
                    suspended_at: std::time::Instant::now(),
                },
            );
        }
        let timeout = self.timeout();
        let this = self.clone();
        tokio::spawn(async move {
            tokio::time::sleep(timeout).await;
            // take 语义：已 resume 的会话不会误杀
            if let Some(session) = this.take_suspended(user_id) {
                tracing::info!(user_id, "session timeout, force leave room");
                // 仍在房间 → 离开
                if session.room.contains_member(user_id) {
                    let (_left, broadcasts, _destroyed) = session.room.leave(user_id);
                    for (target, packet) in broadcasts {
                        target.send(packet).await;
                    }
                }
                // 移除玩家注册（此时玩家已离线）
                crate::server::with_server_ctx(|ctx| ctx.players.remove(user_id));
                drop(player);
            }
        });
    }

    /// 取出挂起会话（resume 或超时用）。take 语义。
    pub fn take_suspended(&self, user_id: i32) -> Option<SuspendedRoomSession> {
        self.sessions.lock().unwrap().remove(&user_id)
    }

    pub fn has_suspended(&self, user_id: i32) -> bool {
        self.sessions.lock().unwrap().contains_key(&user_id)
    }
}
