//! phira-mp: Phira 多人联机服务端
//!
//! 模块划分对应 Java 原项目职责：
//! - `bytes`    —— 字节级编解码
//! - `frame`    —— 帧协议（VarInt 长度前缀）
//! - `float16`  —— IEEE-754 半精度
//! - `packet`   —— 协议包定义（零拷贝预编码广播帧）
//! - `network`  —— 网络装配与 PacketHandler 链（Authenticate→Play→Room）
//! - `phira`    —— Phira API 客户端
//! - `player`   —— Player trait（无连接）/ LocalPlayer / PlayerRegistry / 数据源 provider
//! - `room`     —— Room trait / LocalRoom / 状态机 / 操作层 / RoomRegistry
//! - `session`  —— 会话挂起/恢复（掉线不掉房）
//! - `i18n`     —— 国际化（外置语言目录可覆盖）
//! - `log`      —— 多语言日志系统（LOG_* FTL 键，按语言回退链渲染）
//! - `events`   —— 扩展事件定义（对应 Java main.event 包）
//! - `eventbus` —— 异步事件总线（可取消/可改写事件）
//! - `ban`      —— 封禁管理（全局封禁 + 房间封禁）
//! - `command`  —— 控制台命令（事件驱动）
//! - `http`     —— HTTP 查询 API（/api/rooms 等）
//! - `server`   —— 装配与生命周期（纯容器 + 扩展点）
//! - `record`   —— 对局录制

pub mod ban;
pub mod bytes;
pub mod command;
pub mod config;
pub mod eventbus;
pub mod events;
pub mod float16;
pub mod frame;
pub mod http;
pub mod i18n;
pub mod log;
pub mod network;
pub mod packet;
pub mod phira;
pub mod player;
pub mod record;
pub mod room;
pub mod server;
pub mod session;
