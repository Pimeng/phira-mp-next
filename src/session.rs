//! 会话挂起/恢复（5.3 节）：「掉线不掉房」核心。
//!
//! 对应 Java `LocalSessionManager`：
//! - `suspend(player, remover)`：仅当 handler 是 SuspendableRoomHolder 时挂起；
//!   清理现场（WaitForReady→cancelReady；Playing→abort）；超时 forceLeave + remover。
//! - `resume(player, connection)`：取出挂起会话，校验玩家仍在房间，返回旧 handler。

use crate::player::Player;
use crate::room::Room;
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;

/// 挂起中的房间会话。
pub struct SuspendedRoomSession {
    pub user_id: i32,
    pub room: Arc<dyn Room>,
    pub suspended_at: std::time::Instant,
    /// 挂起代次：同一玩家重复挂起时递增，旧超时任务据此失效。
    generation: u64,
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

/// 挂起失败（玩家不在可挂起的房间状态 / 已离线）。
#[derive(Debug)]
pub struct SuspendFailed;

/// 恢复失败（会话过期 / 玩家已不在原房间）。
#[derive(Debug)]
pub struct ResumeFailed(pub &'static str);

impl SessionManager {
    pub fn new() -> Self {
        Self {
            sessions: Mutex::new(HashMap::new()),
            timeout: Mutex::new(Duration::from_secs(300)),
        }
    }

    pub fn set_timeout(&self, timeout: Duration) {
        *self.timeout.lock().unwrap() = timeout;
    }

    pub fn timeout(&self) -> Duration {
        *self.timeout.lock().unwrap()
    }

    /// 挂起会话（对应 Java `LocalSessionManager.suspend`）。
    ///
    /// - 清理现场并广播（锁外）。
    /// - 启动超时任务：超时 → forceLeave + `remover`（移除玩家注册）。
    pub async fn suspend(
        self: &Arc<Self>,
        player: Arc<dyn Player>,
        room: Arc<dyn Room>,
        remover: impl FnOnce() + Send + 'static,
    ) -> Result<(), SuspendFailed> {
        let user_id = player.id();

        // 清理对局现场（WaitForReady→cancelReady；Playing→abort）
        let plan = room.cleanup_for_suspend(user_id);
        crate::room::send_broadcasts(plan).await;

        let generation = {
            let mut map = self.sessions.lock().unwrap();
            let next_gen = map.get(&user_id).map(|s| s.generation + 1).unwrap_or(0);
            map.insert(
                user_id,
                SuspendedRoomSession {
                    user_id,
                    room: room.clone(),
                    suspended_at: std::time::Instant::now(),
                    generation: next_gen,
                },
            );
            next_gen
        };

        let timeout = self.timeout();
        let this = self.clone();
        tokio::spawn(async move {
            tokio::time::sleep(timeout).await;
            // take 语义 + 代次校验：已 resume 或被新一次挂起取代的会话不会误杀
            let Some(session) = this.take_suspended_if_generation(user_id, generation) else {
                return;
            };
            tracing::info!("Session timeout, force leave (user {user_id})");
            if session.room.contains_member(user_id) {
                let (_l, plan, _d) = session.room.leave(user_id);
                crate::room::send_broadcasts(plan).await;
            }
            // PlayerSessionTimeoutEvent
            crate::server::with_server_ctx(|ctx| {
                let player_opt = ctx.players.get(user_id);
                if let Some(player) = player_opt {
                    let ev = crate::events::PlayerSessionTimeoutEvent {
                        player,
                        room: session.room.clone(),
                    };
                    let bus = ctx.events.clone();
                    tokio::spawn(async move {
                        bus.post(crate::events::PLAYER_SESSION_TIMEOUT, ev).await;
                    });
                }
            });
            remover();
            // 玩家已从注册表移除 → PLAYER_UNREGISTER（先取 Arc 再移除）
            let removed = crate::server::with_server_ctx(|ctx| ctx.players.get(user_id)).flatten();
            if let Some(player) = removed {
                crate::server::with_server_ctx(|ctx| {
                    let bus = ctx.events.clone();
                    let player = player.clone();
                    tokio::spawn(async move {
                        bus.post(
                            crate::events::PLAYER_UNREGISTER,
                            crate::events::PlayerUnregisterEvent { player },
                        )
                        .await;
                    });
                });
            }
        });
        Ok(())
    }

    /// 恢复挂起会话（对应 Java `LocalSessionManager.resume`）。
    /// take 语义；校验玩家仍在原房间。
    pub fn resume(&self, player: &Arc<dyn Player>) -> Result<SuspendedRoomSession, ResumeFailed> {
        match self.take_suspended(player.id()) {
            Some(s) if s.room.contains_member(player.id()) => Ok(s),
            Some(_) => Err(ResumeFailed("ERROR_SESSION_EXPIRED")),
            None => Err(ResumeFailed("ERROR_SESSION_NOT_FOUND")),
        }
    }

    /// 取出挂起会话（注册表/超时用）。take 语义。
    pub fn take_suspended(&self, user_id: i32) -> Option<SuspendedRoomSession> {
        self.sessions.lock().unwrap().remove(&user_id)
    }

    /// 仅当当前会话代次匹配时取出（重复挂起时旧超时任务失效）。
    fn take_suspended_if_generation(&self, user_id: i32, generation: u64) -> Option<SuspendedRoomSession> {
        let mut map = self.sessions.lock().unwrap();
        match map.get(&user_id) {
            Some(s) if s.generation == generation => map.remove(&user_id),
            _ => None,
        }
    }

    pub fn has_suspended(&self, user_id: i32) -> bool {
        self.sessions.lock().unwrap().contains_key(&user_id)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::network::connection::ConnectionHandle;
    use crate::phira::UserInfo;
    use crate::player::{LocalPlayer, PlayerRegistry};
    use crate::room::{LocalRoom, RoomSetting};

    fn make_player(id: i32) -> Arc<dyn Player> {
        let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
        std::mem::forget(rx); // 保持 tx 配对端存活 → is_online() == true
        let conn = ConnectionHandle::new_for_test(tx);
        LocalPlayer::new(
            Arc::new(UserInfo { id, name: format!("P{id}"), ..Default::default() }),
            conn,
        )
    }

    #[tokio::test]
    async fn suspend_and_resume_roundtrip() {
        let sm = Arc::new(SessionManager::new());
        let player = make_player(1);
        let room = LocalRoom::new("R", RoomSetting::default(), || {});
        room.join(player.clone(), false).unwrap();

        sm.suspend(player.clone(), room.clone(), || {}).await.unwrap();
        assert!(sm.has_suspended(1));

        let s = sm.take_suspended(1).unwrap();
        assert!(s.room.contains_member(1));
    }

    #[tokio::test]
    async fn resolve_or_resume_returns_suspended_room() {
        // 需要全局 ctx（sessions 从全局取）；构造最小 ServerContext。
        let ctx = crate::server::ServerContext::new(crate::server::ServerArgs {
            port: 0,
            host: "127.0.0.1".into(),
            proxy_protocol: false,
            http_port: 0,
            http_host: "127.0.0.1".into(),
            language: "zh-CN".into(),
            session_timeout: 300,
            phira_api: "http://127.0.0.1:1/".into(),
            record_dir: None,
        });
        let registry = PlayerRegistry::new();
        let player = make_player(7);
        let room = LocalRoom::new("RS", RoomSetting::default(), || {});
        room.join(player.clone(), false).unwrap();

        // 注册玩家（模拟首次登录）；drop 旧连接的接收端使其 is_closed
        let info = player.user_info();
        let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
        let conn1 = ConnectionHandle::new_for_test(tx);
        let _ = registry
            .resolve_player(
                info.clone(),
                Some(&conn1),
                |i, c| LocalPlayer::new(i, c.expect("test conn")),
                |_p, _c| Ok(None),
            )
            .unwrap();
        drop(rx);
        drop(conn1);

        // 挂起
        ctx.sessions.suspend(player.clone(), room.clone(), || {}).await.unwrap();
        assert!(ctx.sessions.has_suspended(7));

        // 重连（新连接）→ 应恢复挂起房间
        let (tx2, _rx2) = tokio::sync::mpsc::unbounded_channel();
        let conn2 = ConnectionHandle::new_for_test(tx2);
        let (result, _old) = registry
            .resolve_or_resume(info.clone(), &conn2)
            .await
            .expect("resume should succeed");
        assert!(result.suspended.is_some(), "should carry suspended room");
        assert_eq!(result.suspended.unwrap().room.id(), "RS");
    }

    #[tokio::test]
    async fn suspend_creates_session_with_timeout() {
        let sm = Arc::new(SessionManager::new());
        let player = make_player(1);
        let room = LocalRoom::new("R", RoomSetting::default(), || {});
        room.join(player.clone(), false).unwrap();
        let called = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let c = called.clone();
        sm.suspend(player.clone(), room.clone(), move || c.store(true, std::sync::atomic::Ordering::SeqCst))
            .await
            .unwrap();
        // 挂起后 remover 不应立即调用（超时未到）
        assert!(!called.load(std::sync::atomic::Ordering::SeqCst));
        assert!(sm.has_suspended(1));
    }

    #[tokio::test]
    async fn resume_cancels_timeout() {
        let sm = Arc::new(SessionManager::new());
        sm.set_timeout(Duration::from_millis(80));
        let player = make_player(1);
        let room = LocalRoom::new("R", RoomSetting::default(), || {});
        room.join(player.clone(), false).unwrap();
        let called = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let c = called.clone();
        sm.suspend(player.clone(), room.clone(), move || c.store(true, std::sync::atomic::Ordering::SeqCst))
            .await
            .unwrap();
        // resume 取出（take 语义）→ 超时任务 take 失败 → remover 不触发
        let s = sm.resume(&player).expect("resume should succeed");
        assert!(s.room.contains_member(1));
        tokio::time::sleep(Duration::from_millis(200)).await;
        assert!(!called.load(std::sync::atomic::Ordering::SeqCst), "resume should cancel timeout");
    }

    #[tokio::test]
    async fn timeout_forces_leave_and_calls_remover() {
        let sm = Arc::new(SessionManager::new());
        sm.set_timeout(Duration::from_millis(50));
        let player = make_player(1);
        let room = LocalRoom::new("R", RoomSetting::default(), || {});
        room.join(player.clone(), false).unwrap();
        let called = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let c = called.clone();
        sm.suspend(player.clone(), room.clone(), move || c.store(true, std::sync::atomic::Ordering::SeqCst))
            .await
            .unwrap();
        // 等超时触发
        for _ in 0..50 {
            if called.load(std::sync::atomic::Ordering::SeqCst) {
                break;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        assert!(called.load(std::sync::atomic::Ordering::SeqCst), "timeout should call remover");
        assert!(!room.contains_member(1), "timeout should force leave");
    }

    #[tokio::test]
    async fn duplicate_suspend_cancels_old_timeout() {
        let sm = Arc::new(SessionManager::new());
        sm.set_timeout(Duration::from_millis(60));
        let player = make_player(1);
        let room = LocalRoom::new("R", RoomSetting::default(), || {});
        room.join(player.clone(), false).unwrap();
        let first = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let second = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let f = first.clone();
        sm.suspend(player.clone(), room.clone(), move || f.store(true, std::sync::atomic::Ordering::SeqCst))
            .await
            .unwrap();
        let s = second.clone();
        sm.suspend(player.clone(), room.clone(), move || s.store(true, std::sync::atomic::Ordering::SeqCst))
            .await
            .unwrap();
        // 第一次挂起被第二次取代：第一次 remover 不触发，第二次触发
        tokio::time::sleep(Duration::from_millis(90)).await;
        assert!(!first.load(std::sync::atomic::Ordering::SeqCst), "old suspend timeout should be cancelled");
        for _ in 0..30 {
            if second.load(std::sync::atomic::Ordering::SeqCst) {
                break;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        assert!(second.load(std::sync::atomic::Ordering::SeqCst), "second suspend should time out");
    }

    #[tokio::test]
    async fn resume_without_suspended_session_fails() {
        let sm = SessionManager::new();
        let player = make_player(1);
        assert!(sm.resume(&player).is_err());
    }

    #[tokio::test]
    async fn resume_after_player_left_room_fails() {
        let sm = Arc::new(SessionManager::new());
        let player = make_player(1);
        let room = LocalRoom::new("R", RoomSetting::default(), || {});
        room.join(player.clone(), false).unwrap();
        sm.suspend(player.clone(), room.clone(), || {}).await.unwrap();
        // 玩家离开房间（会话仍挂起）→ resume 校验失败
        room.leave(1);
        assert!(sm.resume(&player).is_err());
    }

    #[tokio::test]
    async fn suspend_cleans_up_wait_for_ready() {
        let sm = Arc::new(SessionManager::new());
        let p1 = make_player(1);
        let p2 = make_player(2);
        let room = LocalRoom::new("R", RoomSetting::default(), || {});
        room.join(p1.clone(), false).unwrap();
        room.join(p2.clone(), false).unwrap();
        room.commit_select_chart(1, 1, "C".into()).unwrap();
        room.require_start(1).unwrap(); // → WaitForReady, ready={1}
        room.ready(2).unwrap(); // ready={1,2} 但 online={1,2} → 应开局
        // 若在 Playing，p2 挂起 → cleanup abort p2
        sm.suspend(p2.clone(), room.clone(), || {}).await.unwrap();
        assert!(sm.has_suspended(2));
    }
}
