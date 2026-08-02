//! 房间与状态机（第 6、7 节）。
//!
//! 对应 Java 设计（引用关系与职责拆分）：
//! - [`Room`]（trait，对应 Java `Room` 接口）：领域接口——成员/操作/快照，
//!   **不持连接、不依赖具体 Player 实现**。假想 `RemoteRoom` 实现本 trait 后经
//!   [`RoomRegistry`] 工厂注入即可全链路工作。
//! - [`local::LocalRoom`]（对应 Java `LocalRoom`）：默认实现——内嵌成员管理器、
//!   host 轮转、锁内决策/锁外广播。
//! - [`state::RoomState`]：对局状态机（sealed 三态）。
//! - [`behavior`]：操作层（对应 Java `Room.Operation` / `LocalOperation`）。
//! - [`RoomRegistry`] + [`RoomFactory`]：注册表与自定义房间工厂
//!   （对应 Java `RoomManager.resolveRoom(roomId, factory)`）。

pub mod behavior;
pub mod local;
pub mod state;

pub use local::LocalRoom;

use crate::packet::PacketResult;
use crate::packet::clientbound::{JoinRoomData, SharedFrame};
use crate::packet::data::RoomInfo;
use crate::packet::state::GameState;
use crate::player::Player;
use std::sync::{Arc, Weak};

/// 业务操作错误（对应 GameOperationException，message 为 i18n key）。
#[derive(Debug)]
pub struct GameError(pub &'static str);

impl std::fmt::Display for GameError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

pub type GameResult<T> = Result<T, GameError>;

pub const DEFAULT_MAX_PLAYER: usize = 8;

#[derive(Debug, Clone)]
pub struct RoomSetting {
    pub auto_destroy: bool,
    pub host: bool,
    pub max_player: usize,
    pub locked: bool,
    pub cycle: bool,
    pub live: bool,
    pub chat: bool,
}

impl Default for RoomSetting {
    fn default() -> Self {
        Self {
            auto_destroy: true,
            host: true,
            max_player: DEFAULT_MAX_PLAYER,
            locked: false,
            cycle: false,
            live: false,
            chat: true,
        }
    }
}

/// 加入结果（锁外广播用）。
#[derive(Debug)]
pub enum JoinOutcome {
    AlreadyIn,
    FirstPlayer,
    Joined { is_monitor: bool },
}

/// played 校验结果（避免 None 二义性）。
pub enum PlayedCheck {
    CanPlay {
        chart_id: Option<i32>,
        chart_name: Option<String>,
    },
    AlreadyDone,
}

/// played 时提取的录制数据（第 10 节）。
pub struct RecordingData {
    pub chart_id: Option<i32>,
    pub chart_name: Option<String>,
    pub touch_frames: Vec<crate::packet::data::TouchFrame>,
    pub judge_events: Vec<crate::packet::data::JudgeEvent>,
}

pub struct CommitGameOutcome {
    pub broadcasts: behavior::Broadcast,
    pub game_ended: bool,
    pub recording: Option<RecordingData>,
}

pub struct RoomSnapshot {
    pub room_id: String,
    pub state_kind_name: String,
    pub locked: bool,
    pub players: Vec<i32>,
    pub monitors: Vec<i32>,
    /// 房主（setting.host 关闭时为 None）。
    pub host: Option<i32>,
    /// 当前选曲（SelectChart/WaitForReady/Playing 均有）。
    pub chart_id: Option<i32>,
    pub chart_name: Option<String>,
}

impl RoomSnapshot {
    pub fn state_kind(&self) -> &str {
        &self.state_kind_name
    }
}

// ---------------------------------------------------------------------------
// Room trait（对应 Java Room 接口；可自由实现自定义房间）
// ---------------------------------------------------------------------------

/// 房间领域接口。**不持连接**，操作产出「广播计划」由 handler 锁外发送。
///
/// # 自定义房间（假想 `RemoteRoom`）需要做什么
/// 1. `impl Room for RemoteRoom`：实现成员/操作/快照全部方法；
///    `join`/`leave` 等返回 [`behavior::Broadcast`]（可为空，自行决定通知语义）。
/// 2. 注册：`ctx.rooms.set_factory(...)` 返回 `Arc<RemoteRoom>`
///    （对应 Java `RoomManager.resolveRoom(roomId, onDestroy -> new RemoteRoom(...))`）。
/// 3. 此后 handler/会话/事件层只经 `Arc<dyn Room>` 使用——全链路无感知。
pub trait Room: Send + Sync + 'static {
    fn id(&self) -> &str;

    // ---- 查询 ----
    fn contains_member(&self, user_id: i32) -> bool;
    fn contains_monitor(&self, user_id: i32) -> bool;
    fn is_monitor_user(&self, user_id: i32) -> bool;
    fn is_host(&self, user_id: i32) -> bool;
    fn is_destroyed(&self) -> bool;
    fn game_state_protocol(&self) -> GameState;
    fn setting(&self) -> RoomSetting;
    fn build_room_info(&self, viewer: &dyn Player) -> RoomInfo;
    fn join_room_data(&self) -> JoinRoomData;
    fn snapshot(&self) -> RoomSnapshot;
    fn game_records(&self) -> Vec<(i32, i32, f32, bool)>;

    // ---- 成员管理 ----
    fn join(
        &self,
        player: Arc<dyn Player>,
        is_monitor: bool,
    ) -> GameResult<(JoinOutcome, behavior::Broadcast)>;
    fn leave(&self, player_id: i32) -> (bool, behavior::Broadcast, bool);

    // ---- 鉴权 ----
    fn validate_host(&self, user_id: i32) -> GameResult<()>;

    // ---- 操作（锁内决策，产出广播计划） ----
    fn toggle_lock(&self, user_id: i32) -> GameResult<behavior::Broadcast>;
    fn toggle_cycle(&self, user_id: i32) -> GameResult<behavior::Broadcast>;
    fn chat(&self, user_id: i32, content: String) -> GameResult<behavior::Broadcast>;
    fn validate_select_chart(&self, user_id: i32) -> GameResult<()>;
    fn commit_select_chart(
        &self,
        user_id: i32,
        chart_id: i32,
        chart_name: String,
    ) -> GameResult<behavior::Broadcast>;
    fn require_start(&self, user_id: i32) -> GameResult<behavior::Broadcast>;
    fn ready(&self, user_id: i32) -> GameResult<(behavior::Broadcast, bool)>;
    fn cancel_ready(&self, user_id: i32) -> GameResult<behavior::Broadcast>;
    fn check_played(&self, user_id: i32) -> GameResult<PlayedCheck>;
    fn commit_played(
        &self,
        user_id: i32,
        score: i32,
        accuracy: f32,
        full_combo: bool,
    ) -> GameResult<CommitGameOutcome>;
    fn commit_abort(&self, user_id: i32) -> GameResult<CommitGameOutcome>;
    fn touch_send(
        &self,
        user_id: i32,
        frames: Vec<crate::packet::data::TouchFrame>,
    ) -> behavior::Broadcast;
    fn judge_send(
        &self,
        user_id: i32,
        judges: Vec<crate::packet::data::JudgeEvent>,
    ) -> behavior::Broadcast;
    fn cleanup_for_suspend(&self, user_id: i32) -> behavior::Broadcast;

    // ---- 管理员操作（控制台命令专用；绕过玩家鉴权，默认不支持） ----

    /// 设置房间最大人数（控制台 `maxusers`）。
    fn admin_set_max_player(&self, _count: usize) -> GameResult<()> {
        Err(GameError("ERROR_UNSUPPORTED"))
    }

    /// 强制锁定/解锁（控制台 `lock`；不要求房主）。
    fn admin_set_locked(&self, _locked: bool) -> GameResult<behavior::Broadcast> {
        Err(GameError("ERROR_UNSUPPORTED"))
    }

    /// 开关循环模式（控制台 `cycle`；不要求房主）。
    fn admin_set_cycle(&self, _cycle: bool) -> GameResult<behavior::Broadcast> {
        Err(GameError("ERROR_UNSUPPORTED"))
    }

    /// 指定下一轮房主（控制台 `nexthost`；仅循环模式对局结束时生效）。
    fn admin_set_next_host(&self, _user_id: i32) -> GameResult<()> {
        Err(GameError("ERROR_UNSUPPORTED"))
    }

    /// 立即转移房主（控制台 `sethost`）。
    fn admin_transfer_host(&self, _user_id: i32) -> GameResult<behavior::Broadcast> {
        Err(GameError("ERROR_UNSUPPORTED"))
    }

    /// 管理员向房间广播消息（控制台 `roomsay`；绕过 chat 开关）。
    fn admin_chat(&self, _content: String) -> GameResult<behavior::Broadcast> {
        Err(GameError("ERROR_UNSUPPORTED"))
    }
}

/// 发送广播计划（锁外执行；共享帧零拷贝）。
pub async fn send_broadcasts(plan: behavior::Broadcast) {
    behavior::deliver(plan).await;
}

/// 便捷：直接发共享帧给一名玩家（自定义玩家经 `Player::send_frame` 接收）。
pub async fn send_frame_to(player: &Arc<dyn crate::player::Player>, frame: &SharedFrame) {
    player.send_frame(frame.clone()).await;
}

// ---------------------------------------------------------------------------
// RoomRegistry + RoomFactory（对应 Java RoomManager.resolveRoom 的工厂注入）
// ---------------------------------------------------------------------------

/// 房间工厂（对应 Java 自定义 Room 实现经 `resolveRoom` 注入）。
/// 第三参数为「销毁自清理」回调（注册表移除）。
pub type RoomFactory =
    Arc<dyn Fn(String, RoomSetting, Box<dyn Fn() + Send + Sync>) -> Arc<dyn Room> + Send + Sync>;

/// 房间注册表（7.3 节；弱引用 + on_destroy 自清理）。
pub struct RoomRegistry {
    rooms: Mutex<std::collections::HashMap<String, Weak<dyn Room>>>,
    factory: Mutex<RoomFactory>,
}

use std::sync::Mutex;

impl Default for RoomRegistry {
    fn default() -> Self {
        Self::new()
    }
}

impl RoomRegistry {
    pub fn new() -> Self {
        Self {
            rooms: Mutex::new(std::collections::HashMap::new()),
            factory: Mutex::new(default_factory()),
        }
    }

    /// 替换房间工厂（自定义房间行为/设置的入口）。
    pub fn set_factory(&self, factory: RoomFactory) {
        *self.factory.lock().unwrap() = factory;
    }

    /// 创建房间（默认设置）；已存在返回 error.room_already_exists。
    pub fn create_room(&self, room_id: &str) -> GameResult<Arc<dyn Room>> {
        self.create_room_with(room_id, RoomSetting::default())
    }

    /// 以指定设置创建房间（RoomPreCreateEvent 可改写 setting）。
    pub fn create_room_with(
        &self,
        room_id: &str,
        setting: RoomSetting,
    ) -> GameResult<Arc<dyn Room>> {
        let mut map = self.rooms.lock().unwrap();
        if let Some(existing) = map.get(room_id).and_then(|w| w.upgrade())
            && !existing.is_destroyed()
        {
            return Err(GameError("ERROR_ROOM_ALREADY_EXISTS"));
        }
        let rid = room_id.to_string();
        let on_destroy: Box<dyn Fn() + Send + Sync> = {
            let rid = rid.clone();
            Box::new(move || {
                crate::server::with_server_ctx(|ctx| ctx.rooms.remove(&rid));
            })
        };
        let room = (self.factory.lock().unwrap())(rid, setting, on_destroy);
        map.insert(room_id.to_string(), Arc::downgrade(&room));
        Ok(room)
    }

    pub fn find_room(&self, room_id: &str) -> Option<Arc<dyn Room>> {
        let mut map = self.rooms.lock().unwrap();
        let room = map.get(room_id).and_then(|w| w.upgrade());
        if room.is_none() {
            map.remove(room_id); // 清理失效弱引用
        }
        room.filter(|r| !r.is_destroyed())
    }

    pub fn remove(&self, room_id: &str) {
        self.rooms.lock().unwrap().remove(room_id);
    }

    pub fn all_rooms(&self) -> Vec<Arc<dyn Room>> {
        let mut map = self.rooms.lock().unwrap();
        let mut out = Vec::new();
        let mut dead = Vec::new();
        for (k, w) in map.iter() {
            match w.upgrade() {
                Some(r) if !r.is_destroyed() => out.push(r),
                _ => dead.push(k.clone()),
            }
        }
        for k in dead {
            map.remove(&k);
        }
        out
    }
}

fn default_factory() -> RoomFactory {
    Arc::new(|id, setting, on_destroy| LocalRoom::new(id, setting, on_destroy))
}

impl PacketResult<()> {
    pub fn from_game_result(
        r: GameResult<()>,
        i18n: &crate::i18n::I18nService,
        lang: Option<&str>,
    ) -> Self {
        match r {
            Ok(()) => PacketResult::ok(),
            Err(e) => PacketResult::failed(i18n.message(lang, e.0)),
        }
    }
}
