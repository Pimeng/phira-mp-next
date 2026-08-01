//! phira-mp: Phira 多人联机服务端
//!
//! 模块划分对应《服务端架构分析》文档：
//! - `bytes`    —— 字节级编解码（2.5 节）
//! - `frame`    —— 帧协议（2.3 节）
//! - `float16`  —— IEEE-754 半精度（2.5 节）
//! - `packet`   —— 协议包定义（第 3 章）
//! - `network`  —— 网络装配与 Handler 链（2、5.4、7.5 节）
//! - `phira`    —— Phira API 客户端（第 4 章）
//! - `player`   —— 玩家与全局注册表（5.2、7.4 节）
//! - `room`     —— 房间与状态机（第 6、7 章）
//! - `session`  —— 会话挂起/恢复（5.3 节）
//! - `i18n`     —— 国际化（第 9 章）
//! - `eventbus` —— 事件总线（第 8 章简化版）
//! - `command`  —— 控制台命令
//! - `server`   —— 服务端装配与生命周期（第 1 章）
//! - `record`   —— 对局录制（第 10 章）

pub mod bytes;
pub mod command;
pub mod eventbus;
pub mod float16;
pub mod frame;
pub mod i18n;
pub mod network;
pub mod packet;
pub mod phira;
pub mod player;
pub mod record;
pub mod room;
pub mod server;
pub mod session;
