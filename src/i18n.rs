//! 国际化（第 9 节）。语言文件内嵌，解析顺序：玩家语言 → 服务器默认。

use std::collections::HashMap;
use std::sync::RwLock;

const ZH_CN: &str = r#"{
  "error.invalid_state": "你不能在当前状态执行这个操作",
  "error.permission_denied": "你没有权限",
  "error.room_full": "房间已满",
  "error.room_locked": "房间已锁定",
  "error.room_not_found": "房间不存在",
  "error.room_already_exists": "房间已存在",
  "error.chart_not_selected": "未选择谱面",
  "error.chart_not_found": "谱面信息获取失败",
  "error.chat_not_enabled": "房间未启用聊天",
  "error.record_not_found": "查询记录失败",
  "error.already_in_room": "你已经在房间中",
  "error.not_in_room": "你不在房间中",
  "error.not_host": "你不是房主",
  "error.player_not_found": "玩家不存在",
  "error.session_expired": "会话已过期",
  "error.authentication_failed": "认证失败",
  "error.logged_in_elsewhere": "账号在其他地方登录",
  "error.player_already_online": "玩家已在线"
}"#;

const EN_US: &str = r#"{
  "error.invalid_state": "You cannot perform this operation in the current state",
  "error.permission_denied": "Permission denied",
  "error.room_full": "Room is full",
  "error.room_locked": "Room is locked",
  "error.room_not_found": "Room not found",
  "error.room_already_exists": "Room already exists",
  "error.chart_not_selected": "No chart selected",
  "error.chart_not_found": "Failed to fetch chart info",
  "error.chat_not_enabled": "Chat is not enabled in this room",
  "error.record_not_found": "Failed to fetch record",
  "error.already_in_room": "You are already in a room",
  "error.not_in_room": "You are not in a room",
  "error.not_host": "You are not the host",
  "error.player_not_found": "Player not found",
  "error.session_expired": "Session expired",
  "error.authentication_failed": "Authentication failed",
  "error.logged_in_elsewhere": "Account logged in elsewhere",
  "error.player_already_online": "Player already online"
}"#;

pub struct I18nService {
    default_language: RwLock<String>,
    bundles: HashMap<String, HashMap<String, String>>,
}

impl I18nService {
    pub fn new(default_language: impl Into<String>) -> Self {
        let mut bundles = HashMap::new();
        bundles.insert("zh-CN".to_string(), serde_json::from_str(ZH_CN).unwrap_or_default());
        bundles.insert("en-US".to_string(), serde_json::from_str(EN_US).unwrap_or_default());
        Self {
            default_language: RwLock::new(default_language.into()),
            bundles,
        }
    }

    pub fn set_default_language(&self, lang: &str) {
        *self.default_language.write().unwrap() = lang.to_string();
    }

    /// 按语言取文案；语言缺失回退默认语言；key 缺失返回 key 本身。
    pub fn message(&self, language: Option<&str>, key: &str) -> String {
        let default_lang = self.default_language.read().unwrap().clone();
        let lang = language.unwrap_or(&default_lang);
        self.bundles
            .get(lang)
            .and_then(|b| b.get(key))
            .or_else(|| self.bundles.get(&default_lang).and_then(|b| b.get(key)))
            .or_else(|| self.bundles.get("zh-CN").and_then(|b| b.get(key)))
            .cloned()
            .unwrap_or_else(|| key.to_string())
    }

    pub fn default_message(&self, key: &str) -> String {
        self.message(None, key)
    }

    pub fn message_for(&self, player: &crate::player::Player, key: &str) -> String {
        self.message(player.language().as_deref(), key)
    }
}
