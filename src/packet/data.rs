//! 协议数据结构（3.5 / 3.6 节）。

use crate::bytes::{self, CodecError, Decode, Encode};
use crate::float16;
use crate::packet::state::GameState;
use ::bytes::{Buf, BufMut, BytesMut};

/// 字符串默认上限。
pub const MAX_STRING_DEFAULT: i32 = 131072;
pub const MAX_STRING_TOKEN: i32 = 32;
pub const MAX_STRING_ROOM_ID: i32 = 20;
pub const MAX_STRING_CHAT: i32 = 200;
pub const MAX_STRING_RECORD: i32 = 32767;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UserProfile {
    pub user_id: i32,
    pub user_name: String,
}

impl Encode for UserProfile {
    fn encode(&self, buf: &mut BytesMut) {
        bytes::write_i32(buf, self.user_id);
        bytes::write_string(buf, &self.user_name);
    }
}

impl Decode for UserProfile {
    fn decode(buf: &mut impl Buf) -> Result<Self, CodecError> {
        Ok(Self {
            user_id: bytes::read_i32(buf)?,
            user_name: bytes::read_string(buf, MAX_STRING_DEFAULT)?,
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FullUserProfile {
    pub user_id: i32,
    pub user_name: String,
    pub monitor: bool,
}

impl Encode for FullUserProfile {
    fn encode(&self, buf: &mut BytesMut) {
        bytes::write_i32(buf, self.user_id);
        bytes::write_string(buf, &self.user_name);
        bytes::write_bool(buf, self.monitor);
    }
}

impl Decode for FullUserProfile {
    fn decode(buf: &mut impl Buf) -> Result<Self, CodecError> {
        Ok(Self {
            user_id: bytes::read_i32(buf)?,
            user_name: bytes::read_string(buf, MAX_STRING_DEFAULT)?,
            monitor: bytes::read_bool(buf)?,
        })
    }
}

/// 房间信息快照（认证响应内嵌 / JoinRoom 快照）。
/// 注意成员列表是 `[int userId + FullUserProfile]` 的冗余对（3.5 节，易错点 2）。
#[derive(Debug, Clone)]
pub struct RoomInfo {
    pub room_id: String,
    pub state: GameState,
    pub live: bool,
    pub locked: bool,
    pub cycle: bool,
    /// viewer 视角
    pub is_host: bool,
    /// viewer 视角；WaitForReady 状态下恒 true
    pub is_ready: bool,
    pub users: Vec<FullUserProfile>,
}

impl Encode for RoomInfo {
    fn encode(&self, buf: &mut BytesMut) {
        bytes::write_string(buf, &self.room_id);
        self.state.encode(buf);
        bytes::write_bool(buf, self.live);
        bytes::write_bool(buf, self.locked);
        bytes::write_bool(buf, self.cycle);
        bytes::write_bool(buf, self.is_host);
        bytes::write_bool(buf, self.is_ready);
        bytes::write_varint(buf, self.users.len() as i32);
        for u in &self.users {
            bytes::write_i32(buf, u.user_id); // 冗余 userId
            u.encode(buf);
        }
    }
}

impl Decode for RoomInfo {
    fn decode(buf: &mut impl Buf) -> Result<Self, CodecError> {
        let room_id = bytes::read_string(buf, MAX_STRING_ROOM_ID)?;
        let state = GameState::decode(buf)?;
        let live = bytes::read_bool(buf)?;
        let locked = bytes::read_bool(buf)?;
        let cycle = bytes::read_bool(buf)?;
        let is_host = bytes::read_bool(buf)?;
        let is_ready = bytes::read_bool(buf)?;
        let count = bytes::read_varint(buf)?;
        let mut users = Vec::with_capacity(count.max(0) as usize);
        for _ in 0..count {
            let _redundant_user_id = bytes::read_i32(buf)?; // 丢弃
            users.push(FullUserProfile::decode(buf)?);
        }
        Ok(Self {
            room_id,
            state,
            live,
            locked,
            cycle,
            is_host,
            is_ready,
            users,
        })
    }
}

// ---------- Monitor 数据（3.6 节） ----------

/// 触摸点坐标（半精度！不要用 f32 代替，会破坏字节兼容）。
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CompactPos {
    pub x: u16, // float16 位模式
    pub y: u16,
}

impl CompactPos {
    pub fn from_f32(x: f32, y: f32) -> Self {
        Self {
            x: float16::float_to_half(x),
            y: float16::float_to_half(y),
        }
    }

    pub fn x_f32(&self) -> f32 {
        float16::half_to_float(self.x)
    }

    pub fn y_f32(&self) -> f32 {
        float16::half_to_float(self.y)
    }
}

impl Encode for CompactPos {
    fn encode(&self, buf: &mut BytesMut) {
        buf.put_u16_le(self.x);
        buf.put_u16_le(self.y);
    }
}

impl Decode for CompactPos {
    fn decode(buf: &mut impl Buf) -> Result<Self, CodecError> {
        if buf.remaining() < 4 {
            return Err(CodecError::Other("CompactPos: unexpected end".into()));
        }
        Ok(Self {
            x: buf.get_u16_le(),
            y: buf.get_u16_le(),
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TouchPoint {
    pub id: i8,
    pub pos: CompactPos,
}

impl Encode for TouchPoint {
    fn encode(&self, buf: &mut BytesMut) {
        bytes::write_i8(buf, self.id);
        self.pos.encode(buf);
    }
}

impl Decode for TouchPoint {
    fn decode(buf: &mut impl Buf) -> Result<Self, CodecError> {
        Ok(Self {
            id: bytes::read_i8(buf)?,
            pos: CompactPos::decode(buf)?,
        })
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct TouchFrame {
    pub time: f32,
    pub points: Vec<TouchPoint>,
}

impl Encode for TouchFrame {
    fn encode(&self, buf: &mut BytesMut) {
        bytes::write_f32(buf, self.time);
        bytes::write_list(buf, &self.points);
    }
}

impl Decode for TouchFrame {
    fn decode(buf: &mut impl Buf) -> Result<Self, CodecError> {
        let time = bytes::read_f32(buf)?;
        let count = bytes::read_varint(buf)?;
        let mut points = Vec::with_capacity(count.max(0) as usize);
        for _ in 0..count {
            points.push(TouchPoint::decode(buf)?);
        }
        Ok(Self { time, points })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum Judgement {
    Perfect = 0x00,
    Good = 0x01,
    Bad = 0x02,
    Miss = 0x03,
    HoldPerfect = 0x04,
    HoldGood = 0x05,
}

impl Judgement {
    pub fn from_id(id: u8) -> Result<Self, CodecError> {
        Ok(match id {
            0x00 => Judgement::Perfect,
            0x01 => Judgement::Good,
            0x02 => Judgement::Bad,
            0x03 => Judgement::Miss,
            0x04 => Judgement::HoldPerfect,
            0x05 => Judgement::HoldGood,
            _ => return Err(CodecError::UnknownId("judgement", id)),
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct JudgeEvent {
    pub time: f32,
    pub line_id: i32,
    pub note_id: i32,
    pub judgement: Judgement,
}

impl Encode for JudgeEvent {
    fn encode(&self, buf: &mut BytesMut) {
        bytes::write_f32(buf, self.time);
        bytes::write_i32(buf, self.line_id);
        bytes::write_i32(buf, self.note_id);
        buf.put_u8(self.judgement as u8);
    }
}

impl Decode for JudgeEvent {
    fn decode(buf: &mut impl Buf) -> Result<Self, CodecError> {
        Ok(Self {
            time: bytes::read_f32(buf)?,
            line_id: bytes::read_i32(buf)?,
            note_id: bytes::read_i32(buf)?,
            judgement: Judgement::from_id(bytes::read_u8(buf)?)?,
        })
    }
}
