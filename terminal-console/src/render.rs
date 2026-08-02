//! 渲染：日志/文本在输入行上方打印并重绘提示符，及 ANSI 剥除。

use crate::console::{Console, TermCaps};
use crossterm::style::{Attribute, Color, Print, PrintStyledContent, Stylize};
use crossterm::{cursor, execute, queue, terminal};
use std::io::{self, Write};

/// 日志级别（供 render 用 crossterm 上色；crossterm 在无 VT 的 Windows 自动回退
/// `SetConsoleTextAttribute`，故颜色不依赖 ANSI 转义符本身）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Level {
    Trace,
    Debug,
    Info,
    Warn,
    Error,
}

impl Level {
    fn color(self) -> Color {
        match self {
            Level::Trace => Color::DarkGrey,
            Level::Debug => Color::Cyan,
            Level::Info => Color::Green,
            Level::Warn => Color::Yellow,
            Level::Error => Color::Red,
        }
    }
    pub fn as_str(self) -> &'static str {
        match self {
            Level::Trace => "TRACE",
            Level::Debug => "DEBUG",
            Level::Info => "INFO",
            Level::Warn => "WARN",
            Level::Error => "ERROR",
        }
    }
}

/// 剥掉 ANSI 转义序列（对应 `%stripAnsi`；写文件/纯文本用）。
pub fn strip_ansi(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let bytes = text.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == 0x1B && i + 1 < bytes.len() && bytes[i + 1] == b'[' {
            i += 2; // 跳过 ESC [
            while i < bytes.len() {
                let b = bytes[i];
                i += 1;
                if (0x40..=0x7E).contains(&b) {
                    break; // CSI 终结符
                }
            }
        } else {
            out.push(bytes[i] as char);
            i += 1;
        }
    }
    out
}

/// 是否强制禁用颜色（标准 `NO_COLOR` 环境变量）。
fn color_disabled() -> bool {
    std::env::var_os("NO_COLOR").is_some()
}

/// 在输入行上方打印一行文本（核心 API，对应 JLine `LineReader.printAbove`）。
///
/// 交互模式：清当前行 → 打印 → 重绘提示符+输入快照。
/// 退化模式：直接换行打印。
pub fn print_above(text: &str) {
    print_above_on(Console::global(), text);
}

fn print_above_on(console: &Console, text: &str) {
    let _guard = console.write_lock.lock().unwrap();
    let mut out = io::stdout();
    match console.caps {
        TermCaps::Interactive => {
            let snapshot = console.input_line_snapshot();
            let _ = execute!(
                out,
                cursor::MoveToColumn(0),
                terminal::Clear(terminal::ClearType::CurrentLine),
                Print(text),
                Print(if text.ends_with('\n') { "" } else { "\n" }),
                Print(&snapshot),
            );
            let _ = out.flush();
        }
        TermCaps::Dumb => {
            let _ = out.write_all(text.as_bytes());
            if !text.ends_with('\n') {
                let _ = out.write_all(b"\n");
            }
            let _ = out.flush();
        }
    }
}

/// 日志行分段（文本 + 可选颜色 + 加粗），供 `print_log` 逐段上色。
struct Segment {
    text: String,
    color: Option<Color>,
    bold: bool,
}

/// 解析 fmt 行 `LEVEL HH:MM:SS | target: message`：
/// level 用 level 色（加粗），时间、`|` 分隔符与 target 灰色，其余保持默认。
///
/// 非标准行（不以 LEVEL 开头等）退化为仅 level 上色，其余原样。
fn parse_log_segments(text: &str, level: Level) -> Vec<Segment> {
    let lvl = level.as_str();
    if let Some(rest) = text.strip_prefix(lvl) {
        if let Some(rest) = rest.strip_prefix(' ') {
            if let Some((time, tail)) = rest.split_once(' ') {
                if let Some(tail) = tail.strip_prefix("| ") {
                    if let Some((target, msg)) = tail.split_once(": ") {
                        return vec![
                            Segment { text: lvl.to_string(), color: Some(level.color()), bold: true },
                            Segment { text: " ".to_string(), color: None, bold: false },
                            Segment { text: time.to_string(), color: Some(Color::DarkGrey), bold: false },
                            Segment { text: " | ".to_string(), color: Some(Color::DarkGrey), bold: false },
                            Segment { text: target.to_string(), color: Some(Color::DarkGrey), bold: false },
                            Segment { text: format!(": {msg}"), color: None, bold: false },
                        ];
                    }
                }
            }
        }
        // 非标准行：仅 level 上色
        vec![
            Segment { text: lvl.to_string(), color: Some(level.color()), bold: true },
            Segment { text: rest.to_string(), color: None, bold: false },
        ]
    } else {
        vec![Segment { text: text.to_string(), color: None, bold: false }]
    }
}

fn write_segments(out: &mut impl Write, segments: &[Segment]) {
    for seg in segments {
        if let Some(c) = seg.color {
            let mut styled = seg.text.as_str().with(c);
            if seg.bold {
                styled = styled.attribute(Attribute::Bold);
            }
            let _ = queue!(out, PrintStyledContent(styled));
        } else {
            let _ = queue!(out, Print(&seg.text));
        }
    }
}

/// 结构化日志输出：fmt 行 `LEVEL HH:MM:SS | target: message` 逐段上色
/// （level 用级别色加粗、时间与 target 灰色、其余默认）。
/// 颜色由 crossterm 渲染，无 VT 的 Windows 自动回退 Win32 API。
pub fn print_log(level: Level, text: &str) {
    let console = Console::global();
    if color_disabled() {
        print_above_on(console, text);
        return;
    }
    let segments = parse_log_segments(text, level);
    let _guard = console.write_lock.lock().unwrap();
    let mut out = io::stdout();
    match console.caps {
        TermCaps::Interactive => {
            let snapshot = console.input_line_snapshot();
            let _ = queue!(
                out,
                cursor::MoveToColumn(0),
                terminal::Clear(terminal::ClearType::CurrentLine),
            );
            write_segments(&mut out, &segments);
            let _ = queue!(out, Print("\n"), Print(&snapshot));
            let _ = out.flush();
        }
        TermCaps::Dumb => {
            write_segments(&mut out, &segments);
            let _ = queue!(out, Print("\n"));
            let _ = out.flush();
        }
    }
}

/// 重绘当前输入行（readline 每键后调用）。
pub(crate) fn redraw_input() {
    let console = Console::global();
    if console.caps != TermCaps::Interactive {
        return;
    }
    let _guard = console.write_lock.lock().unwrap();
    let snapshot = console.input_line_snapshot();
    let mut out = io::stdout();
    let _ = execute!(
        out,
        cursor::MoveToColumn(0),
        terminal::Clear(terminal::ClearType::CurrentLine),
        Print(&snapshot),
    );
    let _ = out.flush();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strip_ansi_removes_color_codes() {
        assert_eq!(strip_ansi("\u{1b}[31mred\u{1b}[0m"), "red");
        assert_eq!(strip_ansi("\u{1b}[1;32mgreen bold\u{1b}[m"), "green bold");
        assert_eq!(strip_ansi("plain"), "plain");
        assert_eq!(strip_ansi(""), "");
    }

    #[test]
    fn strip_ansi_handles_cursor_sequences() {
        assert_eq!(strip_ansi("\u{1b}[2Ktext"), "text");
        assert_eq!(strip_ansi("\u{1b}[1G> "), "> ");
    }

    #[test]
    fn strip_ansi_mixed_content() {
        assert_eq!(
            strip_ansi("[12:00:00 \u{1b}[32mINFO\u{1b}[0m] msg \u{1b}[33mwarn\u{1b}[0m"),
            "[12:00:00 INFO] msg warn"
        );
    }

    #[test]
    fn parse_log_segments_splits_fmt_line() {
        let segs = parse_log_segments("INFO 18:20:55 | server: server stopped", Level::Info);
        assert_eq!(segs.len(), 6);
        assert_eq!(segs[0].text, "INFO");
        assert!(segs[0].bold);
        assert_eq!(segs[1].text, " ");
        assert_eq!(segs[2].text, "18:20:55");
        assert_eq!(segs[2].color, Some(Color::DarkGrey));
        assert_eq!(segs[3].text, " | ");
        assert_eq!(segs[3].color, Some(Color::DarkGrey));
        assert_eq!(segs[4].text, "server");
        assert_eq!(segs[4].color, Some(Color::DarkGrey));
        assert_eq!(segs[5].text, ": server stopped");
        assert_eq!(segs[5].color, None);
    }

    #[test]
    fn parse_log_segments_handles_nested_target() {
        let segs = parse_log_segments("WARN 18:20:55 | room::local: bad", Level::Warn);
        assert_eq!(segs[4].text, "room::local");
        assert_eq!(segs[5].text, ": bad");
    }

    #[test]
    fn parse_log_segments_fallback_for_non_fmt_line() {
        let segs = parse_log_segments("plain text", Level::Info);
        assert_eq!(segs.len(), 1);
        assert_eq!(segs[0].text, "plain text");
        assert_eq!(segs[0].color, None);
    }
}
