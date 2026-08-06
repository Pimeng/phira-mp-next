//! 网络层（对应文档 2、5.4、7.5 节）。
//!
//! - 握手：1 字节协议版本 0x01，5 秒超时。
//! - 帧：VarInt 长度前缀（见 `frame` 模块）。
//! - 每连接串行处理：读循环按序把包交给当前 `PacketHandler`。
//! - Handler 链：AuthenticateHandler → PlayHandler → RoomHandler（take/put 模式）。

pub mod authenticate_handler;
pub mod connection;
pub mod handler;
pub mod play_handler;
pub mod proxy;
pub mod room_handler;

/// 协议版本（仅 0x01 受支持）。
pub const PROTOCOL_VERSION: u8 = 0x01;

pub use connection::{ConnectionHandle, spawn_connection};
