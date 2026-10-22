//! 命令名和参数解析，保留原始文本供消息与扩展命令使用。

pub(super) struct ParsedCommand<'a> {
    pub raw: &'a str,
    pub name: String,
    pub args: Vec<&'a str>,
}

impl<'a> ParsedCommand<'a> {
    pub fn parse(input: &'a str) -> Option<Self> {
        let raw = input.trim();
        let mut parts = raw.split_whitespace();
        let name = parts.next()?.to_ascii_lowercase();
        Some(Self {
            raw,
            name,
            args: parts.collect(),
        })
    }
}

/// 取命令中跳过前 `skip` 个空白分隔 token 后的剩余文本（保留空格）。
/// `skip = 1` 跳过命令名；`skip = 2` 再跳过一个参数（如 roomsay 的 roomId）。
pub(super) fn tail_after(cmd: &str, skip: usize) -> Option<String> {
    let mut rest = cmd.trim();
    for _ in 0..skip {
        let i = rest.find(|c: char| c.is_whitespace())?;
        rest = rest[i..].trim_start();
    }
    (!rest.is_empty()).then(|| rest.to_string())
}

/// 解析布尔参数（true/false/1/0/yes/no/on/off，大小写不敏感）。
pub(super) fn parse_bool(s: &str) -> Option<bool> {
    match s.to_ascii_lowercase().as_str() {
        "true" | "1" | "yes" | "on" => Some(true),
        "false" | "0" | "no" | "off" => Some(false),
        _ => None,
    }
}
