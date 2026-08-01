//! GameState（3.7 节）：枚举注册表 + 首字节 ID。

use crate::bytes::{self, CodecError, Decode, Encode};
use ::bytes::{Buf, BytesMut};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GameState {
    SelectChart { chart_id: Option<i32> },
    WaitForReady,
    Playing,
}

impl GameState {
    pub const ID_SELECT_CHART: u8 = 0x00;
    pub const ID_WAIT_FOR_READY: u8 = 0x01;
    pub const ID_PLAYING: u8 = 0x02;
}

impl Encode for GameState {
    fn encode(&self, buf: &mut BytesMut) {
        match self {
            GameState::SelectChart { chart_id } => {
                bytes::write_u8(buf, Self::ID_SELECT_CHART);
                match chart_id {
                    Some(id) => {
                        bytes::write_bool(buf, true);
                        bytes::write_i32(buf, *id);
                    }
                    None => bytes::write_bool(buf, false),
                }
            }
            GameState::WaitForReady => bytes::write_u8(buf, Self::ID_WAIT_FOR_READY),
            GameState::Playing => bytes::write_u8(buf, Self::ID_PLAYING),
        }
    }
}

impl Decode for GameState {
    fn decode(buf: &mut impl Buf) -> Result<Self, CodecError> {
        let id = bytes::read_u8(buf)?;
        Ok(match id {
            Self::ID_SELECT_CHART => {
                let has = bytes::read_bool(buf)?;
                let chart_id = if has { Some(bytes::read_i32(buf)?) } else { None };
                GameState::SelectChart { chart_id }
            }
            Self::ID_WAIT_FOR_READY => GameState::WaitForReady,
            Self::ID_PLAYING => GameState::Playing,
            _ => return Err(CodecError::UnknownId("gamestate", id)),
        })
    }
}
