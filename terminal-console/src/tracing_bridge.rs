//! tracing 桥接：fmt 层输出 → console（日志不打断输入行 + 上色 + Windows 回退）。

use crate::render::{print_log, Level};
use std::io;

/// `MakeWriter`：交给 tracing_subscriber fmt 层。
///
/// fmt 层需配置 `.with_ansi(false)`——颜色由本 crate 用 crossterm 重上色
/// （自动 Windows Win32 回退），避免 tracing 输出的 ANSI 转义符在老 Windows 乱码。
#[derive(Debug, Clone, Copy, Default)]
pub struct ConsoleMakeWriter;

impl<'a> tracing_subscriber::fmt::MakeWriter<'a> for ConsoleMakeWriter {
    type Writer = ConsoleLineWriter;
    fn make_writer(&'a self) -> Self::Writer {
        ConsoleLineWriter::default()
    }
}

/// 行缓冲 writer：tracing fmt 分段写，攒到换行后整体交给 console 上色输出。
#[derive(Default)]
pub struct ConsoleLineWriter {
    buf: Vec<u8>,
}

impl io::Write for ConsoleLineWriter {
    fn write(&mut self, data: &[u8]) -> io::Result<usize> {
        self.buf.extend_from_slice(data);
        while let Some(pos) = self.buf.iter().position(|&b| b == b'\n') {
            let line: Vec<u8> = self.buf.drain(..=pos).collect();
            let text = String::from_utf8_lossy(&line);
            emit_log_line(&text);
        }
        Ok(data.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        if !self.buf.is_empty() {
            let text = String::from_utf8_lossy(&self.buf);
            emit_log_line(&text);
            self.buf.clear();
        }
        io::stdout().flush()
    }
}

impl Drop for ConsoleLineWriter {
    fn drop(&mut self) {
        if !self.buf.is_empty() {
            let text = String::from_utf8_lossy(&self.buf);
            emit_log_line(&text);
        }
    }
}

fn emit_log_line(line: &str) {
    let line = line.strip_suffix('\n').unwrap_or(line);
    if line.is_empty() {
        return;
    }
    print_log(detect_level(line), line);
}

/// 从 fmt 行中识别 level（自定义格式 `LEVEL hh:mm:ss | target: msg`，level 在行首）。
fn detect_level(line: &str) -> Level {
    let trimmed = line.trim_start();
    for (name, lvl) in [
        ("ERROR", Level::Error),
        ("WARN", Level::Warn),
        ("INFO", Level::Info),
        ("DEBUG", Level::Debug),
        ("TRACE", Level::Trace),
    ] {
        if trimmed.starts_with(name) {
            return lvl;
        }
    }
    Level::Info
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detect_level_from_fmt_line() {
        assert_eq!(detect_level("ERROR 10:03:53 | phira: boom"), Level::Error);
        assert_eq!(detect_level("WARN 10:03:53 | phira: hmm"), Level::Warn);
        assert_eq!(detect_level("INFO 10:03:53 | phira: ok"), Level::Info);
        assert_eq!(detect_level("DEBUG 10:03:53 | phira: dbg"), Level::Debug);
        assert_eq!(detect_level("TRACE 10:03:53 | phira: tr"), Level::Trace);
        assert_eq!(detect_level("no level here"), Level::Info);
    }
}
