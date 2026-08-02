//! 国际化（第 9 节；对应 Java `I18nService`）。
//!
//! - 构建期内嵌 zh-CN / en-US 两份 FTL（`include_str!` 自 `lang/`），
//!   保证无外置文件时文案也可用。
//! - 启动时额外加载「语言目录」（可执行文件旁 `lang/`）下的 `*.ftl`，
//!   文件名即语言码（如 `lang/zh-CN.ftl`），同名语言覆盖内嵌值
//!   （对应 Java 资源 lang/*.ftl，支持外置语言文件替换）。
//! - 解析顺序：玩家语言 → 服务器默认 → zh-CN 兜底 → key 本身。
//! - FTL 解析/格式化使用 `fluent` 依赖库（`FluentResource` + `FluentBundle`），
//!   不自行实现解析/插值。文案支持 FTL 变量引用（如 `{ $name }`），
//!   渲染时通过 [`I18nService::message_with_args`] 传入参数。
//! - key 直接使用 FTL 消息 id（如 `ERROR_INVALID_STATE`），不依赖属性/点号。

use std::collections::HashMap;
use std::path::Path;
use std::sync::RwLock;

use fluent::FluentArgs;
use fluent::FluentResource;
use fluent::concurrent::FluentBundle;
use unic_langid::LanguageIdentifier;

// 构建期内嵌语言文件（与 lang/ 目录内容一致；外置文件可覆盖）。
const ZH_CN: &str = include_str!("../lang/zh-CN.ftl");
const EN_US: &str = include_str!("../lang/en-US.ftl");

pub struct I18nService {
    default_language: RwLock<String>,
    bundles: RwLock<HashMap<String, FluentBundle<FluentResource>>>,
}

impl I18nService {
    pub fn new(default_language: impl Into<String>) -> Self {
        let svc = Self {
            default_language: RwLock::new(default_language.into()),
            bundles: RwLock::new(HashMap::new()),
        };
        // 内嵌默认语言（构建期），外置目录同名语言会覆盖。
        svc.register_language("zh-CN", ZH_CN);
        svc.register_language("en-US", EN_US);
        svc.load_external_dir(Path::new("lang"));
        svc
    }

    /// 加载语言目录下全部 `*.ftl`（文件名即语言码；如 `lang/zh-CN.ftl`）。
    /// 单个文件解析失败则跳过（与旧 JSON 行为一致）。
    pub fn load_external_dir(&self, dir: &Path) {
        let Ok(rd) = std::fs::read_dir(dir) else {
            return;
        };
        for entry in rd.flatten() {
            let path = entry.path();
            if path.extension().and_then(|e| e.to_str()) != Some("ftl") {
                continue;
            }
            let Some(lang) = path
                .file_stem()
                .and_then(|s| s.to_str())
                .map(|s| s.to_string())
            else {
                continue;
            };
            let Ok(text) = std::fs::read_to_string(&path) else {
                continue;
            };
            let Ok(res) = FluentResource::try_new(text) else {
                continue;
            };
            self.add_resource(&lang, res);
        }
    }

    /// 注册/覆盖一门语言（对应 Java 运行期替换语言文件；入参为 FTL 源码）。
    pub fn register_language(&self, lang: impl Into<String>, ftl: &str) {
        let lang = lang.into();
        // 传入 FTL 解析失败属于 bug，直接 panic 暴露。
        let res = FluentResource::try_new(ftl.to_string())
            .unwrap_or_else(|(_, errs)| panic!("invalid FTL for `{lang}`: {errs:?}"));
        self.add_resource(&lang, res);
    }

    /// 将解析好的资源并入对应语言的 bundle；同 id 消息被新资源整体覆盖
    /// （`add_resource_overriding` 即覆盖语义）。
    fn add_resource(&self, lang: &str, res: FluentResource) {
        let langid = lang
            .parse::<LanguageIdentifier>()
            .unwrap_or_else(|_| "und".parse().unwrap());
        let mut bundles = self.bundles.write().unwrap();
        let bundle = bundles
            .entry(lang.to_string())
            .or_insert_with(|| FluentBundle::new_concurrent(vec![langid]));
        // 关闭双向文本隔离标记（\u{2068}/\u{2069}），保持纯文本插值输出。
        bundle.set_use_isolating(false);
        bundle.add_resource_overriding(res);
    }

    pub fn set_default_language(&self, lang: &str) {
        *self.default_language.write().unwrap() = lang.to_string();
    }

    /// 按语言取文案（无参数版）；语言缺失回退默认语言；key 缺失返回 key 本身。
    pub fn message(&self, language: Option<&str>, key: &str) -> String {
        self.message_with_args(language, key, &[])
    }

    /// 按语言取文案并代入 FTL 变量参数（对应 `{ $name }` 占位符）。
    /// 回退顺序：玩家语言 → 默认语言 → zh-CN → key 本身。
    pub fn message_with_args(
        &self,
        language: Option<&str>,
        key: &str,
        args: &[(&str, &str)],
    ) -> String {
        let default_lang = self.default_language.read().unwrap().clone();
        let lang = language.unwrap_or(&default_lang);
        let mut fargs = FluentArgs::new();
        for (name, value) in args {
            fargs.set((*name).to_string(), (*value).to_string());
        }
        let bundles = self.bundles.read().unwrap();
        for candidate in [lang, default_lang.as_str(), "zh-CN"] {
            let Some(bundle) = bundles.get(candidate) else {
                continue;
            };
            let Some(msg) = bundle.get_message(key) else {
                continue;
            };
            let Some(pattern) = msg.value() else {
                continue;
            };
            let mut errors = vec![];
            return bundle
                .format_pattern(pattern, Some(&fargs), &mut errors)
                .to_string();
        }
        key.to_string()
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
        assert_eq!(svc().message(None, "ERROR_PERMISSION_DENIED"), "你没有权限");
    }

    #[test]
    fn specified_language_message() {
        assert_eq!(
            svc().message(Some("en-US"), "ERROR_PERMISSION_DENIED"),
            "Permission denied"
        );
    }

    #[test]
    fn missing_key_returns_key() {
        assert_eq!(svc().message(None, "NON_EXISTENT_KEY"), "NON_EXISTENT_KEY");
    }

    #[test]
    fn empty_key_returns_key() {
        assert_eq!(svc().message(None, ""), "");
    }

    #[test]
    fn missing_language_falls_back_to_default() {
        // fr-FR 不存在 → 回退默认 zh-CN
        assert_eq!(
            svc().message(Some("fr-FR"), "ERROR_PERMISSION_DENIED"),
            "你没有权限"
        );
    }

    #[test]
    fn change_default_language() {
        let s = svc();
        s.set_default_language("en-US");
        assert_eq!(
            s.message(None, "ERROR_PERMISSION_DENIED"),
            "Permission denied"
        );
    }

    #[test]
    fn different_keys_translations() {
        let s = svc();
        assert_eq!(s.message(Some("en-US"), "ERROR_ROOM_FULL"), "Room is full");
        assert_eq!(
            s.message(Some("en-US"), "ERROR_ROOM_NOT_FOUND"),
            "Room not found"
        );
        assert_eq!(
            s.message(Some("en-US"), "ERROR_NOT_IN_ROOM"),
            "You are not in a room"
        );
    }

    #[test]
    fn system_key_both_languages() {
        let s = svc();
        assert_eq!(
            s.message(Some("en-US"), "SYSTEM_LIVE_RECORDER_NAME"),
            "Live Recorder (Please ignore this account)"
        );
        assert_eq!(
            s.message(Some("zh-CN"), "SYSTEM_LIVE_RECORDER_NAME"),
            "录制状态设置器(请忽略该账号)"
        );
    }

    #[test]
    fn register_language_overrides() {
        let s = svc();
        s.register_language(
            "zh-CN",
            r#"
ERROR_PERMISSION_DENIED = 自定义拒绝
CUSTOM_KEY = 自定义
"#,
        );
        // 覆盖内嵌值 + 新增 key
        assert_eq!(
            s.message(Some("zh-CN"), "ERROR_PERMISSION_DENIED"),
            "自定义拒绝"
        );
        assert_eq!(s.message(Some("zh-CN"), "CUSTOM_KEY"), "自定义");
    }

    #[test]
    fn register_new_language() {
        let s = svc();
        s.register_language(
            "ja-JP",
            r#"
ERROR_PERMISSION_DENIED = 日本語拒否
"#,
        );
        assert_eq!(
            s.message(Some("ja-JP"), "ERROR_PERMISSION_DENIED"),
            "日本語拒否"
        );
        // 该语言缺 key → 回退默认 zh-CN
        assert_eq!(s.message(Some("ja-JP"), "ERROR_ROOM_FULL"), "房间已满");
    }

    #[test]
    fn load_external_dir_overrides() {
        let s = svc();
        let dir = std::env::temp_dir().join(format!("i18n_test_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("fr-FR.ftl"), "ERROR_PERMISSION_DENIED = Refusé\n").unwrap();
        s.load_external_dir(&dir);
        assert_eq!(
            s.message(Some("fr-FR"), "ERROR_PERMISSION_DENIED"),
            "Refusé"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn placeholder_in_message_value() {
        let s = svc();
        s.register_language(
            "zh-CN",
            r#"
welcome = 欢迎，{ $name }！
"#,
        );
        assert_eq!(
            s.message_with_args(Some("zh-CN"), "welcome", &[("name", "小明")]),
            "欢迎，小明！"
        );
    }

    #[test]
    fn placeholder_in_flat_key() {
        let s = svc();
        s.register_language(
            "zh-CN",
            r#"
GREET_HELLO = 你好，{ $name }！
"#,
        );
        // 扁平 key + 占位符
        assert_eq!(
            s.message_with_args(Some("zh-CN"), "GREET_HELLO", &[("name", "小明")]),
            "你好，小明！"
        );
    }

    #[test]
    fn placeholder_unicode_value() {
        let s = svc();
        s.register_language(
            "zh-CN",
            r#"
who = 当前玩家：{ $player }
"#,
        );
        assert_eq!(
            s.message_with_args(Some("zh-CN"), "who", &[("player", "阿尔法")]),
            "当前玩家：阿尔法"
        );
    }
}
