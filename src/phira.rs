//! Phira API 客户端（第 4 节）：HTTP + 重试 + 缓存。

use serde::Deserialize;
use std::num::NonZeroUsize;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use crate::server::ServerArgs;

/// Phira API 客户端调优参数（来自 config.yml / CLI）。
#[derive(Debug, Clone)]
pub struct PhiraFetcherConfig {
    /// Phira API Base URL。
    pub api_url: String,
    /// GET 请求最大尝试次数。
    pub max_attempts: u32,
    /// 重试退避基数（毫秒，退避 = base × attempt）。
    pub retry_base_ms: u64,
    pub token_cache_ttl: Duration,
    pub token_cache_cap: usize,
    pub user_cache_ttl: Duration,
    pub user_cache_cap: usize,
    pub chart_cache_ttl: Duration,
    pub chart_cache_cap: usize,
    pub record_cache_ttl: Duration,
    pub record_cache_cap: usize,
}

impl Default for PhiraFetcherConfig {
    fn default() -> Self {
        Self {
            api_url: "https://phira.5wyxi.com/".into(),
            max_attempts: 5,
            retry_base_ms: 150,
            token_cache_ttl: Duration::from_secs(600), // 10min
            token_cache_cap: 10000,
            user_cache_ttl: Duration::from_secs(600),
            user_cache_cap: 5000,
            chart_cache_ttl: Duration::from_secs(1800), // 30min
            chart_cache_cap: 10000,
            record_cache_ttl: Duration::from_secs(1800),
            record_cache_cap: 50000,
        }
    }
}

impl From<&ServerArgs> for PhiraFetcherConfig {
    fn from(args: &ServerArgs) -> Self {
        Self {
            api_url: args.phira_api.clone(),
            max_attempts: args.phira_max_attempts,
            retry_base_ms: args.phira_retry_base_ms,
            token_cache_ttl: Duration::from_secs(args.phira_token_cache_ttl),
            token_cache_cap: args.phira_token_cache_cap,
            user_cache_ttl: Duration::from_secs(args.phira_user_cache_ttl),
            user_cache_cap: args.phira_user_cache_cap,
            chart_cache_ttl: Duration::from_secs(args.phira_chart_cache_ttl),
            chart_cache_cap: args.phira_chart_cache_cap,
            record_cache_ttl: Duration::from_secs(args.phira_record_cache_ttl),
            record_cache_cap: args.phira_record_cache_cap,
        }
    }
}

#[derive(Debug, Clone, Deserialize, Default)]
#[serde(default)]
pub struct UserInfo {
    pub id: i32,
    pub name: String,
    pub avatar: String,
    pub language: Option<String>,
    pub rks: f64,
    pub banned: bool,
    #[serde(rename = "loginBanned")]
    pub login_banned: bool,
    pub roles: i64,
    pub exp: i64,
}

#[derive(Debug, Clone, Deserialize, Default)]
#[serde(default)]
pub struct ChartInfo {
    pub id: i32,
    pub name: String,
    pub level: String,
    pub difficulty: f32,
    pub charter: String,
    pub composer: String,
    pub ranked: bool,
    pub uploader: i32,
}

#[derive(Debug, Clone, Deserialize, Default)]
#[serde(default)]
pub struct GameRecord {
    pub id: i32,
    pub player: i32,
    pub chart: i32,
    pub score: i32,
    pub accuracy: f32,
    pub perfect: i32,
    pub good: i32,
    pub bad: i32,
    pub miss: i32,
    #[serde(rename = "fullCombo")]
    pub full_combo: bool,
    #[serde(rename = "maxCombo")]
    pub max_combo: i32,
    pub speed: f32,
    pub time: Option<String>,
}

#[derive(Debug)]
pub enum PhiraError {
    Http(String),
    NotFound(String),
}

impl std::fmt::Display for PhiraError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            PhiraError::Http(m) => write!(f, "http error: {m}"),
            PhiraError::NotFound(m) => write!(f, "not found: {m}"),
        }
    }
}

impl std::error::Error for PhiraError {}

/// 简单 TTL + LRU 缓存（等价 Caffeine 的 expireAfterWrite + maximumSize）。
struct TtlCache<K: std::hash::Hash + Eq + Clone, V: Clone> {
    map: lru::LruCache<K, (V, Instant)>,
    ttl: Duration,
}

impl<K: std::hash::Hash + Eq + Clone, V: Clone> TtlCache<K, V> {
    fn new(ttl: Duration, cap: usize) -> Self {
        Self {
            map: lru::LruCache::new(NonZeroUsize::new(cap).unwrap()),
            ttl,
        }
    }

    fn get(&mut self, key: &K) -> Option<V> {
        let (v, t) = self.map.get(key)?;
        if t.elapsed() > self.ttl {
            self.map.pop(key);
            return None;
        }
        Some(v.clone())
    }

    fn put(&mut self, key: K, value: V) {
        self.map.put(key, (value, Instant::now()));
    }
}

pub struct PhiraFetcher {
    client: reqwest::Client,
    base_url: String,
    max_attempts: u32,
    retry_base_ms: u64,
    token_cache: Mutex<TtlCache<String, Arc<UserInfo>>>,
    user_cache: Mutex<TtlCache<i32, Arc<UserInfo>>>,
    chart_cache: Mutex<TtlCache<i32, Arc<ChartInfo>>>,
    record_cache: Mutex<TtlCache<i32, Arc<GameRecord>>>,
}

impl PhiraFetcher {
    pub fn new(config: PhiraFetcherConfig) -> Self {
        let client = reqwest::Client::builder()
            .connect_timeout(Duration::from_secs(5))
            .timeout(Duration::from_secs(10))
            .user_agent("JPhira/1")
            .build()
            .expect("reqwest client");
        Self {
            client,
            base_url: config.api_url,
            max_attempts: config.max_attempts,
            retry_base_ms: config.retry_base_ms,
            token_cache: Mutex::new(TtlCache::new(
                config.token_cache_ttl,
                config.token_cache_cap,
            )),
            user_cache: Mutex::new(TtlCache::new(config.user_cache_ttl, config.user_cache_cap)),
            chart_cache: Mutex::new(TtlCache::new(config.chart_cache_ttl, config.chart_cache_cap)),
            record_cache: Mutex::new(TtlCache::new(
                config.record_cache_ttl,
                config.record_cache_cap,
            )),
        }
    }

    fn url(&self, path: &str) -> String {
        format!("{}/{}", self.base_url.trim_end_matches('/'), path)
    }

    /// GET with retry：最多 5 次，退避 150/300/450/600/750ms；非 2xx 视为失败。
    async fn get_json<T: serde::de::DeserializeOwned>(
        &self,
        path: &str,
        token: Option<&str>,
    ) -> Result<T, PhiraError> {
        let url = self.url(path);
        let mut last_err = String::new();
        for attempt in 0..self.max_attempts {
            let mut req = self.client.get(&url).header("Accept", "application/json");
            if let Some(t) = token {
                req = req.bearer_auth(t);
            }
            match req.send().await {
                Ok(resp) if resp.status().is_success() => {
                    crate::log::log_debug_global!(
                        "LOG_PHIRA_FETCH_OK",
                        ("path", path),
                        ("status", resp.status().to_string()),
                    );
                    return resp
                        .json::<T>()
                        .await
                        .map_err(|e| PhiraError::Http(format!("json decode: {e}")));
                }
                Ok(resp) => {
                    last_err = format!("status {}", resp.status());
                    crate::log::log_debug_global!(
                        "LOG_PHIRA_FETCH_NON_2XX",
                        ("path", path),
                        ("err", last_err.clone()),
                    );
                    // 404 不重试，直接判为不存在
                    if resp.status() == reqwest::StatusCode::NOT_FOUND {
                        return Err(PhiraError::NotFound(path.to_string()));
                    }
                }
                Err(e) => {
                    last_err = e.to_string();
                    crate::log::log_debug_global!(
                        "LOG_PHIRA_FETCH_TRANSPORT",
                        ("path", path),
                        ("err", last_err.clone()),
                    );
                }
            }
            crate::log::log_debug_global!(
                "LOG_PHIRA_FETCH_RETRY",
                ("path", path),
                ("attempt", attempt),
            );
            tokio::time::sleep(Duration::from_millis(
                self.retry_base_ms * (attempt as u64 + 1),
            ))
            .await;
        }
        Err(PhiraError::Http(last_err))
    }

    /// token → 用户信息（GET /me）。
    pub async fn get_user_info(&self, token: &str) -> Result<Arc<UserInfo>, PhiraError> {
        if let Some(v) = self.token_cache.lock().unwrap().get(&token.to_string()) {
            return Ok(v);
        }
        let info: UserInfo = self.get_json("me", Some(token)).await?;
        let info = Arc::new(info);
        self.token_cache
            .lock()
            .unwrap()
            .put(token.to_string(), info.clone());
        Ok(info)
    }

    /// userId → 用户（GET /user/{id}）。
    #[allow(dead_code)]
    pub async fn get_user(&self, id: i32) -> Result<Arc<UserInfo>, PhiraError> {
        if let Some(v) = self.user_cache.lock().unwrap().get(&id) {
            return Ok(v);
        }
        let info: UserInfo = self.get_json(&format!("user/{id}"), None).await?;
        let info = Arc::new(info);
        self.user_cache.lock().unwrap().put(id, info.clone());
        Ok(info)
    }

    /// chartId → 谱面（GET /chart/{id}）。
    pub async fn get_chart_info(&self, id: i32) -> Result<Arc<ChartInfo>, PhiraError> {
        if let Some(v) = self.chart_cache.lock().unwrap().get(&id) {
            return Ok(v);
        }
        let info: ChartInfo = self.get_json(&format!("chart/{id}"), None).await?;
        let info = Arc::new(info);
        self.chart_cache.lock().unwrap().put(id, info.clone());
        Ok(info)
    }

    /// recordId → 成绩（GET /record/{id}）。
    pub async fn get_record_info(&self, id: i32) -> Result<Arc<GameRecord>, PhiraError> {
        if let Some(v) = self.record_cache.lock().unwrap().get(&id) {
            return Ok(v);
        }
        let info: GameRecord = self.get_json(&format!("record/{id}"), None).await?;
        let info = Arc::new(info);
        self.record_cache.lock().unwrap().put(id, info.clone());
        Ok(info)
    }
}
