//! YAML 配置文件（默认 `config.yml`）。
//!
//! 加载优先级：**CLI 显式参数 > config.yml > 内置默认值**。
//! - 配置文件不存在时静默使用全部内置默认值；
//! - 存在但解析失败（语法错误 / 未知键）则报错退出；
//! - 配置文件中的值只在对应 CLI 参数**未被显式指定**时生效。
//!
//! 参考示例见仓库根目录 `config.example.yml`。

use clap::parser::ValueSource;
use clap::ArgMatches;
use serde::Deserialize;

use crate::server::ServerArgs;

/// 顶层配置。所有字段均可选，缺省回退到内置默认值。
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct ServerConfig {
    /// 服务器基础设置。
    pub server: Option<ServerSection>,
    /// Phira API 客户端调优。
    pub phira: Option<PhiraSection>,
    /// 网络超时。
    pub network: Option<NetworkSection>,
    /// 房间设置。
    pub room: Option<RoomSection>,
}

/// 服务器基础设置（对应原有 CLI 参数）。
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct ServerSection {
    /// 监听地址。
    pub host: Option<String>,
    /// 游戏端口。
    pub port: Option<u16>,
    /// HTTP 查询 API 端口（0 = 禁用）。
    pub http_port: Option<u16>,
    /// 启用 HAProxy PROXY 协议。
    pub proxy_protocol: Option<bool>,
    /// 服务器默认语言。
    pub language: Option<String>,
    /// 会话挂起超时（秒）。
    pub session_timeout: Option<u64>,
    /// 对局录制输出目录（null = 禁用录制）。
    pub record_dir: Option<String>,
}

/// Phira API 客户端调优。
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct PhiraSection {
    /// Phira API Base URL。
    pub api: Option<String>,
    /// GET 请求最大尝试次数。
    pub max_attempts: Option<u32>,
    /// 重试退避基数（毫秒，退避 = base × attempt）。
    pub retry_base_ms: Option<u64>,
    /// 各缓存 TTL / 容量。
    pub cache: Option<PhiraCacheSection>,
}

/// Phira API 缓存调优。
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct PhiraCacheSection {
    /// token → 用户缓存 TTL（秒）。
    pub token_ttl: Option<u64>,
    /// token → 用户缓存容量。
    pub token_cap: Option<usize>,
    /// 用户信息缓存 TTL（秒）。
    pub user_ttl: Option<u64>,
    /// 用户信息缓存容量。
    pub user_cap: Option<usize>,
    /// 谱面缓存 TTL（秒）。
    pub chart_ttl: Option<u64>,
    /// 谱面缓存容量。
    pub chart_cap: Option<usize>,
    /// 成绩缓存 TTL（秒）。
    pub record_ttl: Option<u64>,
    /// 成绩缓存容量。
    pub record_cap: Option<usize>,
}

/// 网络超时（秒）。
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct NetworkSection {
    /// 握手超时（秒）。
    pub handshake_timeout: Option<u64>,
    /// 读帧超时（秒）。
    pub read_timeout: Option<u64>,
    /// PROXY 协议解析超时（秒）。
    pub proxy_timeout: Option<u64>,
}

/// 房间设置。
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct RoomSection {
    /// 新建房间默认最大人数。
    pub default_max_player: Option<usize>,
}

impl ServerConfig {
    /// 从文件加载配置；文件不存在时返回全默认配置。
    ///
    /// 错误仅来自：文件读取失败（非 NotFound）或 YAML 解析失败。
    pub fn load(path: &str) -> std::io::Result<ServerConfig> {
        let text = match std::fs::read_to_string(path) {
            Ok(t) => t,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                return Ok(ServerConfig::default());
            }
            Err(e) => return Err(e),
        };
        serde_yml::from_str(&text)
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, format!("{path}: {e}")))
    }

    /// 将配置文件值合并进 CLI 参数：
    /// 仅当 CLI 参数**未显式指定**时，才用配置文件的值（否则保持 CLI 值/内置默认）。
    pub fn merge_into(&self, cli: ServerArgs, matches: &ArgMatches) -> ServerArgs {
        macro_rules! pick {
            ($name:literal, $cli:expr, $cfg:expr) => {
                if matches.value_source($name) == Some(ValueSource::CommandLine) {
                    $cli
                } else {
                    $cfg.unwrap_or($cli)
                }
            };
        }
        // Option 字段：配置值优先，其次 CLI 值（unwrap_or 对 Option<T> 不适用）。
        macro_rules! pick_opt {
            ($name:literal, $cli:expr, $cfg:expr) => {
                if matches.value_source($name) == Some(ValueSource::CommandLine) {
                    $cli
                } else {
                    $cfg.or($cli)
                }
            };
        }
        let server = &self.server;
        let phira = &self.phira;
        let cache = phira.as_ref().and_then(|p| p.cache.as_ref());
        let network = &self.network;
        let room = &self.room;

        ServerArgs {
            host: pick!("host", cli.host, server.as_ref().and_then(|s| s.host.clone())),
            port: pick!("port", cli.port, server.as_ref().and_then(|s| s.port)),
            http_port: pick!(
                "http_port",
                cli.http_port,
                server.as_ref().and_then(|s| s.http_port)
            ),
            proxy_protocol: pick!(
                "proxy_protocol",
                cli.proxy_protocol,
                server.as_ref().and_then(|s| s.proxy_protocol)
            ),
            language: pick!(
                "language",
                cli.language,
                server.as_ref().and_then(|s| s.language.clone())
            ),
            session_timeout: pick!(
                "session_timeout",
                cli.session_timeout,
                server.as_ref().and_then(|s| s.session_timeout)
            ),
            record_dir: pick_opt!(
                "record_dir",
                cli.record_dir,
                server.as_ref().and_then(|s| s.record_dir.clone())
            ),
            phira_api: pick!(
                "phira_api",
                cli.phira_api,
                phira.as_ref().and_then(|p| p.api.clone())
            ),
            phira_max_attempts: pick!(
                "phira_max_attempts",
                cli.phira_max_attempts,
                phira.as_ref().and_then(|p| p.max_attempts)
            ),
            phira_retry_base_ms: pick!(
                "phira_retry_base_ms",
                cli.phira_retry_base_ms,
                phira.as_ref().and_then(|p| p.retry_base_ms)
            ),
            phira_token_cache_ttl: pick!(
                "phira_token_cache_ttl",
                cli.phira_token_cache_ttl,
                cache.and_then(|c| c.token_ttl)
            ),
            phira_token_cache_cap: pick!(
                "phira_token_cache_cap",
                cli.phira_token_cache_cap,
                cache.and_then(|c| c.token_cap)
            ),
            phira_user_cache_ttl: pick!(
                "phira_user_cache_ttl",
                cli.phira_user_cache_ttl,
                cache.and_then(|c| c.user_ttl)
            ),
            phira_user_cache_cap: pick!(
                "phira_user_cache_cap",
                cli.phira_user_cache_cap,
                cache.and_then(|c| c.user_cap)
            ),
            phira_chart_cache_ttl: pick!(
                "phira_chart_cache_ttl",
                cli.phira_chart_cache_ttl,
                cache.and_then(|c| c.chart_ttl)
            ),
            phira_chart_cache_cap: pick!(
                "phira_chart_cache_cap",
                cli.phira_chart_cache_cap,
                cache.and_then(|c| c.chart_cap)
            ),
            phira_record_cache_ttl: pick!(
                "phira_record_cache_ttl",
                cli.phira_record_cache_ttl,
                cache.and_then(|c| c.record_ttl)
            ),
            phira_record_cache_cap: pick!(
                "phira_record_cache_cap",
                cli.phira_record_cache_cap,
                cache.and_then(|c| c.record_cap)
            ),
            handshake_timeout: pick!(
                "handshake_timeout",
                cli.handshake_timeout,
                network.as_ref().and_then(|n| n.handshake_timeout)
            ),
            read_timeout: pick!(
                "read_timeout",
                cli.read_timeout,
                network.as_ref().and_then(|n| n.read_timeout)
            ),
            proxy_timeout: pick!(
                "proxy_timeout",
                cli.proxy_timeout,
                network.as_ref().and_then(|n| n.proxy_timeout)
            ),
            default_max_player: pick!(
                "default_max_player",
                cli.default_max_player,
                room.as_ref().and_then(|r| r.default_max_player)
            ),
            // config 路径只来自 CLI（默认 config.yml），不进配置文件。
            config: cli.config,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::{CommandFactory, FromArgMatches};

    fn parse_cli(args: &[&str]) -> (ServerArgs, ArgMatches) {
        let matches = ServerArgs::command()
            .try_get_matches_from(args)
            .expect("cli parse");
        let cli = ServerArgs::from_arg_matches(&matches).expect("from arg matches");
        (cli, matches)
    }

    #[test]
    fn load_missing_file_returns_default() {
        let cfg = ServerConfig::load("definitely-not-exist.yml").unwrap();
        assert!(cfg.server.is_none());
        assert!(cfg.phira.is_none());
    }

    #[test]
    fn load_invalid_yaml_errors() {
        let dir = std::env::temp_dir().join("phira-mp-config-test");
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("bad.yml");
        std::fs::write(&path, "server: [unclosed").unwrap();
        assert!(ServerConfig::load(path.to_str().unwrap()).is_err());
    }

    #[test]
    fn load_unknown_key_errors() {
        let dir = std::env::temp_dir().join("phira-mp-config-test");
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("unknown.yml");
        std::fs::write(&path, "server:\n  port: 12346\n  typo_key: 1\n").unwrap();
        assert!(ServerConfig::load(path.to_str().unwrap()).is_err());
    }

    #[test]
    fn merge_uses_config_when_cli_not_given() {
        let (cli, matches) = parse_cli(&["phira-mp"]);
        let cfg = ServerConfig {
            server: Some(ServerSection {
                port: Some(20000),
                ..Default::default()
            }),
            ..Default::default()
        };
        let merged = cfg.merge_into(cli, &matches);
        assert_eq!(merged.port, 20000);
        // 未出现在配置中的字段保持内置默认
        assert_eq!(merged.host, "0.0.0.0");
    }

    #[test]
    fn merge_cli_overrides_config() {
        let (cli, matches) = parse_cli(&["phira-mp", "--port", "30000"]);
        let cfg = ServerConfig {
            server: Some(ServerSection {
                port: Some(20000),
                ..Default::default()
            }),
            ..Default::default()
        };
        let merged = cfg.merge_into(cli, &matches);
        assert_eq!(merged.port, 30000);
    }

    #[test]
    fn merge_nested_cache_section() {
        let (cli, matches) = parse_cli(&["phira-mp"]);
        let cfg = ServerConfig {
            phira: Some(PhiraSection {
                cache: Some(PhiraCacheSection {
                    token_cap: Some(42),
                    ..Default::default()
                }),
                ..Default::default()
            }),
            ..Default::default()
        };
        let merged = cfg.merge_into(cli, &matches);
        assert_eq!(merged.phira_token_cache_cap, 42);
        // 未配置的缓存项保持默认
        assert_eq!(merged.phira_token_cache_ttl, 600);
    }
}
