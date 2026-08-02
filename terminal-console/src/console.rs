//! 控制台核心：终端能力检测、输入行状态、Builder、运行入口。

use crate::input::{self, ConsoleHandler};
use std::io::{self, IsTerminal};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Mutex, OnceLock};

/// 终端能力。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TermCaps {
    /// 完整交互终端（raw mode + 光标控制 + 颜色）。
    Interactive,
    /// 退化终端（纯文本行读写，无光标控制）。
    Dumb,
}

/// 全局共享的「当前控制台」（对应 TCA 的静态 terminal/reader）。
static ACTIVE: OnceLock<Console> = OnceLock::new();

/// 是否有交互式 readline 正在运行（render 据此决定是否重绘提示符）。
pub(crate) static INTERACTIVE: AtomicBool = AtomicBool::new(false);

/// 控制台：持终端能力 + 输入行快照 + 写锁。
///
/// 通常经 [`Console::builder`] 构造一个实例 `run`；render/tracing 桥接则操作
/// 进程级「活动控制台」（[`Console::global`]）。
pub struct Console {
    pub(crate) caps: TermCaps,
    pub(crate) prompt: String,
    /// 命令历史（向上/向下键翻阅）。
    pub(crate) history: Mutex<Vec<String>>,
    /// 当前输入行快照（提示符 + buffer），render 重绘用。
    pub(crate) input_line: Mutex<String>,
    /// 写锁：保证「清行→打印→重绘」原子完成，不与 readline 回显交错。
    pub(crate) write_lock: Mutex<()>,
}

impl Console {
    /// 构造器。
    pub fn builder() -> ConsoleBuilder {
        ConsoleBuilder::default()
    }

    fn detect_caps() -> TermCaps {
        if io::stdin().is_terminal() && io::stdout().is_terminal() {
            TermCaps::Interactive
        } else {
            TermCaps::Dumb
        }
    }

    /// 进程级活动控制台（render / tracing 桥接使用）。
    ///
    /// 若尚未 `run` 任何实例，返回一个按 TTY 自动检测的默认控制台。
    pub fn global() -> &'static Console {
        ACTIVE.get_or_init(|| Console::builder().build())
    }

    /// 把本实例设为进程级活动控制台（`run` 内部调用）。
    fn activate(self) -> &'static Console {
        // 已有活动控制台则复用（多实例场景取第一个），否则设为活动。
        ACTIVE.get_or_init(|| self)
    }

    pub fn caps(&self) -> TermCaps {
        self.caps
    }

    pub fn prompt(&self) -> &str {
        &self.prompt
    }

    /// 更新输入行快照（readline 线程调用）。
    pub(crate) fn set_input_line(&self, line: String) {
        *self.input_line.lock().unwrap() = line;
    }

    /// 输入行快照（render 重绘读取）。
    pub(crate) fn input_line_snapshot(&self) -> String {
        self.input_line.lock().unwrap().clone()
    }

    /// 追加历史。
    pub(crate) fn push_history(&self, cmd: String) {
        let mut h = self.history.lock().unwrap();
        if h.last().map(|l| l != &cmd).unwrap_or(true) {
            h.push(cmd);
        }
    }

    /// 取历史（index 从最新往回数：0 = 最新）。
    pub(crate) fn history_at(&self, back: usize) -> Option<String> {
        let h = self.history.lock().unwrap();
        let len = h.len();
        if back < len {
            Some(h[len - 1 - back].clone())
        } else {
            None
        }
    }

    /// 启动交互式控制台（阻塞当前线程）。
    ///
    /// - 交互模式：raw mode readline，Ctrl+C → `handler.on_shutdown()`。
    /// - 退化模式：纯行读，EOF 结束。
    pub fn run(self, handler: impl ConsoleHandler + 'static) {
        let console = self.activate();
        match console.caps {
            TermCaps::Interactive => input::run_interactive(console, handler),
            TermCaps::Dumb => input::run_dumb(console, handler),
        }
        INTERACTIVE.store(false, Ordering::SeqCst);
    }
}

/// Console 构造器（Rust 风格 Builder）。
pub struct ConsoleBuilder {
    prompt: String,
    caps: Option<TermCaps>,
}

impl Default for ConsoleBuilder {
    fn default() -> Self {
        Self {
            prompt: "> ".to_string(),
            caps: None,
        }
    }
}

impl ConsoleBuilder {
    /// 自定义提示符（默认 `"> "`）。
    pub fn prompt(mut self, prompt: impl Into<String>) -> Self {
        self.prompt = prompt.into();
        self
    }

    /// 强制终端能力（默认按 TTY 自动检测；测试可强制 `Dumb`）。
    pub fn caps(mut self, caps: TermCaps) -> Self {
        self.caps = Some(caps);
        self
    }

    pub fn build(self) -> Console {
        Console {
            caps: self.caps.unwrap_or_else(Console::detect_caps),
            prompt: self.prompt,
            history: Mutex::new(Vec::new()),
            input_line: Mutex::new(String::new()),
            write_lock: Mutex::new(()),
        }
    }
}
