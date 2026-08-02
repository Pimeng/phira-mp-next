//! 国际化（第 9 节；对应 Java `I18nService`）。
//!
//! - 内嵌 zh-CN / en-US 两份语言文件（编译期常量）。
//! - 支持从「外置语言目录」（可执行文件旁 `lang/`）加载 `*.json` 覆盖/扩展
//!   （对应 Java 允许资源外语言文件替换）。
//! - 解析顺序：玩家语言 → 服务器默认 → zh-CN 兜底 → key 本身。

use std::collections::HashMap;
use std::path::Path;
use std::sync::RwLock;

// 与 Java 资源 lang/*.json 完全一致（含 system.live_recorder_name）。
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
  "error.player_already_online": "玩家已在线",
  "system.live_recorder_name": "录制状态设置器(请忽略该账号)"
}"#;

const EN_US: &str = r#"{
  "error.invalid_state": "You cannot perform this action in current state",
  "error.permission_denied": "Permission denied",
  "error.room_full": "Room is full",
  "error.room_locked": "Room is locked",
  "error.room_not_found": "Room not found",
  "error.room_already_exists": "Room already exists",
  "error.chart_not_selected": "Chart not selected",
  "error.chart_not_found": "Failed to get chart information",
  "error.chat_not_enabled": "Chat is not enabled in this room",
  "error.record_not_found": "Failed to query record",
  "error.already_in_room": "You are already in a room",
  "error.not_in_room": "You are not in a room",
  "error.not_host": "You are not the host",
  "error.player_not_found": "Player not found",
  "error.session_expired": "Session expired",
  "error.authentication_failed": "Authentication failed",
  "error.logged_in_elsewhere": "Account logged in from another location",
  "error.player_already_online": "Player is already online",
  "system.live_recorder_name": "Live Recorder (Please ignore this account)"
}"#;

pub struct I18nService {
    default_language: RwLock<String>,
    bundles: RwLock<HashMap<String, HashMap<String, String>>>,
}

impl I18nService {
    pub fn new(default_language: impl Into<String>) -> Self {
        let mut bundles = HashMap::new();
        bundles.insert(
            "zh-CN".to_string(),
            serde_json::from_str(ZH_CN).unwrap_or_default(),
        );
        bundles.insert(
            "en-US".to_string(),
            serde_json::from_str(EN_US).unwrap_or_default(),
        );
        let svc = Self {
            default_language: RwLock::new(default_language.into()),
            bundles: RwLock::new(bundles),
        };
        svc.load_external_dir(Path::new("lang"));
        svc
    }

    /// 从外置目录加载 `*.json`（文件名即语言码；键值覆盖内嵌值）。
    pub fn load_external_dir(&self, dir: &Path) {
        let Ok(rd) = std::fs::read_dir(dir) else {
            return;
        };
        for entry in rd.flatten() {
            let path = entry.path();
            if path.extension().and_then(|e| e.to_str()) != Some("json") {
                continue;
            }
            let Some(lang) = path.file_stem().and_then(|s| s.to_str()).map(|s| s.to_string()) else {
                continue;
            };
            let Ok(text) = std::fs::read_to_string(&path) else {
                continue;
            };
            let Ok(map) = serde_json::from_str::<HashMap<String, String>>(&text) else {
                continue;
            };
            self.bundles
                .write()
                .unwrap()
                .entry(lang)
                .or_default()
                .extend(map);
        }
    }

    /// 注册/覆盖一门语言（对应 Java 运行期替换语言文件）。
    pub fn register_language(&self, lang: impl Into<String>, bundle: HashMap<String, String>) {
        self.bundles
            .write()
            .unwrap()
            .entry(lang.into())
            .or_default()
            .extend(bundle);
    }

    pub fn set_default_language(&self, lang: &str) {
        *self.default_language.write().unwrap() = lang.to_string();
    }

    /// 按语言取文案；语言缺失回退默认语言；key 缺失返回 key 本身。
    pub fn message(&self, language: Option<&str>, key: &str) -> String {
        let default_lang = self.default_language.read().unwrap().clone();
        let lang = language.unwrap_or(&default_lang);
        let bundles = self.bundles.read().unwrap();
        bundles
            .get(lang)
            .and_then(|b| b.get(key))
            .or_else(|| bundles.get(&default_lang).and_then(|b| b.get(key)))
            .or_else(|| bundles.get("zh-CN").and_then(|b| b.get(key)))
            .cloned()
            .unwrap_or_else(|| key.to_string())
    }

    pub fn default_message(&self, key: &str) -> String {
        self.message(None, key)
    }

    pub fn message_for(&self, player: &dyn crate::player::Player, key: &str) -> String {
        self.message(player.language().as_deref(), key)
    }
}

/// Trailer 化错误消息（供恢复失败等特殊路径使用）。
pub fn trailer(i18n: &I18nService, lang: Option<&str>, key: &str) -> bytes::Bytes {
    bytes::Bytes::from(i18n.message(lang, key).into_bytes())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn svc() -> I18nService {
        I18nService::new("zh-CN")
    }

    #[test]
    fn default_language_message() {
        assert_eq!(svc().message(None, "error.permission_denied"), "你没有权限");
    }

    #[test]
    fn specified_language_message() {
        assert_eq!(svc().message(Some("en-US"), "error.permission_denied"), "Permission denied");
    }

    #[test]
    fn missing_key_returns_key() {
        assert_eq!(svc().message(None, "non.existent.key"), "non.existent.key");
    }

    #[test]
    fn empty_key_returns_key() {
        assert_eq!(svc().message(None, ""), "");
    }

    #[test]
    fn missing_language_falls_back_to_default() {
        // fr-FR 不存在 → 回退默认 zh-CN
        assert_eq!(svc().message(Some("fr-FR"), "error.permission_denied"), "你没有权限");
    }

    #[test]
    fn change_default_language() {
        let s = svc();
        s.set_default_language("en-US");
        assert_eq!(s.message(None, "error.permission_denied"), "Permission denied");
    }

    #[test]
    fn different_keys_translations() {
        let s = svc();
        assert_eq!(s.message(Some("en-US"), "error.room_full"), "Room is full");
        assert_eq!(s.message(Some("en-US"), "error.room_not_found"), "Room not found");
        assert_eq!(s.message(Some("en-US"), "error.not_in_room"), "You are not in a room");
    }

    #[test]
    fn system_key_both_languages() {
        let s = svc();
        assert_eq!(s.message(Some("en-US"), "system.live_recorder_name"), "Live Recorder (Please ignore this account)");
        assert_eq!(s.message(Some("zh-CN"), "system.live_recorder_name"), "录制状态设置器(请忽略该账号)");
    }

    #[test]
    fn register_language_overrides() {
        let s = svc();
        let mut bundle = HashMap::new();
        bundle.insert("error.permission_denied".to_string(), "自定义拒绝".to_string());
        bundle.insert("custom.key".to_string(), "自定义".to_string());
        s.register_language("zh-CN", bundle);
        // 覆盖内嵌值 + 新增 key
        assert_eq!(s.message(Some("zh-CN"), "error.permission_denied"), "自定义拒绝");
        assert_eq!(s.message(Some("zh-CN"), "custom.key"), "自定义");
    }

    #[test]
    fn register_new_language() {
        let s = svc();
        let mut bundle = HashMap::new();
        bundle.insert("error.permission_denied".to_string(), "日本語拒否".to_string());
        s.register_language("ja-JP", bundle);
        assert_eq!(s.message(Some("ja-JP"), "error.permission_denied"), "日本語拒否");
        // 该语言缺 key → 回退默认 zh-CN
        assert_eq!(s.message(Some("ja-JP"), "error.room_full"), "房间已满");
    }

    #[test]
    fn load_external_dir_overrides() {
        let s = svc();
        let dir = std::env::temp_dir().join(format!("i18n_test_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("fr-FR.json"),
            r#"{"error.permission_denied":"Refusé"}"#,
        )
        .unwrap();
        s.load_external_dir(&dir);
        assert_eq!(s.message(Some("fr-FR"), "error.permission_denied"), "Refusé");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
