//! 可替换的认证、谱面和成绩数据源。

use crate::phira::{ChartInfo, GameRecord, PhiraError, UserInfo};
use std::sync::{Arc, RwLock};

/// token → 用户信息。
pub type AuthProvider = Arc<
    dyn Fn(String) -> futures::future::BoxFuture<'static, Result<Arc<UserInfo>, PhiraError>>
        + Send
        + Sync,
>;
/// chart_id → 谱面信息。
pub type ChartProvider = Arc<
    dyn Fn(i32) -> futures::future::BoxFuture<'static, Result<Arc<ChartInfo>, PhiraError>>
        + Send
        + Sync,
>;
/// record_id → 成绩。
pub type RecordProvider = Arc<
    dyn Fn(i32) -> futures::future::BoxFuture<'static, Result<Arc<GameRecord>, PhiraError>>
        + Send
        + Sync,
>;

#[derive(Default)]
pub struct Providers {
    pub auth: RwLock<Option<AuthProvider>>,
    pub chart: RwLock<Option<ChartProvider>>,
    pub record: RwLock<Option<RecordProvider>>,
}

static PROVIDERS: Providers = Providers {
    auth: RwLock::new(None),
    chart: RwLock::new(None),
    record: RwLock::new(None),
};

/// 替换认证数据源（自建账号体系；对应 PlayerPreAuthenticateEvent.setUserInfo 的全局版）。
pub fn set_auth_provider(p: AuthProvider) {
    *PROVIDERS.auth.write().unwrap() = Some(p);
}

pub fn auth_provider() -> Option<AuthProvider> {
    PROVIDERS.auth.read().unwrap().clone()
}

/// 替换谱面数据源。
pub fn set_chart_provider(p: ChartProvider) {
    *PROVIDERS.chart.write().unwrap() = Some(p);
}

pub fn chart_provider() -> Option<ChartProvider> {
    PROVIDERS.chart.read().unwrap().clone()
}

/// 替换成绩数据源。
pub fn set_record_provider(p: RecordProvider) {
    *PROVIDERS.record.write().unwrap() = Some(p);
}

pub fn record_provider() -> Option<RecordProvider> {
    PROVIDERS.record.read().unwrap().clone()
}
