//! 封禁管理（控制台命令 `ban`/`unban`/`banlist`/`banroom`/`unbanroom` 的数据源）。
//!
//! - **全局封禁**：封禁后该用户无法登录；已在线者立即踢出。
//! - **房间封禁**：禁止该用户进入指定房间（已在房间内的由命令层移出）。
//!
//! 封禁表存于内存（服务重启即清空）。如需持久化/审计，可订阅
//! `player.pre_login`（封禁检查）并在外部自行持久化本管理器状态。

use std::collections::HashMap;
use std::sync::RwLock;
use std::time::Instant;

/// 封禁条目。
#[derive(Debug, Clone)]
pub struct BanEntry {
    pub banned_at: Instant,
    pub reason: Option<String>,
}

/// 封禁管理器（全局封禁 + 房间封禁）。
pub struct BanManager {
    /// 全局封禁：userId → 条目。
    bans: RwLock<HashMap<i32, BanEntry>>,
    /// 房间封禁：roomId → (userId → 条目)。
    room_bans: RwLock<HashMap<String, HashMap<i32, BanEntry>>>,
}

impl Default for BanManager {
    fn default() -> Self {
        Self::new()
    }
}

impl BanManager {
    pub fn new() -> Self {
        Self {
            bans: RwLock::new(HashMap::new()),
            room_bans: RwLock::new(HashMap::new()),
        }
    }

    // ---------------- 全局封禁 ----------------

    /// 封禁用户；返回是否为新封禁（重复封禁返回 false 并更新原因）。
    pub fn ban(&self, user_id: i32, reason: Option<String>) -> bool {
        let mut bans = self.bans.write().unwrap();
        let is_new = !bans.contains_key(&user_id);
        bans.insert(
            user_id,
            BanEntry {
                banned_at: Instant::now(),
                reason,
            },
        );
        is_new
    }

    /// 解封用户；返回是否确实解除了封禁。
    pub fn unban(&self, user_id: i32) -> bool {
        self.bans.write().unwrap().remove(&user_id).is_some()
    }

    pub fn is_banned(&self, user_id: i32) -> bool {
        self.bans.read().unwrap().contains_key(&user_id)
    }

    /// 封禁列表：(userId, reason)。
    pub fn ban_list(&self) -> Vec<(i32, Option<String>)> {
        self.bans
            .read()
            .unwrap()
            .iter()
            .map(|(&id, e)| (id, e.reason.clone()))
            .collect()
    }

    // ---------------- 房间封禁 ----------------

    /// 禁止用户进入指定房间；返回是否为新封禁。
    pub fn ban_room(&self, room_id: &str, user_id: i32) -> bool {
        let mut map = self.room_bans.write().unwrap();
        let entry = map.entry(room_id.to_string()).or_default();
        let is_new = !entry.contains_key(&user_id);
        entry.insert(
            user_id,
            BanEntry {
                banned_at: Instant::now(),
                reason: None,
            },
        );
        is_new
    }

    /// 解除房间禁入；返回是否确实解除了封禁。
    pub fn unban_room(&self, room_id: &str, user_id: i32) -> bool {
        let mut map = self.room_bans.write().unwrap();
        let removed = map
            .get_mut(room_id)
            .map(|e| e.remove(&user_id).is_some())
            .unwrap_or(false);
        if map.get(room_id).map_or(false, |e| e.is_empty()) {
            map.remove(room_id);
        }
        removed
    }

    pub fn is_room_banned(&self, room_id: &str, user_id: i32) -> bool {
        self.room_bans
            .read()
            .unwrap()
            .get(room_id)
            .map_or(false, |e| e.contains_key(&user_id))
    }

    /// 指定房间的封禁用户列表。
    pub fn room_ban_list(&self, room_id: &str) -> Vec<i32> {
        self.room_bans
            .read()
            .unwrap()
            .get(room_id)
            .map(|e| e.keys().copied().collect())
            .unwrap_or_default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn global_ban_unban() {
        let m = BanManager::new();
        assert!(!m.is_banned(1));
        assert!(m.ban(1, None));
        assert!(!m.ban(1, Some("spam".into())), "重复封禁不是新封禁");
        assert!(m.is_banned(1));
        assert_eq!(m.ban_list().len(), 1);
        assert!(m.unban(1));
        assert!(!m.unban(1), "重复解封返回 false");
        assert!(!m.is_banned(1));
    }

    #[test]
    fn room_ban_unban() {
        let m = BanManager::new();
        assert!(!m.is_room_banned("R1", 1));
        assert!(m.ban_room("R1", 1));
        assert!(!m.ban_room("R1", 1));
        assert!(m.is_room_banned("R1", 1));
        assert!(!m.is_room_banned("R2", 1), "其他房间不受影响");
        assert_eq!(m.room_ban_list("R1"), vec![1]);
        assert!(m.unban_room("R1", 1));
        assert!(!m.is_room_banned("R1", 1));
        assert!(m.room_ban_list("R1").is_empty());
        // 空表自动清理
        assert!(m.room_bans.read().unwrap().is_empty());
    }
}
