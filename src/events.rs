//! 服务器扩展事件（对应 Java `main.event` 包的 35 个事件）。
//!
//! 约定：
//! - 只读事件 → `EventBus::post(key, event)`。
//! - 可取消 / 可改写事件 → `EventBus::post_mut(key, event)`；
//!   `cancel(reason)` 后服务端按 `cancel_reason` 回复玩家。
//!
//! 无插件系统：扩展方通过 `ctx.events.subscribe*` 直接注册（库使用者/嵌入方）。

use crate::network::connection::DisconnectReason;
use crate::network::handler::PacketHandler;
use crate::packet::clientbound::ClientBoundPacket;
use crate::packet::serverbound::ServerBoundPacket;
use crate::phira::{ChartInfo, UserInfo};
use crate::player::Player;
use crate::room::{Room, RoomSetting};
use std::sync::Arc;

// ---------------- 事件 key ----------------

pub const SERVER_LIFECYCLE: &str = "server.lifecycle";
pub const COMMAND_PROCESS: &str = "server.command";

pub const PLAYER_PRE_AUTHENTICATE: &str = "player.pre_authenticate";
pub const PLAYER_PRE_LOGIN: &str = "player.pre_login";
pub const PLAYER_CREATE: &str = "player.create";
pub const PLAYER_POST_LOGIN: &str = "player.post_login";
pub const PLAYER_CONNECTION_BIND: &str = "player.connection_bind";
pub const PLAYER_DISCONNECT: &str = "player.disconnect";
pub const PLAYER_UNREGISTER: &str = "player.unregister";

pub const PACKET_RECEIVE: &str = "network.packet_receive";
pub const PACKET_SEND: &str = "network.packet_send";
pub const PLAYER_SWITCH_PACKET_HANDLER: &str = "network.switch_packet_handler";

pub const ROOM_PRE_CREATE: &str = "room.pre_create";
pub const ROOM_POST_CREATE: &str = "room.post_create";
pub const PLAYER_PRE_JOIN_ROOM: &str = "room.pre_join";
pub const PLAYER_POST_JOIN_ROOM: &str = "room.post_join";
pub const PLAYER_JOIN_ROOM_SUCCESS: &str = "room.join_success";
pub const PLAYER_LEAVE_ROOM: &str = "room.leave";
pub const ROOM_DESTROY: &str = "room.destroy";
pub const ROOM_HOST_CHANGE: &str = "room.host_change";
pub const ROOM_STATE_CHANGE: &str = "room.state_change";

pub const ROOM_PRE_SELECT_CHART: &str = "op.pre_select_chart";
pub const ROOM_POST_SELECT_CHART: &str = "op.post_select_chart";
pub const ROOM_CHAT: &str = "op.chat";
pub const ROOM_LOCK_CHANGE: &str = "op.lock_change";
pub const ROOM_CYCLE_CHANGE: &str = "op.cycle_change";

pub const GAME_REQUIRE_START: &str = "game.require_start";
pub const GAME_START: &str = "game.start";
pub const GAME_PLAYING_START: &str = "game.playing_start";
pub const PLAYER_READY: &str = "game.ready";
pub const PLAYER_CANCEL_READY: &str = "game.cancel_ready";
pub const PLAYER_PLAYED: &str = "game.played";
pub const GAME_ABORT: &str = "game.abort";
pub const GAME_END: &str = "game.end";

pub const PLAYER_SESSION_SUSPEND: &str = "session.suspend";
pub const PLAYER_SESSION_TIMEOUT: &str = "session.timeout";

// ---------------- 可取消基元（内禀方法宏，避免孤儿规则问题） ----------------

macro_rules! cancellable {
    ($t:ty) => {
        impl $t {
            pub fn is_cancelled(&self) -> bool {
                self.cancel_reason.is_some()
            }
            pub fn cancel_reason(&self) -> Option<&str> {
                self.cancel_reason.as_deref()
            }
            /// 取消并给出原因（原因透传给玩家）。
            pub fn cancel(&mut self, reason: impl Into<String>) {
                self.cancel_reason = Some(reason.into());
            }
        }
    };
}

// ---------------- server ----------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LifecyclePhase {
    Started,
    Stopping,
    Stopped,
}

pub struct ServerLifecycleEvent {
    pub phase: LifecyclePhase,
}

/// 控制台命令事件（对应 CommandProcessEvent）：
/// 任一订阅者 `cancel` 即视为「已处理」，否则输出 unknown command。
pub struct CommandProcessEvent {
    /// 完整命令行（含参数）。
    pub command: String,
    pub cancel_reason: Option<String>,
}
cancellable!(CommandProcessEvent);

// ---------------- player ----------------

/// 认证前事件：可取消（拒绝登录）；可注入 `user_info` 跳过远程验证（自建账号体系）。
pub struct PlayerPreAuthenticateEvent {
    pub token: String,
    pub user_info: Option<Arc<UserInfo>>,
    pub cancel_reason: Option<String>,
}
cancellable!(PlayerPreAuthenticateEvent);

/// 登录前事件：用户信息已就绪，可取消（白名单/封禁）。
pub struct PlayerPreLoginEvent {
    pub user_info: Arc<UserInfo>,
    pub cancel_reason: Option<String>,
}
cancellable!(PlayerPreLoginEvent);

pub struct PlayerCreateEvent {
    pub player: Arc<dyn Player>,
}

pub struct PlayerPostLoginEvent {
    pub player: Arc<dyn Player>,
}

pub struct PlayerConnectionBindEvent {
    pub player: Arc<dyn Player>,
}

/// 断线事件：**可取消**——订阅者 `cancel` 后接管整个断线清理流程
/// （默认清理不再执行，订阅者自行负责移除注册/退房/挂起）。
pub struct PlayerDisconnectEvent {
    pub player: Arc<dyn Player>,
    pub reason: DisconnectReason,
    pub cancel_reason: Option<String>,
}
cancellable!(PlayerDisconnectEvent);

/// 玩家从注册表移除事件（观察用，不可拦截）。
pub struct PlayerUnregisterEvent {
    pub player: Arc<dyn Player>,
}

// ---------------- network ----------------

/// 入站包事件：可取消（包被丢弃，不再进入 handler）。
pub struct PacketReceiveEvent {
    pub packet: ServerBoundPacket,
    pub cancel_reason: Option<String>,
}
cancellable!(PacketReceiveEvent);

/// 出站包事件：可取消（包不发送）。
pub struct PacketSendEvent {
    pub packet: ClientBoundPacket,
    pub cancel_reason: Option<String>,
}
cancellable!(PacketSendEvent);

/// Handler 切换事件：可替换/装饰新 handler（对应 PlayerSwitchPacketHandlerEvent）。
pub struct PlayerSwitchPacketHandlerEvent {
    pub player: Option<Arc<dyn Player>>,
    pub new_handler: Option<Box<dyn PacketHandler>>,
}

// ---------------- room ----------------

/// 建房前事件：可修改 `RoomSetting`，可取消（对应 RoomPreCreateEvent）。
pub struct RoomPreCreateEvent {
    pub creator: Arc<dyn Player>,
    pub room_id: String,
    pub setting: RoomSetting,
    pub cancel_reason: Option<String>,
}
cancellable!(RoomPreCreateEvent);

pub struct RoomPostCreateEvent {
    pub room: Arc<dyn Room>,
    pub creator: Arc<dyn Player>,
}

pub struct PlayerPreJoinRoomEvent {
    pub player: Arc<dyn Player>,
    pub room: Arc<dyn Room>,
    pub monitor: bool,
    pub cancel_reason: Option<String>,
}
cancellable!(PlayerPreJoinRoomEvent);

pub struct PlayerPostJoinRoomEvent {
    pub player: Arc<dyn Player>,
    pub room: Arc<dyn Room>,
    pub cancel_reason: Option<String>,
}
cancellable!(PlayerPostJoinRoomEvent);

pub struct PlayerJoinRoomSuccessEvent {
    pub player: Arc<dyn Player>,
    pub room: Arc<dyn Room>,
}

pub struct PlayerLeaveRoomEvent {
    pub player: Arc<dyn Player>,
    pub room: Arc<dyn Room>,
}

pub struct RoomDestroyEvent {
    pub room_id: String,
}

pub struct RoomHostChangeEvent {
    pub room: Arc<dyn Room>,
    pub old_host: i32,
    pub new_host: i32,
}

pub struct RoomStateChangeEvent {
    pub room: Arc<dyn Room>,
    pub new_state: crate::packet::state::GameState,
}

// ---------------- operation ----------------

/// 选谱前事件：可注入 `chart` 跳过远程拉谱，可取消。
pub struct RoomPreSelectChartEvent {
    pub room: Arc<dyn Room>,
    pub player: Arc<dyn Player>,
    pub chart_id: i32,
    pub chart: Option<Arc<ChartInfo>>,
    pub cancel_reason: Option<String>,
}
cancellable!(RoomPreSelectChartEvent);

pub struct RoomPostSelectChartEvent {
    pub room: Arc<dyn Room>,
    pub player: Arc<dyn Player>,
    pub chart_id: i32,
}

/// 聊天事件：可改写消息内容，可取消（对应 RoomChatEvent）。
pub struct RoomChatEvent {
    pub room: Arc<dyn Room>,
    pub player: Arc<dyn Player>,
    pub message: String,
    pub cancel_reason: Option<String>,
}
cancellable!(RoomChatEvent);

pub struct RoomLockChangeEvent {
    pub room: Arc<dyn Room>,
    pub player: Arc<dyn Player>,
    pub locked: bool,
}

pub struct RoomCycleChangeEvent {
    pub room: Arc<dyn Room>,
    pub player: Arc<dyn Player>,
    pub cycle: bool,
}

// ---------------- game ----------------

pub struct GameRequireStartEvent {
    pub room: Arc<dyn Room>,
    pub player: Arc<dyn Player>,
    pub cancel_reason: Option<String>,
}
cancellable!(GameRequireStartEvent);

pub struct GameStartEvent {
    pub room: Arc<dyn Room>,
    pub player: Arc<dyn Player>,
}

pub struct GamePlayingStartEvent {
    pub room: Arc<dyn Room>,
}

pub struct PlayerReadyEvent {
    pub room: Arc<dyn Room>,
    pub player: Arc<dyn Player>,
}

pub struct PlayerCancelReadyEvent {
    pub room: Arc<dyn Room>,
    pub player: Arc<dyn Player>,
}

pub struct PlayerPlayedEvent {
    pub room: Arc<dyn Room>,
    pub player: Arc<dyn Player>,
    pub score: i32,
    pub accuracy: f32,
    pub full_combo: bool,
}

pub struct GameAbortEvent {
    pub room: Arc<dyn Room>,
    pub player: Arc<dyn Player>,
}

/// 对局结束事件：携带成绩表（对应 GameEndEvent 的 records）。
pub struct GameEndEvent {
    pub room: Arc<dyn Room>,
    /// user_id → (score, accuracy, full_combo)。
    pub records: Vec<(i32, i32, f32, bool)>,
}

// ---------------- session ----------------

/// 会话挂起事件：可取消（玩家直接退房不挂起）。
pub struct PlayerSessionSuspendEvent {
    pub player: Arc<dyn Player>,
    pub room: Arc<dyn Room>,
    pub cancel_reason: Option<String>,
}
cancellable!(PlayerSessionSuspendEvent);

pub struct PlayerSessionTimeoutEvent {
    pub player: Arc<dyn Player>,
    pub room: Arc<dyn Room>,
}
