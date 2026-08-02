//! phira-mp 服务端入口。
//!
//! 日志走 terminal-console（对标 Java TCA）：
//! - 自定义 fmt 格式 `LEVEL hh:mm:ss | target: message`（Minecraft 风格简化版，
//!   不重复 level、时间只到秒）；
//! - `.with_ansi(false)`：不让 tracing 输出 ANSI 转义符（老 Windows 会乱码）；
//! - `ConsoleMakeWriter`：日志经 crossterm 重上色（无 VT 时自动回退 Win32 API），
//!   且在输入行上方打印、自动重绘提示符——日志不打断输入。

use clap::Parser;
use phira_mp::server::{run, ServerArgs};
use std::fmt;
use terminal_console::ConsoleMakeWriter;
use tracing::Event;
use tracing_core::Subscriber;
use tracing_subscriber::fmt::format::{FormatEvent, FormatFields, Writer};
use tracing_subscriber::fmt::FmtContext;
use tracing_subscriber::registry::LookupSpan;
use tracing_subscriber::EnvFilter;

/// 自定义日志格式：`LEVEL hh:mm:ss | target: message`。
///
/// 不输出颜色——颜色由 terminal-console 的 `print_log` 按 level 上色
/// （无 VT 的 Windows 自动回退 Win32 API）。
struct ConsoleFormat;

impl<S, N> FormatEvent<S, N> for ConsoleFormat
where
    S: Subscriber + for<'a> LookupSpan<'a>,
    N: for<'a> FormatFields<'a> + 'static,
{
    fn format_event(
        &self,
        ctx: &FmtContext<'_, S, N>,
        mut writer: Writer<'_>,
        event: &Event<'_>,
    ) -> fmt::Result {
        let meta = event.metadata();
        let t = time::OffsetDateTime::now_local()
            .unwrap_or_else(|_| time::OffsetDateTime::now_utc());
        write!(
            writer,
            "{} {:02}:{:02}:{:02} | {}: ",
            meta.level().as_str(),
            t.hour(),
            t.minute(),
            t.second(),
            short_target(meta.target()),
        )?;
        ctx.field_format().format_fields(writer.by_ref(), event)?;
        writeln!(writer)
    }
}

/// 截取模块路径最后一段（`phira_mp::network::authenticate_handler` → `authenticate_handler`）。
fn short_target(target: &str) -> &str {
    target.rsplit_once("::").map(|(_, rest)| rest).unwrap_or(target)
}

#[tokio::main]
async fn main() -> std::io::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")))
        .with_ansi(false) // 颜色交给 terminal-console/crossterm（自动 Windows Win32 回退）
        .event_format(ConsoleFormat)
        .with_writer(ConsoleMakeWriter)
        .init();

    let args = ServerArgs::parse();
    run(args).await
}
