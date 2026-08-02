//! 多语言日志系统。
//!
//! 日志消息模板定义在 `lang/*.ftl`（`LOG_*` 键），通过 [`I18nService`]
//! 按回退链渲染：指定语言 → 服务器默认语言（`--language`）→ zh-CN → key 本身。
//! 实际输出仍走 tracing（terminal-console 上色、日志不打断输入行），
//! 统一 target 为 `log`，与硬编码日志区分。
//!
//! 宏形式（与 tracing 风格一致）：
//! - `log_info!(&i18n, "KEY", ("name", value), ...)` —— 服务器默认语言渲染；
//! - `log_info!(&i18n, "en-US", "KEY", ("name", value), ...)` —— 指定语言渲染；
//! - `log_info_global!("KEY", ...)` / `log_info_global!("en-US", "KEY", ...)` ——
//!   无本地 ctx 时经全局 `ServerContext` 取 i18n（不存在则输出 key 本身）；
//! - `log_raw(level, text)` —— 输出已渲染的多行文本（命令等结构化输出）。
//! 参数对 `("name", value)` 对应 FTL 变量 `{ $name }`。
#![allow(unused_macros)] // 宏是 crate 内日志 API（log_error!/log_warn!/log_debug!/log_trace!），允许当前无调用点

use crate::i18n::I18nService;
use tracing::Level;

/// 按回退链渲染多语言日志消息（无服务时输出 key 本身）。
pub fn render_message(
    svc: Option<&I18nService>,
    lang: Option<&str>,
    key: &str,
    args: &[(&str, &str)],
) -> String {
    match svc {
        Some(svc) => svc.message_with_args(lang, key, args),
        None => key.to_string(),
    }
}

/// 经全局 `ServerContext` 取 i18n 渲染（无 ctx 时输出 key 本身，便于测试/无服务环境）。
pub fn render_global_message(lang: Option<&str>, key: &str, args: &[(&str, &str)]) -> String {
    match crate::server::global_ctx() {
        Some(ctx) => render_message(Some(&ctx.i18n), lang, key, args),
        None => key.to_string(),
    }
}

/// 渲染一条多语言日志并输出到 tracing（程序化 API，供非宏调用）。
///
/// - `svc`：i18n 服务（`None` 时直接输出 key，便于无服务环境调试）；
/// - `lang`：优先语言（`None` = 服务器默认语言，走回退链）；
/// - `args`：FTL 变量参数（`&[("name", "value")]`）。
pub fn log_event(
    level: Level,
    svc: Option<&I18nService>,
    lang: Option<&str>,
    key: &str,
    args: &[(&str, &str)],
) {
    let message = render_message(svc, lang, key, args);
    log_raw(level, &message);
}

/// 直接输出一条日志行（text 应为已渲染文本；命令等结构化多行输出用）。
pub fn log_raw(level: Level, text: &str) {
    match level {
        Level::ERROR => tracing::error!(target: "log", "{}", text),
        Level::WARN => tracing::warn!(target: "log", "{}", text),
        Level::INFO => tracing::info!(target: "log", "{}", text),
        Level::DEBUG => tracing::debug!(target: "log", "{}", text),
        Level::TRACE => tracing::trace!(target: "log", "{}", text),
    }
}

/// 内部宏：展开为「渲染 → `tracing::event!` 输出」，level 为字面量（`tracing::event!` 要求常量）。
/// `$lang:expr` 接受 `&str` / `&String` / `String`；`None` 表示走服务器默认语言。
macro_rules! log_impl {
    // 无显式语言（`None` 字面量）→ 服务器默认语言。需排在 `$lang:expr` 规则前，
    // 否则 `None` 会被当成 `$lang:expr` 匹配（`Option<&str>` 无法转 `&str`）。
    ($lvl:expr, $svc:expr, None, $key:literal $(, ($name:literal, $val:expr))* $(,)?) => {{
        let message = $crate::log::render_message(
            ::core::option::Option::Some($svc),
            ::core::option::Option::None,
            $key,
            &[ $( ($name, $val.to_string().as_str()) ),* ],
        );
        $crate::log::log_raw($lvl, &message);
    }};
    // 显式指定语言
    ($lvl:expr, $svc:expr, $lang:expr, $key:literal $(, ($name:literal, $val:expr))* $(,)?) => {{
        let message = $crate::log::render_message(
            ::core::option::Option::Some($svc),
            ::core::option::Option::Some(::core::convert::AsRef::<str>::as_ref(&$lang)),
            $key,
            &[ $( ($name, $val.to_string().as_str()) ),* ],
        );
        $crate::log::log_raw($lvl, &message);
    }};
    // 未指定语言（无第三参数）→ 服务器默认语言
    ($lvl:expr, $svc:expr, $key:literal $(, ($name:literal, $val:expr))* $(,)?) => {{
        let message = $crate::log::render_message(
            ::core::option::Option::Some($svc),
            ::core::option::Option::None,
            $key,
            &[ $( ($name, $val.to_string().as_str()) ),* ],
        );
        $crate::log::log_raw($lvl, &message);
    }};
}

/// 内部宏（全局 ctx 版）：同 [`log_impl`]，但 i18n 服务经全局 `ServerContext` 获取。
macro_rules! log_impl_global {
    // 显式指定语言
    ($lvl:expr, $lang:expr, $key:literal $(, ($name:literal, $val:expr))* $(,)?) => {{
        let message = $crate::log::render_global_message(
            ::core::option::Option::Some(::core::convert::AsRef::<str>::as_ref(&$lang)),
            $key,
            &[ $( ($name, $val.to_string().as_str()) ),* ],
        );
        $crate::log::log_raw($lvl, &message);
    }};
    // 服务器默认语言
    ($lvl:expr, $key:literal $(, ($name:literal, $val:expr))* $(,)?) => {{
        let message = $crate::log::render_global_message(
            ::core::option::Option::None,
            $key,
            &[ $( ($name, $val.to_string().as_str()) ),* ],
        );
        $crate::log::log_raw($lvl, &message);
    }};
}

macro_rules! log_error {
    ($($t:tt)*) => { $crate::log::log_impl!(::tracing::Level::ERROR, $($t)*) };
}
macro_rules! log_warn {
    ($($t:tt)*) => { $crate::log::log_impl!(::tracing::Level::WARN, $($t)*) };
}
macro_rules! log_info {
    ($($t:tt)*) => { $crate::log::log_impl!(::tracing::Level::INFO, $($t)*) };
}
macro_rules! log_debug {
    ($($t:tt)*) => { $crate::log::log_impl!(::tracing::Level::DEBUG, $($t)*) };
}
macro_rules! log_trace {
    ($($t:tt)*) => { $crate::log::log_impl!(::tracing::Level::TRACE, $($t)*) };
}
macro_rules! log_error_global {
    ($($t:tt)*) => { $crate::log::log_impl_global!(::tracing::Level::ERROR, $($t)*) };
}
macro_rules! log_warn_global {
    ($($t:tt)*) => { $crate::log::log_impl_global!(::tracing::Level::WARN, $($t)*) };
}
macro_rules! log_info_global {
    ($($t:tt)*) => { $crate::log::log_impl_global!(::tracing::Level::INFO, $($t)*) };
}
macro_rules! log_debug_global {
    ($($t:tt)*) => { $crate::log::log_impl_global!(::tracing::Level::DEBUG, $($t)*) };
}
macro_rules! log_trace_global {
    ($($t:tt)*) => { $crate::log::log_impl_global!(::tracing::Level::TRACE, $($t)*) };
}

// 导出宏供 crate 内使用（其余宏为完整日志 API，随调用点逐步启用）。
#[allow(unused_imports)]
pub(crate) use {
    log_debug, log_debug_global, log_error, log_error_global, log_impl, log_impl_global,
    log_info, log_info_global, log_trace, log_trace_global, log_warn, log_warn_global,
};

#[cfg(test)]
mod tests {
    use super::render_message;
    use crate::i18n::I18nService;

    fn svc() -> I18nService {
        I18nService::new("zh-CN")
    }

    /// 多语言日志按指定语言渲染（zh-CN）。
    #[test]
    fn log_info_renders_in_specified_language() {
        let s = svc();
        // 服务器语言键：验证参数插值 + 指定语言渲染
        let msg = render_message(
            Some(&s),
            Some("zh-CN"),
            "LOG_LANGUAGE",
            &[("lang", "zh-CN")],
        );
        assert!(msg.contains("服务器语言"));
        assert!(msg.contains("zh-CN"));
    }

    /// 默认语言（未指定 lang）渲染。
    #[test]
    fn log_info_renders_with_server_default_language() {
        let s = svc();
        let msg = render_message(Some(&s), None, "LOG_BOOTING", &[]);
        assert_eq!(msg, "正在启动 Phira 服务器...");
    }

    /// 指定语言未注册 → 回退服务器默认语言（zh-CN）。
    #[test]
    fn log_info_falls_back_on_missing_language() {
        let s = svc();
        let msg = render_message(Some(&s), Some("fr-FR"), "LOG_BOOTING", &[]);
        assert_eq!(msg, "正在启动 Phira 服务器...");
    }

    /// 参数插值：占位符被替换。
    #[test]
    fn log_info_substitutes_placeholders() {
        let s = svc();
        let msg = render_message(
            Some(&s),
            Some("zh-CN"),
            "LOG_LISTENING",
            &[("host", "0.0.0.0"), ("port", "12346")],
        );
        assert_eq!(msg, "正在偷听 0.0.0.0:12346");
    }

    /// 无 i18n 服务时输出 key 本身（便于无服务环境调试）。
    #[test]
    fn render_without_service_returns_key() {
        let msg = render_message(None, None, "LOG_BOOTING", &[]);
        assert_eq!(msg, "LOG_BOOTING");
    }
}
