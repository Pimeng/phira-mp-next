//! 玩家与全局注册表（5.2、7.4 节）。
//!
//! 对应 Java 设计：
//! - [`Player`]（trait，对应 Java `Player` 接口）：纯领域身份——
//!   id/name/language/user_info/扩展数据槽/房间反查，**不持有连接**。
//!   假想一个 `RemotePlayer`（玩家状态存远端、本地无连接）时，实现本 trait 后
//!   经 [`PlayerRegistry::resolve_player`] 注入即可全链路工作（房间/事件/协议都不感知连接）。
//! - [`LocalPlayer`]（对应 Java `LocalPlayer`）：默认实现，额外持
//!   [`ConnectionReference`]（可换绑/顶号）、`kick`、`send`、在线状态。
//! - [`PlayerRegistry`]：userId 全局唯一，泛型 create/resume
//!   （对应 Java `PlayerManager.resolvePlayer(userId, clazz, constructor, resumer, closeBinder)`）。
//! - 认证/谱面/成绩数据源可整体替换（对应 Java `PhiraFetcher.GET_*` 可覆盖）。

use crate::network::connection::{ConnectionHandle, DisconnectReason};
use crate::phira::{ChartInfo, GameRecord, PhiraError, UserInfo};
use std::any::Any;
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, RwLock};

// ---------------------------------------------------------------------------
// ConnectionReference：玩家与连接之间的可重绑引用（对应 Java ConnectionReference）
// ---------------------------------------------------------------------------

/// 可重绑连接引用。顶号/断线重连时原子换绑，并处理旧连接的善后。
#[derive(Clone)]
pub struct ConnectionReference {
    inner: Arc<RwLock<ConnectionHandle>>,
}

impl ConnectionReference {
    pub fn new(conn: ConnectionHandle) -> Self {
        Self {
            inner: Arc::new(RwLock::new(conn)),
        }
    }

    /// 当前连接。
    pub fn get(&self) -> ConnectionHandle {
        self.inner.read().unwrap().clone()
    }

    /// 换绑到新连接；返回被替换的旧连接（调用方负责善后，如发「他处登录」）。
    pub fn resume(&self, new_conn: ConnectionHandle) -> ConnectionHandle {
        std::mem::replace(&mut *self.inner.write().unwrap(), new_conn)
    }

    /// 仅当当前连接 id 匹配时替换（防旧连接的清理覆盖新连接）。
    pub fn compare_exchange(&self, expect_id: u64, new_conn: ConnectionHandle) -> bool {
        let mut g = self.inner.write().unwrap();
        if g.id() == expect_id {
            *g = new_conn;
            true
        } else {
            false
        }
    }
}

// ---------------------------------------------------------------------------
// Player trait（对应 Java Player 接口；无连接，可自由实现自定义玩家）
// ---------------------------------------------------------------------------

/// 玩家领域身份。实现者无需关心网络连接（连接由 [`LocalPlayer`] 或 handler 层管理）。
///
/// # 自定义玩家（假想 `RemotePlayer`）需要做什么
/// 1. `impl Player for RemotePlayer`：提供 id/name/user_info；覆盖 [`Player::as_any`]；
///    按需覆盖扩展槽/[`Player::current_room`]（如从远端缓存反查）。
/// 2. 注册：调用 [`PlayerRegistry::resolve_player`]，constructor 构造 `Arc<RemotePlayer>`，
///    resumer 定义「旧玩家 → 新连接」的接管语义（无连接玩家可直接 `Ok(None)`）。
/// 3. 此后房间/事件/协议层只经 `Arc<dyn Player>` 使用该玩家——全链路无感知。
pub trait Player: Send + Sync + 'static {
    fn id(&self) -> i32;
    fn name(&self) -> String;
    fn user_info(&self) -> Arc<UserInfo>;

    fn language(&self) -> Option<String> {
        self.user_info().language.clone()
    }

    /// downcast 支持（自定义实现返回 `self`）。
    fn as_any(&self) -> &dyn Any;

    // ---- 扩展数据槽（对应 Java 自定义 Player 子类的字段） ----

    /// 挂接扩展数据（按 `TypeId` 存取；默认实现忽略）。
    fn set_extension_typed(&self, _type_id: std::any::TypeId, _ext: Arc<dyn Any + Send + Sync>) {}

    /// 按 `TypeId` 取扩展数据（默认实现返回 None）。
    fn extension_typed(&self, _type_id: std::any::TypeId) -> Option<Arc<dyn Any + Send + Sync>> {
        None
    }
}

/// 泛型便捷：设置扩展数据。
pub fn set_extension<T: Any + Send + Sync>(player: &dyn Player, ext: T) {
    player.set_extension_typed(std::any::TypeId::of::<T>(), Arc::new(ext));
}

/// 泛型便捷：按类型取扩展数据。
pub fn extension<T: Any + Send + Sync>(player: &dyn Player) -> Option<Arc<T>> {
    player
        .extension_typed(std::any::TypeId::of::<T>())?
        .downcast::<T>()
        .ok()
}

/// downcast：trait 对象 → 具体类型引用（经 [`Player::as_any`]）。
pub fn downcast_player<T: Player>(player: &dyn Player) -> Option<&T> {
    player.as_any().downcast_ref::<T>()
}

/// downcast：`Arc<dyn Player>` → `Arc<LocalPlayer>`（本地玩家专属操作入口）。
pub fn local_of(player: &Arc<dyn Player>) -> Option<Arc<LocalPlayer>> {
    if player.as_any().is::<LocalPlayer>() {
        // 安全：背面类型已确认为 LocalPlayer，Arc 布局一致（无 fat → fat 转换）。
        let raw = Arc::as_ptr(player) as *const LocalPlayer;
        unsafe {
            Arc::increment_strong_count(raw);
            Some(Arc::from_raw(raw))
        }
    } else {
        None
    }
}

// ---------------------------------------------------------------------------
// LocalPlayer（默认实现：持 ConnectionReference）
// ---------------------------------------------------------------------------

pub struct LocalPlayer {
    info: Arc<UserInfo>,
    connection: ConnectionReference,
    kicked: AtomicBool,
    /// 扩展数据槽：key = TypeId。
    extensions: RwLock<HashMap<std::any::TypeId, Arc<dyn Any + Send + Sync>>>,
}

impl std::fmt::Debug for LocalPlayer {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LocalPlayer")
            .field("id", &self.info.id)
            .field("name", &self.info.name)
            .finish()
    }
}

impl LocalPlayer {
    pub fn new(info: Arc<UserInfo>, conn: ConnectionHandle) -> Arc<Self> {
        Arc::new(Self {
            info,
            connection: ConnectionReference::new(conn),
            kicked: AtomicBool::new(false),
            extensions: RwLock::new(HashMap::new()),
        })
    }

    // ---- 连接（经 ConnectionReference，支持换绑/顶号） ----

    pub fn connection(&self) -> ConnectionHandle {
        self.connection.get()
    }

    pub fn connection_ref(&self) -> &ConnectionReference {
        &self.connection
    }

    /// 换绑连接（断线重连 = 连接换绑）。返回旧连接。
    pub fn bind_connection(&self, conn: ConnectionHandle) -> ConnectionHandle {
        self.connection.resume(conn)
    }

    pub fn is_online(&self) -> bool {
        !self.connection().is_closed()
    }

    /// 踢人：标记后由 room/handler 层调用 leave + close。
    pub fn kick(&self) {
        self.kicked.store(true, Ordering::SeqCst);
        let conn = self.connection();
        conn.mark_kicked();
    }

    pub fn is_kicked(&self) -> bool {
        self.kicked.load(Ordering::SeqCst)
    }

    // ---- 发包 ----

    pub async fn send(&self, packet: crate::packet::clientbound::ClientBoundPacket) {
        self.connection().send(packet).await;
    }

    /// 发送预编码共享帧（广播零拷贝路径）。
    pub async fn send_frame(&self, frame: &crate::packet::clientbound::SharedFrame) {
        self.connection().send_frame(frame.clone()).await;
    }
}

impl Player for LocalPlayer {
    fn id(&self) -> i32 {
        self.info.id
    }

    fn name(&self) -> String {
        self.info.name.clone()
    }

    fn user_info(&self) -> Arc<UserInfo> {
        self.info.clone()
    }

    fn as_any(&self) -> &dyn Any {
        self
    }

    fn set_extension_typed(&self, type_id: std::any::TypeId, ext: Arc<dyn Any + Send + Sync>) {
        self.extensions.write().unwrap().insert(type_id, ext);
    }

    fn extension_typed(&self, type_id: std::any::TypeId) -> Option<Arc<dyn Any + Send + Sync>> {
        self.extensions.read().unwrap().get(&type_id).cloned()
    }
}

// ---------------------------------------------------------------------------
// 数据源提供者（对应 Java PhiraFetcher 的可替换静态函数字段）
// ---------------------------------------------------------------------------

/// token → 用户信息。
pub type AuthProvider =
    Arc<dyn Fn(String) -> futures::future::BoxFuture<'static, Result<Arc<UserInfo>, PhiraError>>
        + Send
        + Sync>;
/// chart_id → 谱面信息。
pub type ChartProvider =
    Arc<dyn Fn(i32) -> futures::future::BoxFuture<'static, Result<Arc<ChartInfo>, PhiraError>>
        + Send
        + Sync>;
/// record_id → 成绩。
pub type RecordProvider =
    Arc<dyn Fn(i32) -> futures::future::BoxFuture<'static, Result<Arc<GameRecord>, PhiraError>>
        + Send
        + Sync>;

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

// ---------------------------------------------------------------------------
// PlayerRegistry（对应 Java PlayerManager；支持泛型创建/恢复自定义 Player）
// ---------------------------------------------------------------------------

/// 恢复挂起会话所需的上下文（由注册表取出，handler 层据此重建 RoomHandler）。
pub struct SuspendedContext {
    pub room: Arc<dyn crate::room::Room>,
}

/// resolve 结果。
pub struct ResolveResult {
    pub player: Arc<dyn Player>,
    pub created: bool,
    /// 恢复（Resume）时取出的挂起房间上下文（handler 层据此构造 RoomHandler）。
    pub suspended: Option<SuspendedContext>,
}

/// 全局玩家注册表（ConcurrentHashMap 等价物）。
pub struct PlayerRegistry {
    players: Mutex<HashMap<i32, Arc<dyn Player>>>,
}

/// 恢复结果类型（对应 Java `ResolveResult.Type`）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ResolveType {
    Create,
    Resume,
    Rebind,
}

impl Default for PlayerRegistry {
    fn default() -> Self {
        Self::new()
    }
}

impl PlayerRegistry {
    pub fn new() -> Self {
        Self {
            players: Mutex::new(HashMap::new()),
        }
    }

    /// 注册或恢复（对应 Java `resolvePlayer` 的泛型体操）：
    ///
    /// - 不存在 → 用 `constructor` 创建。
    /// - 存在且为同一连接 → 直接返回（仅 LocalPlayer 可判定）。
    /// - 存在但不同连接 → `resumer` 换绑/接管，并取出挂起会话供恢复。
    ///
    /// 返回 `(ResolveResult, 旧连接)`；旧连接若存在且未关闭，调用方负责踢旧。
    pub fn resolve_player<C, R>(
        &self,
        info: Arc<UserInfo>,
        conn: &ConnectionHandle,
        constructor: C,
        resumer: R,
    ) -> Result<(ResolveResult, Option<ConnectionHandle>), String>
    where
        C: FnOnce(Arc<UserInfo>, ConnectionHandle) -> Arc<dyn Player>,
        R: FnOnce(&Arc<dyn Player>, ConnectionHandle) -> Result<Option<ConnectionHandle>, String>,
    {
        let (player, created, old_conn) = {
            let mut map = self.players.lock().unwrap();
            match map.get(&info.id) {
                None => {
                    let player = constructor(info.clone(), conn.clone());
                    map.insert(info.id, player.clone());
                    (player, true, None)
                }
                Some(existing) => {
                    let same_conn = local_of(existing)
                        .map(|l| l.connection().id() == conn.id())
                        .unwrap_or(false);
                    if same_conn {
                        // 同一连接重复认证（不应发生）
                        (existing.clone(), false, None)
                    } else {
                        let old = resumer(existing, conn.clone())?;
                        (existing.clone(), false, old)
                    }
                }
            }
        };

        Ok((
            ResolveResult {
                player,
                created,
                suspended: None,
            },
            old_conn,
        ))
    }

    /// 默认创建 + 恢复（对应 Java AuthenticateHandler 里的 resolvePlayer 调用）。
    ///
    /// 语义对齐 Java：
    /// - 不存在 → Create。
    /// - 已在线（不同连接且连接存活）→ `error.player_already_online`。
    /// - 有挂起会话 → Resume（取出挂起上下文）。
    /// - 无挂起会话但玩家对象残留 → Rebind（换绑，无房间）。
    ///
    /// 返回 `(ResolveResult, 旧连接)`；旧连接若存在且未关闭，调用方负责踢旧。
    pub async fn resolve_or_resume(
        &self,
        info: Arc<UserInfo>,
        conn: &ConnectionHandle,
    ) -> Result<(ResolveResult, Option<ConnectionHandle>), String> {
        // 已在线且连接存活 → 拒绝重复登录
        let existing = self.players.lock().unwrap().get(&info.id).cloned();
        if let Some(p) = &existing {
            let online = local_of(p)
                .map(|l| !l.connection().is_closed() && l.connection().id() != conn.id())
                .unwrap_or(false);
            if online {
                return Err("error.player_already_online".to_string());
            }
        }

        // 尝试恢复挂起会话（take 语义；校验玩家仍在原房间）
        let suspended = match &existing {
            Some(p) => crate::server::with_server_ctx(|ctx| {
                tracing::info!("Resolve or resume: checking suspended (user {})", p.id());
                let taken = ctx.sessions.take_suspended(p.id());
                tracing::info!("Resolve or resume: taking suspended (user {})", p.id());
                if taken.is_some() {
                    tracing::info!("Resolve or resume: filter check (user {})", p.id());
                }
                taken
                    .filter(|s| s.room.contains_member(p.id()))
                    .map(|s| SuspendedContext { room: s.room })
            })
            .flatten(),
            None => None,
        };

        let resolve_type = match (&existing, &suspended) {
            (None, _) => ResolveType::Create,
            (Some(_), Some(_)) => ResolveType::Resume,
            (Some(_), None) => ResolveType::Rebind,
        };

        self.resolve_player(
            info,
            conn,
            |info, c| LocalPlayer::new(info, c),
            |player, new_conn| {
                // 换绑：取出旧连接，若存活由调用方踢旧
                local_of(player)
                    .ok_or_else(|| "error.player_type_unsupported".to_string())
                    .map(|l| Some(l.bind_connection(new_conn)))
            },
        )
        .map(|(mut result, old)| {
            result.suspended = suspended;
            result.created = resolve_type == ResolveType::Create;
            (result, old)
        })
    }

    /// 移除注册（挂起失败/会话超时/踢出时调用）。仅当注册的连接仍是指定连接时移除。
    pub fn remove_if_bound(&self, user_id: i32, conn_id: u64) {
        let mut map = self.players.lock().unwrap();
        if let Some(p) = map.get(&user_id) {
            let matches = local_of(p)
                .map(|l| l.connection().id() == conn_id)
                .unwrap_or(false);
            if matches {
                map.remove(&user_id);
            }
        }
    }

    pub fn remove(&self, user_id: i32) {
        self.players.lock().unwrap().remove(&user_id);
    }

    pub fn get(&self, user_id: i32) -> Option<Arc<dyn Player>> {
        self.players.lock().unwrap().get(&user_id).cloned()
    }

    pub fn is_online(&self, user_id: i32) -> bool {
        self.get(user_id)
            .and_then(|p| local_of(&p).map(|l| l.is_online()))
            .unwrap_or(false)
    }

    pub fn online_players(&self) -> Vec<Arc<LocalPlayer>> {
        self.players
            .lock()
            .unwrap()
            .values()
            .filter_map(|p| local_of(p).filter(|l| l.is_online()))
            .collect()
    }

    pub fn all_players(&self) -> Vec<Arc<dyn Player>> {
        self.players.lock().unwrap().values().cloned().collect()
    }

    /// 断线原因映射（对应 Java PlayerDisconnectEvent.DisconnectReason）。
    pub fn disconnect_reason_of(conn: &ConnectionHandle) -> DisconnectReason {
        conn.disconnect_reason()
    }
}
