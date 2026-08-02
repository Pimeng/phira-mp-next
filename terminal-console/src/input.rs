//! 输入处理：交互式 readline 与退化行读。

use crate::console::{Console, INTERACTIVE};
use crossterm::event::{Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use crossterm::{execute, style::Print, terminal};
use std::io::{self, Write};
use std::sync::atomic::Ordering;

/// 控制台命令处理器。
///
/// 对应 TCA `SimpleTerminalConsole` 的抽象方法，但用 Rust trait 表达。
pub trait ConsoleHandler: Send {
    /// 是否仍在运行（对应 `isRunning`）。
    fn is_running(&self) -> bool;
    /// 执行一条命令（对应 `runCommand`）。
    fn on_command(&mut self, command: &str);
    /// Ctrl+C 触发（对应 `shutdown`）。
    fn on_shutdown(&mut self);
}

/// 单键动作（供自定义按键映射；当前内建处理，预留扩展点）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum KeyAction {
    Insert(char),
    Backspace,
    Submit,
    HistoryPrev,
    HistoryNext,
    Shutdown,
    Eof,
    Ignore,
}

/// 把按键事件映射为动作（内建默认映射）。
fn map_key(code: KeyCode, modifiers: KeyModifiers) -> KeyAction {
    match (code, modifiers) {
        (KeyCode::Char('c'), m) if m.contains(KeyModifiers::CONTROL) => KeyAction::Shutdown,
        (KeyCode::Char('d'), m) if m.contains(KeyModifiers::CONTROL) => KeyAction::Eof,
        (KeyCode::Enter, _) => KeyAction::Submit,
        (KeyCode::Backspace, _) => KeyAction::Backspace,
        (KeyCode::Up, _) => KeyAction::HistoryPrev,
        (KeyCode::Down, _) => KeyAction::HistoryNext,
        (KeyCode::Char(c), m) if !m.contains(KeyModifiers::CONTROL) => KeyAction::Insert(c),
        _ => KeyAction::Ignore,
    }
}

/// 交互式 readline（raw mode）。
pub(crate) fn run_interactive(console: &'static Console, mut handler: impl ConsoleHandler) {
    INTERACTIVE.store(true, Ordering::SeqCst);
    struct RawGuard;
    impl Drop for RawGuard {
        fn drop(&mut self) {
            let _ = terminal::disable_raw_mode();
        }
    }
    if terminal::enable_raw_mode().is_err() {
        INTERACTIVE.store(false, Ordering::SeqCst);
        return run_dumb(console, handler);
    }
    let _raw = RawGuard;

    let mut buffer = String::new();
    let mut history_idx: Option<usize> = None; // None = 不在翻阅历史
    console.set_input_line(console.prompt().to_string());
    crate::render::redraw_input();

    while handler.is_running() {
        let ev = match crossterm::event::read() {
            Ok(e) => e,
            Err(_) => break,
        };
        let Event::Key(key_event) = ev else {
            continue;
        };
        // Windows 上每次按键 crossterm 会同时报 Press 与 Release 两个事件；
        // 只处理 Press/Repeat，忽略 Release，否则同一字符会被插入两次
        // （输入 `help` 会变成 `hheellpp`）。
        if key_event.kind == KeyEventKind::Release {
            continue;
        }
        let KeyEvent { code, modifiers, .. } = key_event;
        match map_key(code, modifiers) {
            KeyAction::Shutdown => {
                console.set_input_line(String::new());
                handler.on_shutdown();
                break;
            }
            KeyAction::Eof => break,
            KeyAction::Submit => {
                let line = std::mem::take(&mut buffer);
                history_idx = None;
                newline();
                console.set_input_line(console.prompt().to_string());
                let cmd = line.trim().to_string();
                if !cmd.is_empty() {
                    console.push_history(cmd.clone());
                    handler.on_command(&cmd);
                }
                crate::render::redraw_input();
            }
            KeyAction::Backspace => {
                if buffer.pop().is_some() {
                    history_idx = None;
                    sync_line(console, &buffer);
                }
            }
            KeyAction::Insert(c) => {
                buffer.push(c);
                history_idx = None;
                sync_line(console, &buffer);
            }
            KeyAction::HistoryPrev => {
                let next = history_idx.map(|i| i + 1).unwrap_or(0);
                if let Some(entry) = console.history_at(next) {
                    history_idx = Some(next);
                    buffer = entry;
                    sync_line(console, &buffer);
                }
            }
            KeyAction::HistoryNext => {
                match history_idx {
                    Some(0) | None => {
                        history_idx = None;
                        buffer.clear();
                        sync_line(console, &buffer);
                    }
                    Some(i) => {
                        let prev = i - 1;
                        if let Some(entry) = console.history_at(prev) {
                            history_idx = Some(prev);
                            buffer = entry;
                            sync_line(console, &buffer);
                        }
                    }
                }
            }
            KeyAction::Ignore => {}
        }
    }
    console.set_input_line(String::new());
    INTERACTIVE.store(false, Ordering::SeqCst);
}

/// 退化行读（无 TTY / raw mode 不可用）。
pub(crate) fn run_dumb(_console: &Console, mut handler: impl ConsoleHandler) {
    use std::io::BufRead;
    let stdin = io::stdin();
    let mut lock = stdin.lock();
    loop {
        if !handler.is_running() {
            break;
        }
        let mut line = String::new();
        match lock.read_line(&mut line) {
            Ok(0) | Err(_) => break, // EOF
            Ok(_) => {
                let cmd = line.trim().to_string();
                if !cmd.is_empty() {
                    handler.on_command(&cmd);
                }
            }
        }
    }
}

fn sync_line(console: &Console, buffer: &str) {
    console.set_input_line(format!("{}{}", console.prompt(), buffer));
    crate::render::redraw_input();
}

fn newline() {
    let _guard = Console::global().write_lock.lock().unwrap();
    let mut out = io::stdout();
    let _ = execute!(out, Print("\r\n"));
    let _ = out.flush();
}
