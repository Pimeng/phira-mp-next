//! terminal-console —— Rust 版交互式终端控制台。
//!
//! 灵感来自 Java 的 TerminalConsoleAppender（TCA），但按 Rust 习惯重新设计，
//! 不是逐行翻译。核心能力：
//!
//! - **日志不打断输入行**：日志在输入行「上方」打印，提示符与已输入内容自动重绘
//!   （等价 JLine `LineReader.printAbove`）。
//! - **交互式 readline**：raw mode 逐键读取（可打印字符 / 退格 / 回车提交 / 历史）。
//! - **Ctrl+C → 优雅关闭回调**。
//! - **无 TTY 回退**：非终端环境退化为纯文本行读写。
//! - **Windows 自适应**：crossterm 在无 VT processing 的老 Windows 自动回退 Win32 API
//!   （颜色 `SetConsoleTextAttribute`、光标 `SetConsoleCursorPosition`）。
//!
//! # 快速开始
//!
//! ```no_run
//! use terminal_console::{Console, ConsoleHandler};
//!
//! struct App;
//! impl ConsoleHandler for App {
//!     fn is_running(&self) -> bool { true }
//!     fn on_command(&mut self, cmd: &str) { terminal_console::print_above(&format!("echo: {cmd}")); }
//!     fn on_shutdown(&mut self) {}
//! }
//!
//! Console::builder().prompt("app> ").build().run(App);
//! ```
//!
//! # 日志桥接（tracing）
//!
//! ```no_run
//! tracing_subscriber::fmt()
//!     .with_ansi(false) // 颜色交给本 crate（自动 Windows 回退）
//!     .with_writer(terminal_console::ConsoleMakeWriter)
//!     .init();
//! ```

mod console;
mod input;
mod render;
mod tracing_bridge;

pub use console::{Console, ConsoleBuilder, TermCaps};
pub use input::{ConsoleHandler, KeyAction};
pub use render::{print_above, print_log, strip_ansi, Level};
pub use tracing_bridge::{ConsoleLineWriter, ConsoleMakeWriter};

/// 控制台是否处于交互模式（有 TTY 且 raw mode 可用）。
pub fn is_interactive() -> bool {
    Console::global().caps() == TermCaps::Interactive
}
