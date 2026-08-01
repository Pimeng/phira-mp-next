//! ClientBound 包（服务端 → 客户端，3.3 节，20 个）。

use crate::bytes::{self, CodecError, Decode, Encode};
use crate::packet::data::{FullUserProfile, JudgeEvent, RoomInfo, TouchFrame};
use crate::packet::message::Message;
use crate::packet::state::GameState;
use crate::packet::{encode_void_result, DecodeSized, PacketResult};
use ::bytes::{Buf, Bytes, BytesMut};

impl DecodeSized for FullUserProfile {
    fn decode_sized(buf: &mut impl Buf) -> Result<Self, CodecError> {
        crate::bytes::Decode::decode(buf)
    }
}

impl DecodeSized for RoomInfo {
    fn decode_sized(buf: &mut impl Buf) -> Result<Self, CodecError> {
        crate::bytes::Decode::decode(buf)
    }
}

/// Authenticate 成功载荷（3.3 节 0x01）。
#[derive(Debug, Clone)]
pub struct AuthenticateData {
    pub user_profile: FullUserProfile,
    pub room_info: Option<RoomInfo>,
}

impl Encode for AuthenticateData {
    fn encode(&self, buf: &mut BytesMut) {
        self.user_profile.encode(buf);
        bytes::write_bool(buf, self.room_info.is_some());
        if let Some(info) = &self.room_info {
            info.encode(buf);
        }
    }
}

impl DecodeSized for AuthenticateData {
    fn decode_sized(buf: &mut impl Buf) -> Result<Self, CodecError> {
        let user_profile: FullUserProfile = crate::bytes::Decode::decode(buf)?;
        let has_room = bytes::read_bool(buf)?;
        let room_info = if has_room {
            let mut info: RoomInfo = crate::bytes::Decode::decode(buf)?;
            // hasRoomInfo 已在前面读取；RoomInfo 自身不含该字段
            let _ = &mut info;
            Some(info)
        } else {
            None
        };
        Ok(AuthenticateData { user_profile, room_info })
    }
}

/// JoinRoom 成功载荷（3.3 节 0x09）。
#[derive(Debug, Clone)]
pub struct JoinRoomData {
    pub game_state: GameState,
    pub users: Vec<FullUserProfile>,
    pub live: bool,
}

impl Encode for JoinRoomData {
    fn encode(&self, buf: &mut BytesMut) {
        self.game_state.encode(buf);
        bytes::write_varint(buf, self.users.len() as i32);
        for u in &self.users {
            u.encode(buf);
        }
        bytes::write_bool(buf, self.live);
    }
}

impl DecodeSized for JoinRoomData {
    fn decode_sized(buf: &mut impl Buf) -> Result<Self, CodecError> {
        let game_state = <GameState as Decode>::decode(buf)?;
        let count = bytes::read_varint(buf)?;
        let mut users = Vec::with_capacity(count.max(0) as usize);
        for _ in 0..count {
            users.push(crate::bytes::Decode::decode(buf)?);
        }
        let live = bytes::read_bool(buf)?;
        Ok(JoinRoomData { game_state, users, live })
    }
}

impl DecodeSized for GameState {
    fn decode_sized(buf: &mut impl Buf) -> Result<Self, CodecError> {
        crate::bytes::Decode::decode(buf)
    }
}

#[derive(Debug, Clone)]
pub enum ClientBoundPacket {
    /// 0x00（单例）
    Pong,
    /// 0x01
    Authenticate { result: PacketResult<AuthenticateData>, trailer: Option<Bytes> },
    /// 0x02
    Chat { result: PacketResult<()>, trailer: Option<Bytes> },
    /// 0x03
    Touches { from_player_id: i32, frames: Vec<TouchFrame>, trailer: Option<Bytes> },
    /// 0x04
    Judges { from_player_id: i32, judges: Vec<JudgeEvent>, trailer: Option<Bytes> },
    /// 0x05
    Message { message: Message, trailer: Option<Bytes> },
    /// 0x06
    ChangeState { game_state: GameState, trailer: Option<Bytes> },
    /// 0x07
    ChangeHost { is_host: bool, trailer: Option<Bytes> },
    /// 0x08
    CreateRoom { result: PacketResult<()>, trailer: Option<Bytes> },
    /// 0x09
    JoinRoom { result: PacketResult<JoinRoomData>, trailer: Option<Bytes> },
    /// 0x0A
    OnJoinRoom { user_profile: FullUserProfile, trailer: Option<Bytes> },
    /// 0x0B
    LeaveRoom { result: PacketResult<()>, trailer: Option<Bytes> },
    /// 0x0C
    LockRoom { result: PacketResult<()>, trailer: Option<Bytes> },
    /// 0x0D
    CycleRoom { result: PacketResult<()>, trailer: Option<Bytes> },
    /// 0x0E
    SelectChart { result: PacketResult<()>, trailer: Option<Bytes> },
    /// 0x0F
    RequestStart { result: PacketResult<()>, trailer: Option<Bytes> },
    /// 0x10
    Ready { result: PacketResult<()>, trailer: Option<Bytes> },
    /// 0x11
    CancelReady { result: PacketResult<()>, trailer: Option<Bytes> },
    /// 0x12
    Played { result: PacketResult<()>, trailer: Option<Bytes> },
    /// 0x13
    Abort { result: PacketResult<()>, trailer: Option<Bytes> },
}

impl ClientBoundPacket {
    /// 从完整帧解码（协议库双向支持；服务端自身不发此包，供测试/工具用）。
    pub fn decode_frame(frame: &[u8]) -> Result<Self, CodecError> {
        let mut buf = frame;
        let id = bytes::read_u8(&mut buf)?;
        use crate::packet::take_trailer as tr;
        Ok(match id {
            0x00 => ClientBoundPacket::Pong,
            0x01 => ClientBoundPacket::Authenticate {
                result: PacketResult::decode(&mut buf)?,
                trailer: tr(&mut buf),
            },
            0x02 => ClientBoundPacket::Chat {
                result: PacketResult::decode(&mut buf)?,
                trailer: tr(&mut buf),
            },
            0x03 => {
                let from_player_id = bytes::read_i32(&mut buf)?;
                let count = bytes::read_varint(&mut buf)?;
                let mut frames = Vec::with_capacity(count.max(0) as usize);
                for _ in 0..count {
                    frames.push(Decode::decode(&mut buf)?);
                }
                ClientBoundPacket::Touches { from_player_id, frames, trailer: tr(&mut buf) }
            }
            0x04 => {
                let from_player_id = bytes::read_i32(&mut buf)?;
                let count = bytes::read_varint(&mut buf)?;
                let mut judges = Vec::with_capacity(count.max(0) as usize);
                for _ in 0..count {
                    judges.push(Decode::decode(&mut buf)?);
                }
                ClientBoundPacket::Judges { from_player_id, judges, trailer: tr(&mut buf) }
            }
            0x05 => ClientBoundPacket::Message {
                message: Decode::decode(&mut buf)?,
                trailer: tr(&mut buf),
            },
            0x06 => ClientBoundPacket::ChangeState {
                game_state: Decode::decode(&mut buf)?,
                trailer: tr(&mut buf),
            },
            0x07 => ClientBoundPacket::ChangeHost {
                is_host: bytes::read_bool(&mut buf)?,
                trailer: tr(&mut buf),
            },
            0x08 => ClientBoundPacket::CreateRoom {
                result: PacketResult::decode(&mut buf)?,
                trailer: tr(&mut buf),
            },
            0x09 => ClientBoundPacket::JoinRoom {
                result: PacketResult::decode(&mut buf)?,
                trailer: tr(&mut buf),
            },
            0x0A => ClientBoundPacket::OnJoinRoom {
                user_profile: Decode::decode(&mut buf)?,
                trailer: tr(&mut buf),
            },
            0x0B => ClientBoundPacket::LeaveRoom {
                result: PacketResult::decode(&mut buf)?,
                trailer: tr(&mut buf),
            },
            0x0C => ClientBoundPacket::LockRoom {
                result: PacketResult::decode(&mut buf)?,
                trailer: tr(&mut buf),
            },
            0x0D => ClientBoundPacket::CycleRoom {
                result: PacketResult::decode(&mut buf)?,
                trailer: tr(&mut buf),
            },
            0x0E => ClientBoundPacket::SelectChart {
                result: PacketResult::decode(&mut buf)?,
                trailer: tr(&mut buf),
            },
            0x0F => ClientBoundPacket::RequestStart {
                result: PacketResult::decode(&mut buf)?,
                trailer: tr(&mut buf),
            },
            0x10 => ClientBoundPacket::Ready {
                result: PacketResult::decode(&mut buf)?,
                trailer: tr(&mut buf),
            },
            0x11 => ClientBoundPacket::CancelReady {
                result: PacketResult::decode(&mut buf)?,
                trailer: tr(&mut buf),
            },
            0x12 => ClientBoundPacket::Played {
                result: PacketResult::decode(&mut buf)?,
                trailer: tr(&mut buf),
            },
            0x13 => ClientBoundPacket::Abort {
                result: PacketResult::decode(&mut buf)?,
                trailer: tr(&mut buf),
            },
            _ => return Err(CodecError::UnknownId("clientbound packet", id)),
        })
    }

    pub fn id(&self) -> u8 {
        match self {
            ClientBoundPacket::Pong => 0x00,
            ClientBoundPacket::Authenticate { .. } => 0x01,
            ClientBoundPacket::Chat { .. } => 0x02,
            ClientBoundPacket::Touches { .. } => 0x03,
            ClientBoundPacket::Judges { .. } => 0x04,
            ClientBoundPacket::Message { .. } => 0x05,
            ClientBoundPacket::ChangeState { .. } => 0x06,
            ClientBoundPacket::ChangeHost { .. } => 0x07,
            ClientBoundPacket::CreateRoom { .. } => 0x08,
            ClientBoundPacket::JoinRoom { .. } => 0x09,
            ClientBoundPacket::OnJoinRoom { .. } => 0x0A,
            ClientBoundPacket::LeaveRoom { .. } => 0x0B,
            ClientBoundPacket::LockRoom { .. } => 0x0C,
            ClientBoundPacket::CycleRoom { .. } => 0x0D,
            ClientBoundPacket::SelectChart { .. } => 0x0E,
            ClientBoundPacket::RequestStart { .. } => 0x0F,
            ClientBoundPacket::Ready { .. } => 0x10,
            ClientBoundPacket::CancelReady { .. } => 0x11,
            ClientBoundPacket::Played { .. } => 0x12,
            ClientBoundPacket::Abort { .. } => 0x13,
        }
    }

    // ---- 便捷构造 ----

    pub fn pong() -> Self {
        ClientBoundPacket::Pong
    }

    pub fn message(msg: Message) -> Self {
        ClientBoundPacket::Message { message: msg, trailer: None }
    }

    pub fn change_state(game_state: GameState) -> Self {
        ClientBoundPacket::ChangeState { game_state, trailer: None }
    }

    pub fn change_host(is_host: bool) -> Self {
        ClientBoundPacket::ChangeHost { is_host, trailer: None }
    }

    pub fn on_join_room(user_profile: FullUserProfile) -> Self {
        ClientBoundPacket::OnJoinRoom { user_profile, trailer: None }
    }

    pub fn chat_result(result: PacketResult<()>) -> Self {
        ClientBoundPacket::Chat { result, trailer: None }
    }

    pub fn create_room_result(result: PacketResult<()>) -> Self {
        ClientBoundPacket::CreateRoom { result, trailer: None }
    }

    pub fn join_room_result(result: PacketResult<JoinRoomData>) -> Self {
        ClientBoundPacket::JoinRoom { result, trailer: None }
    }

    pub fn leave_room_result(result: PacketResult<()>) -> Self {
        ClientBoundPacket::LeaveRoom { result, trailer: None }
    }

    pub fn lock_room_result(result: PacketResult<()>) -> Self {
        ClientBoundPacket::LockRoom { result, trailer: None }
    }

    pub fn cycle_room_result(result: PacketResult<()>) -> Self {
        ClientBoundPacket::CycleRoom { result, trailer: None }
    }

    pub fn select_chart_result(result: PacketResult<()>) -> Self {
        ClientBoundPacket::SelectChart { result, trailer: None }
    }

    pub fn request_start_result(result: PacketResult<()>) -> Self {
        ClientBoundPacket::RequestStart { result, trailer: None }
    }

    pub fn ready_result(result: PacketResult<()>) -> Self {
        ClientBoundPacket::Ready { result, trailer: None }
    }

    pub fn cancel_ready_result(result: PacketResult<()>) -> Self {
        ClientBoundPacket::CancelReady { result, trailer: None }
    }

    pub fn played_result(result: PacketResult<()>) -> Self {
        ClientBoundPacket::Played { result, trailer: None }
    }

    pub fn abort_result(result: PacketResult<()>) -> Self {
        ClientBoundPacket::Abort { result, trailer: None }
    }
}

impl Encode for ClientBoundPacket {
    fn encode(&self, buf: &mut BytesMut) {
        use crate::packet::serverbound::encode_with_trailer as enc;
        let out: BytesMut = match self {
            ClientBoundPacket::Pong => enc(0x00, |_| {}, &None),
            ClientBoundPacket::Authenticate { result, trailer } => {
                enc(0x01, |b| result.encode(b), trailer)
            }
            ClientBoundPacket::Chat { result, trailer } => enc(0x02, |b| encode_void_result(result, b), trailer),
            ClientBoundPacket::Touches { from_player_id, frames, trailer } => {
                enc(0x03, |b| {
                    bytes::write_i32(b, *from_player_id);
                    bytes::write_list(b, frames);
                }, trailer)
            }
            ClientBoundPacket::Judges { from_player_id, judges, trailer } => {
                enc(0x04, |b| {
                    bytes::write_i32(b, *from_player_id);
                    bytes::write_list(b, judges);
                }, trailer)
            }
            ClientBoundPacket::Message { message, trailer } => {
                enc(0x05, |b| message.encode(b), trailer)
            }
            ClientBoundPacket::ChangeState { game_state, trailer } => {
                enc(0x06, |b| game_state.encode(b), trailer)
            }
            ClientBoundPacket::ChangeHost { is_host, trailer } => {
                enc(0x07, |b| bytes::write_bool(b, *is_host), trailer)
            }
            ClientBoundPacket::CreateRoom { result, trailer } => {
                enc(0x08, |b| encode_void_result(result, b), trailer)
            }
            ClientBoundPacket::JoinRoom { result, trailer } => {
                enc(0x09, |b| result.encode(b), trailer)
            }
            ClientBoundPacket::OnJoinRoom { user_profile, trailer } => {
                enc(0x0A, |b| user_profile.encode(b), trailer)
            }
            ClientBoundPacket::LeaveRoom { result, trailer } => {
                enc(0x0B, |b| encode_void_result(result, b), trailer)
            }
            ClientBoundPacket::LockRoom { result, trailer } => {
                enc(0x0C, |b| encode_void_result(result, b), trailer)
            }
            ClientBoundPacket::CycleRoom { result, trailer } => {
                enc(0x0D, |b| encode_void_result(result, b), trailer)
            }
            ClientBoundPacket::SelectChart { result, trailer } => {
                enc(0x0E, |b| encode_void_result(result, b), trailer)
            }
            ClientBoundPacket::RequestStart { result, trailer } => {
                enc(0x0F, |b| encode_void_result(result, b), trailer)
            }
            ClientBoundPacket::Ready { result, trailer } => {
                enc(0x10, |b| encode_void_result(result, b), trailer)
            }
            ClientBoundPacket::CancelReady { result, trailer } => {
                enc(0x11, |b| encode_void_result(result, b), trailer)
            }
            ClientBoundPacket::Played { result, trailer } => {
                enc(0x12, |b| encode_void_result(result, b), trailer)
            }
            ClientBoundPacket::Abort { result, trailer } => {
                enc(0x13, |b| encode_void_result(result, b), trailer)
            }
        };
        buf.extend_from_slice(&out);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn packet_result_codec() {
        let mut buf = BytesMut::new();
        encode_void_result(&PacketResult::<()>::ok(), &mut buf);
        assert_eq!(&buf[..], &[0x01]);

        let mut buf = BytesMut::new();
        encode_void_result(&PacketResult::<()>::failed("房间已满"), &mut buf);
        let mut slice = buf.freeze();
        match PacketResult::<()>::decode(&mut slice).unwrap() {
            PacketResult::Failed(msg) => assert_eq!(msg, "房间已满"),
            _ => panic!(),
        }
    }

    #[test]
    fn authenticate_packet_golden() {
        // success + profile(1, "A", false) + hasRoomInfo=false
        let p = ClientBoundPacket::Authenticate {
            result: PacketResult::Success(AuthenticateData {
                user_profile: FullUserProfile { user_id: 1, user_name: "A".into(), monitor: false },
                room_info: None,
            }),
            trailer: None,
        };
        let mut buf = BytesMut::new();
        p.encode(&mut buf);
        // id=0x01, success=0x01, userId=1(4B LE), "A"(varint1 + 'A'), monitor=0, hasRoom=0
        let expect = [0x01, 0x01, 1, 0, 0, 0, 1, b'A', 0, 0];
        assert_eq!(&buf[..], &expect);
    }

    #[test]
    fn singleton_pong_single_byte() {
        let mut buf = BytesMut::new();
        ClientBoundPacket::pong().encode(&mut buf);
        assert_eq!(&buf[..], &[0x00]);
    }
}
