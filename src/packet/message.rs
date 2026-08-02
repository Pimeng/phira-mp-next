//! Message（3.4 节）：嵌入 ClientBoundMessagePacket 的 16 种消息。
//! 每种消息先写 1 字节 messageId。

use crate::bytes::{self, CodecError, Decode, Encode};
use ::bytes::{Buf, BytesMut};

#[derive(Debug, Clone, PartialEq)]
pub enum Message {
    Chat {
        user: i32,
        content: String,
    },
    CreateRoom {
        user: i32,
    },
    JoinRoom {
        user: i32,
        name: String,
    },
    LeaveRoom {
        user: i32,
        name: String,
    },
    NewHost {
        user: i32,
    },
    SelectChart {
        user: i32,
        name: String,
        id: i32,
    },
    GameStart {
        user: i32,
    },
    Ready {
        user: i32,
    },
    CancelReady {
        user: i32,
    },
    CancelGame {
        user: i32,
    },
    StartPlaying,
    Played {
        user: i32,
        score: i32,
        accuracy: f32,
        full_combo: bool,
    },
    GameEnd,
    Abort {
        user: i32,
    },
    LockRoom {
        lock: bool,
    },
    CycleRoom {
        cycle: bool,
    },
}

impl Message {
    pub fn id(&self) -> u8 {
        match self {
            Message::Chat { .. } => 0x00,
            Message::CreateRoom { .. } => 0x01,
            Message::JoinRoom { .. } => 0x02,
            Message::LeaveRoom { .. } => 0x03,
            Message::NewHost { .. } => 0x04,
            Message::SelectChart { .. } => 0x05,
            Message::GameStart { .. } => 0x06,
            Message::Ready { .. } => 0x07,
            Message::CancelReady { .. } => 0x08,
            Message::CancelGame { .. } => 0x09,
            Message::StartPlaying => 0x0A,
            Message::Played { .. } => 0x0B,
            Message::GameEnd => 0x0C,
            Message::Abort { .. } => 0x0D,
            Message::LockRoom { .. } => 0x0E,
            Message::CycleRoom { .. } => 0x0F,
        }
    }
}

impl Encode for Message {
    fn encode(&self, buf: &mut BytesMut) {
        bytes::write_u8(buf, self.id());
        match self {
            Message::Chat { user, content } => {
                bytes::write_i32(buf, *user);
                bytes::write_string(buf, content);
            }
            Message::CreateRoom { user } => bytes::write_i32(buf, *user),
            Message::JoinRoom { user, name } | Message::LeaveRoom { user, name } => {
                bytes::write_i32(buf, *user);
                bytes::write_string(buf, name);
            }
            Message::NewHost { user } => bytes::write_i32(buf, *user),
            Message::SelectChart { user, name, id } => {
                bytes::write_i32(buf, *user);
                bytes::write_string(buf, name);
                bytes::write_i32(buf, *id);
            }
            Message::GameStart { user } => bytes::write_i32(buf, *user),
            Message::Ready { user } => bytes::write_i32(buf, *user),
            Message::CancelReady { user } => bytes::write_i32(buf, *user),
            Message::CancelGame { user } => bytes::write_i32(buf, *user),
            Message::StartPlaying | Message::GameEnd => {}
            Message::Played {
                user,
                score,
                accuracy,
                full_combo,
            } => {
                bytes::write_i32(buf, *user);
                bytes::write_i32(buf, *score);
                bytes::write_f32(buf, *accuracy);
                bytes::write_bool(buf, *full_combo);
            }
            Message::Abort { user } => bytes::write_i32(buf, *user),
            Message::LockRoom { lock } => bytes::write_bool(buf, *lock),
            Message::CycleRoom { cycle } => bytes::write_bool(buf, *cycle),
        }
    }
}

impl Decode for Message {
    fn decode(buf: &mut impl Buf) -> Result<Self, CodecError> {
        let id = bytes::read_u8(buf)?;
        Ok(match id {
            0x00 => Message::Chat {
                user: bytes::read_i32(buf)?,
                content: bytes::read_string(buf, 131072)?,
            },
            0x01 => Message::CreateRoom {
                user: bytes::read_i32(buf)?,
            },
            0x02 => Message::JoinRoom {
                user: bytes::read_i32(buf)?,
                name: bytes::read_string(buf, 131072)?,
            },
            0x03 => Message::LeaveRoom {
                user: bytes::read_i32(buf)?,
                name: bytes::read_string(buf, 131072)?,
            },
            0x04 => Message::NewHost {
                user: bytes::read_i32(buf)?,
            },
            0x05 => Message::SelectChart {
                user: bytes::read_i32(buf)?,
                name: bytes::read_string(buf, 131072)?,
                id: bytes::read_i32(buf)?,
            },
            0x06 => Message::GameStart {
                user: bytes::read_i32(buf)?,
            },
            0x07 => Message::Ready {
                user: bytes::read_i32(buf)?,
            },
            0x08 => Message::CancelReady {
                user: bytes::read_i32(buf)?,
            },
            0x09 => Message::CancelGame {
                user: bytes::read_i32(buf)?,
            },
            0x0A => Message::StartPlaying,
            0x0B => Message::Played {
                user: bytes::read_i32(buf)?,
                score: bytes::read_i32(buf)?,
                accuracy: bytes::read_f32(buf)?,
                full_combo: bytes::read_bool(buf)?,
            },
            0x0C => Message::GameEnd,
            0x0D => Message::Abort {
                user: bytes::read_i32(buf)?,
            },
            0x0E => Message::LockRoom {
                lock: bytes::read_bool(buf)?,
            },
            0x0F => Message::CycleRoom {
                cycle: bytes::read_bool(buf)?,
            },
            _ => return Err(CodecError::UnknownId("message", id)),
        })
    }
}
