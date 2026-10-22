//! 命令行参数、默认值与配置文件合并入口。

use crate::config::ServerConfig;
use clap::Parser;

/// 命令行参数（1.2 节）。
///
/// 配置来源优先级：CLI 显式参数 > `config.yml` > 内置默认值。
/// 见 [`ServerArgs::load`] 与 [`crate::config`]。
#[derive(Parser, Debug, Clone)]
#[command(name = "phira-mp", about = "Phira multiplayer server")]
pub struct ServerArgs {
    /// 配置文件路径（默认自动加载 ./config.yml）
    #[arg(long, default_value = "config.yml")]
    pub config: String,

    /// 偷听端口
    #[arg(long, default_value_t = 12346, value_parser = clap::value_parser!(u16).range(1..))]
    pub port: u16,

    /// 绑定地址
    #[arg(long, default_value = "0.0.0.0")]
    pub host: String,

    /// HTTP API 偷听端口（0 = 禁用 HTTP 服务；偷听地址复用 `host`）
    #[arg(long, default_value_t = 12347, value_parser = clap::value_parser!(u16).range(0..))]
    pub http_port: u16,

    /// 启用 HAProxy PROXY 协议
    #[arg(long, default_value_t = false)]
    pub proxy_protocol: bool,

    /// 服务器默认语言（i18n 回退链的第二级：玩家语言 → 服务器默认 → zh-CN → key）
    #[arg(long, alias = "lang", default_value = "zh-CN")]
    pub language: String,

    /// 会话挂起超时（秒）
    #[arg(long, default_value_t = 300)]
    pub session_timeout: u64,

    /// Phira API Base URL
    #[arg(long, default_value = "https://phira.5wyxi.com/")]
    pub phira_api: String,

    /// 对局录制输出目录（不设置则禁用录制）
    #[arg(long)]
    pub record_dir: Option<String>,

    /// Phira API GET 最大尝试次数
    #[arg(long, default_value_t = 5)]
    pub phira_max_attempts: u32,

    /// Phira API 重试退避基数（毫秒）
    #[arg(long, default_value_t = 150)]
    pub phira_retry_base_ms: u64,

    /// Phira token 缓存 TTL（秒）
    #[arg(long, default_value_t = 600)]
    pub phira_token_cache_ttl: u64,

    /// Phira token 缓存容量
    #[arg(long, default_value_t = 10000)]
    pub phira_token_cache_cap: usize,

    /// Phira 用户缓存 TTL（秒）
    #[arg(long, default_value_t = 600)]
    pub phira_user_cache_ttl: u64,

    /// Phira 用户缓存容量
    #[arg(long, default_value_t = 5000)]
    pub phira_user_cache_cap: usize,

    /// Phira 谱面缓存 TTL（秒）
    #[arg(long, default_value_t = 1800)]
    pub phira_chart_cache_ttl: u64,

    /// Phira 谱面缓存容量
    #[arg(long, default_value_t = 10000)]
    pub phira_chart_cache_cap: usize,

    /// Phira 成绩缓存 TTL（秒）
    #[arg(long, default_value_t = 1800)]
    pub phira_record_cache_ttl: u64,

    /// Phira 成绩缓存容量
    #[arg(long, default_value_t = 50000)]
    pub phira_record_cache_cap: usize,

    /// 握手超时（秒）
    #[arg(long, default_value_t = 5)]
    pub handshake_timeout: u64,

    /// 读帧超时（秒）
    #[arg(long, default_value_t = 5)]
    pub read_timeout: u64,

    /// PROXY 协议解析超时（秒）
    #[arg(long, default_value_t = 5)]
    pub proxy_timeout: u64,

    /// 新建房间默认最大人数
    #[arg(long, default_value_t = 8)]
    pub default_max_player: usize,
}

impl ServerArgs {
    /// 加载最终配置：CLI 显式参数 > config.yml > 内置默认值。
    ///
    /// 参数解析错误（含 `--help`）由 clap 直接处理（打印并退出）；
    /// 仅配置文件读取/解析失败返回 `Err`。
    pub fn load() -> std::io::Result<ServerArgs> {
        use clap::{CommandFactory, FromArgMatches};
        let matches = ServerArgs::command().get_matches();
        let cli = ServerArgs::from_arg_matches(&matches).unwrap_or_else(|e| e.exit());
        let cfg = ServerConfig::load(&cli.config)?;
        Ok(cfg.merge_into(cli, &matches))
    }
}

/// 标准默认配置（与 clap 默认值一致；测试用 `..Default::default()` 补全字段）。
/// 注意：修改 clap 默认值时需同步更新这里。
impl Default for ServerArgs {
    fn default() -> Self {
        Self {
            config: "config.yml".into(),
            port: 12346,
            host: "0.0.0.0".into(),
            http_port: 12347,
            proxy_protocol: false,
            language: "zh-CN".into(),
            session_timeout: 300,
            phira_api: "https://phira.5wyxi.com/".into(),
            record_dir: None,
            phira_max_attempts: 5,
            phira_retry_base_ms: 150,
            phira_token_cache_ttl: 600,
            phira_token_cache_cap: 10000,
            phira_user_cache_ttl: 600,
            phira_user_cache_cap: 5000,
            phira_chart_cache_ttl: 1800,
            phira_chart_cache_cap: 10000,
            phira_record_cache_ttl: 1800,
            phira_record_cache_cap: 50000,
            handshake_timeout: 5,
            read_timeout: 5,
            proxy_timeout: 5,
            default_max_player: 8,
        }
    }
}
