//! 玩家注册、连接接管与挂起会话恢复。

use super::{LocalPlayer, Player, local_of};
use crate::log::log_info;
use crate::network::connection::{ConnectionHandle, DisconnectReason};
use crate::phira::UserInfo;
use std::collections::HashMap;
use std::sync::{Arc, Mutex};

/// 恢复挂起会话所需的上下文（由注册表取出，handler 层据此重建 RoomHandler）。
pub struct SuspendedContext {
    pub room: Arc<dyn crate::room::Room>,
}

/// resolve 结果。
pub struct ResolveResult {
    pub player: Arc<dyn Player>,
    pub created: bool,
    /// 恢复（Resume）时取出的挂起房间上下文（handler 层据此构造 RoomHandler）。
    pub suspended: Option<SuspendedContext>,
}

/// 全局玩家注册表（ConcurrentHashMap 等价物）。
pub struct PlayerRegistry {
    players: Mutex<HashMap<i32, Arc<dyn Player>>>,
}

/// 恢复结果类型（对应 Java `ResolveResult.Type`）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ResolveType {
    Create,
    Resume,
    Rebind,
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

    /// 注册或恢复（对应 Java `resolvePlayer` 的泛型体操）：
    ///
    /// - 不存在 → 用 `constructor` 创建。
    /// - 存在且为同一连接 → 直接返回。
    /// - 存在但不同连接 → `resumer` 换绑/接管，并取出挂起会话供恢复。
    ///
    /// `conn` 为 `None` 表示无连接玩家（如 RemotePlayer）；此时 constructor 收到
    /// `None`，resumer 负责定义「接管」语义（无连接玩家可直接返回 `Ok(None)`）。
    ///
    /// 返回 `(ResolveResult, 旧连接)`；旧连接若存在且未关闭，调用方负责踢旧。
    pub fn resolve_player<C, R>(
        &self,
        info: Arc<UserInfo>,
        conn: Option<&ConnectionHandle>,
        constructor: C,
        resumer: R,
    ) -> Result<(ResolveResult, Option<ConnectionHandle>), String>
    where
        C: FnOnce(Arc<UserInfo>, Option<ConnectionHandle>) -> Arc<dyn Player>,
        R: FnOnce(
            &Arc<dyn Player>,
            Option<ConnectionHandle>,
        ) -> Result<Option<ConnectionHandle>, String>,
    {
        let (player, created, old_conn) = {
            let mut map = self.players.lock().unwrap();
            match map.get(&info.id) {
                None => {
                    let player = constructor(info.clone(), conn.cloned());
                    map.insert(info.id, player.clone());
                    (player, true, None)
                }
                Some(existing) => {
                    let same_conn = conn.as_ref().is_some_and(|c| {
                        existing
                            .bound_connection()
                            .is_some_and(|b| b.same_connection(c))
                    });
                    if same_conn {
                        // 同一连接重复认证（不应发生）
                        (existing.clone(), false, None)
                    } else {
                        let old = resumer(existing, conn.cloned())?;
                        (existing.clone(), false, old)
                    }
                }
            }
        };

        Ok((
            ResolveResult {
                player,
                created,
                suspended: None,
            },
            old_conn,
        ))
    }

    /// 默认创建 + 恢复（对应 Java AuthenticateHandler 里的 resolvePlayer 调用）。
    ///
    /// 语义对齐 Java：
    /// - 不存在 → Create。
    /// - 已在线（不同连接且连接存活）→ `error.player_already_online`。
    /// - 有挂起会话 → Resume（取出挂起上下文）。
    /// - 无挂起会话但玩家对象残留 → Rebind（换绑，无房间）。
    ///
    /// 返回 `(ResolveResult, 旧连接)`；旧连接若存在且未关闭，调用方负责踢旧。
    pub async fn resolve_or_resume(
        &self,
        info: Arc<UserInfo>,
        conn: &ConnectionHandle,
    ) -> Result<(ResolveResult, Option<ConnectionHandle>), String> {
        // 已在线且连接存活 → 拒绝重复登录
        let existing = self.players.lock().unwrap().get(&info.id).cloned();
        if let Some(p) = &existing {
            let online = p.is_online()
                && p.bound_connection()
                    .is_none_or(|b| !b.same_connection(conn));
            if online {
                return Err("ERROR_PLAYER_ALREADY_ONLINE".to_string());
            }
        }

        // 尝试恢复挂起会话（take 语义；校验玩家仍在原房间）
        let suspended = match &existing {
            Some(p) => crate::server::with_server_ctx(|ctx| {
                log_info!(&ctx.i18n, "LOG_PLAYER_CHECK_SUSPENDED", ("id", p.id()));
                let taken = ctx.sessions.take_suspended(p.id());
                log_info!(&ctx.i18n, "LOG_PLAYER_TAKE_SUSPENDED", ("id", p.id()));
                if taken.is_some() {
                    log_info!(&ctx.i18n, "LOG_PLAYER_FILTER_CHECK", ("id", p.id()));
                }
                taken
                    .filter(|s| s.room.contains_member(p.id()))
                    .map(|s| SuspendedContext { room: s.room })
            })
            .flatten(),
            None => None,
        };

        let resolve_type = match (&existing, &suspended) {
            (None, _) => ResolveType::Create,
            (Some(_), Some(_)) => ResolveType::Resume,
            (Some(_), None) => ResolveType::Rebind,
        };

        self.resolve_player(
            info,
            Some(conn),
            |info, c| LocalPlayer::new(info, c.expect("LocalPlayer requires a connection")),
            |player, new_conn| {
                // 换绑：取出旧连接，若存活由调用方踢旧
                local_of(player)
                    .ok_or_else(|| "ERROR_PLAYER_TYPE_UNSUPPORTED".to_string())
                    .map(|l| {
                        let c = new_conn.expect("resume requires a connection");
                        Some(l.bind_connection(c))
                    })
            },
        )
        .map(|(mut result, old)| {
            result.suspended = suspended;
            result.created = resolve_type == ResolveType::Create;
            (result, old)
        })
    }

    /// 移除注册（挂起失败/会话超时/踢出时调用）。仅当玩家仍绑定了指定连接时移除
    /// （无连接玩家 `bound_connection()` 为 None → 永远不匹配，由其宿主自行移除）。
    pub fn remove_if_bound(&self, user_id: i32, conn: &ConnectionHandle) {
        let mut map = self.players.lock().unwrap();
        if let Some(p) = map.get(&user_id)
            && p.bound_connection()
                .is_some_and(|b| b.same_connection(conn))
        {
            map.remove(&user_id);
        }
    }

    pub fn remove(&self, user_id: i32) {
        self.players.lock().unwrap().remove(&user_id);
    }

    pub fn get(&self, user_id: i32) -> Option<Arc<dyn Player>> {
        self.players.lock().unwrap().get(&user_id).cloned()
    }

    pub fn is_online(&self, user_id: i32) -> bool {
        self.get(user_id).map(|p| p.is_online()).unwrap_or(false)
    }

    pub fn online_players(&self) -> Vec<Arc<dyn Player>> {
        self.players
            .lock()
            .unwrap()
            .values()
            .filter(|p| p.is_online())
            .cloned()
            .collect()
    }

    pub fn all_players(&self) -> Vec<Arc<dyn Player>> {
        self.players.lock().unwrap().values().cloned().collect()
    }

    /// 断线原因映射（对应 Java PlayerDisconnectEvent.DisconnectReason）。
    pub fn disconnect_reason_of(conn: &ConnectionHandle) -> DisconnectReason {
        conn.disconnect_reason()
    }
}
