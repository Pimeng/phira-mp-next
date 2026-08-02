//! ClientBound 包（服务端 → 客户端，3.3 节，20 个）。
//!
//! 广播零拷贝：[`encode_packet`] 把包编码为「已带 VarInt 帧头的完整字节」，
//! 包成 `Arc<Bytes>` 后可零克隆地发给任意多个连接（writer 任务直接写 socket）。

use crate::bytes::{self, CodecError, Decode, Encode};
use crate::packet::data::{FullUserProfile, JudgeEvent, RoomInfo, TouchFrame};
use crate::packet::message::Message;
use crate::packet::state::GameState;
use crate::packet::{encode_void_result, DecodeSized, PacketResult};
use ::bytes::{Buf, Bytes, BytesMut};
use std::sync::Arc;

/// 预编码的完整出站帧（含 VarInt 长度前缀）。广播共享，零拷贝。
pub type SharedFrame = Arc<Bytes>;

/// 编码为完整帧字节（id + 字段 + trailer，再前置 VarInt 长度）。
pub fn encode_packet(packet: &ClientBoundPacket) -> Bytes {
    let mut body = BytesMut::new();
    packet.encode(&mut body);
    crate::frame::encode_frame(&body).into()
}

/// 编码并包成可广播的共享帧。
pub fn encode_shared(packet: &ClientBoundPacket) -> SharedFrame {
    Arc::new(encode_packet(packet))
}

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

/// 认证失败专用包（result=Failed(reason)）。供 handler 统一构造。
pub fn authenticate_failed(reason: impl Into<String>) -> ClientBoundPacket {
    ClientBoundPacket::Authenticate {
        result: PacketResult::Failed(reason.into()),
        trailer: None,
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
            Some(crate::bytes::Decode::decode(buf)?)
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

/// 编码包体（id + 字段 + trailer），直接写入调用方缓冲（无中间拷贝）。
fn encode_body(packet: &ClientBoundPacket, buf: &mut BytesMut) {
    use crate::packet::put_trailer;
    let id = packet.id();
    bytes::write_u8(buf, id);
    match packet {
        ClientBoundPacket::Pong => {}
        ClientBoundPacket::Authenticate { result, .. } => result.encode(buf),
        ClientBoundPacket::Chat { result, .. }
        | ClientBoundPacket::CreateRoom { result, .. }
        | ClientBoundPacket::LeaveRoom { result, .. }
        | ClientBoundPacket::LockRoom { result, .. }
        | ClientBoundPacket::CycleRoom { result, .. }
        | ClientBoundPacket::SelectChart { result, .. }
        | ClientBoundPacket::RequestStart { result, .. }
        | ClientBoundPacket::Ready { result, .. }
        | ClientBoundPacket::CancelReady { result, .. }
        | ClientBoundPacket::Played { result, .. }
        | ClientBoundPacket::Abort { result, .. } => encode_void_result(result, buf),
        ClientBoundPacket::Touches { from_player_id, frames, .. } => {
            bytes::write_i32(buf, *from_player_id);
            bytes::write_list(buf, frames);
        }
        ClientBoundPacket::Judges { from_player_id, judges, .. } => {
            bytes::write_i32(buf, *from_player_id);
            bytes::write_list(buf, judges);
        }
        ClientBoundPacket::Message { message, .. } => message.encode(buf),
        ClientBoundPacket::ChangeState { game_state, .. } => game_state.encode(buf),
        ClientBoundPacket::ChangeHost { is_host, .. } => bytes::write_bool(buf, *is_host),
        ClientBoundPacket::JoinRoom { result, .. } => result.encode(buf),
        ClientBoundPacket::OnJoinRoom { user_profile, .. } => user_profile.encode(buf),
    }
    put_trailer(buf, packet.trailer());
}

impl ClientBoundPacket {
    fn trailer(&self) -> &Option<Bytes> {
        match self {
            ClientBoundPacket::Pong => &None,
            ClientBoundPacket::Authenticate { trailer, .. }
            | ClientBoundPacket::Chat { trailer, .. }
            | ClientBoundPacket::Touches { trailer, .. }
            | ClientBoundPacket::Judges { trailer, .. }
            | ClientBoundPacket::Message { trailer, .. }
            | ClientBoundPacket::ChangeState { trailer, .. }
            | ClientBoundPacket::ChangeHost { trailer, .. }
            | ClientBoundPacket::CreateRoom { trailer, .. }
            | ClientBoundPacket::JoinRoom { trailer, .. }
            | ClientBoundPacket::OnJoinRoom { trailer, .. }
            | ClientBoundPacket::LeaveRoom { trailer, .. }
            | ClientBoundPacket::LockRoom { trailer, .. }
            | ClientBoundPacket::CycleRoom { trailer, .. }
            | ClientBoundPacket::SelectChart { trailer, .. }
            | ClientBoundPacket::RequestStart { trailer, .. }
            | ClientBoundPacket::Ready { trailer, .. }
            | ClientBoundPacket::CancelReady { trailer, .. }
            | ClientBoundPacket::Played { trailer, .. }
            | ClientBoundPacket::Abort { trailer, .. } => trailer,
        }
    }
}

impl Encode for ClientBoundPacket {
    fn encode(&self, buf: &mut BytesMut) {
        encode_body(self, buf);
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

    #[test]
    fn encode_packet_includes_frame_header() {
        let bytes = encode_packet(&ClientBoundPacket::pong());
        // frame = varint(1) + [0x00]
        assert_eq!(&bytes[..], &[0x01, 0x00]);
    }

    /// 剥掉帧头后 roundtrip（供全量包对称性测试）。
    fn roundtrip(p: &ClientBoundPacket) -> ClientBoundPacket {
        let mut buf = BytesMut::new();
        p.encode(&mut buf);
        ClientBoundPacket::decode_frame(&buf).unwrap()
    }

    fn void_results() -> [PacketResult<()>; 2] {
        [PacketResult::ok(), PacketResult::failed("x")]
    }

    #[test]
    fn roundtrip_all_result_packets() {
        for r in void_results() {
            for p in [
                ClientBoundPacket::chat_result(r.clone()),
                ClientBoundPacket::create_room_result(r.clone()),
                ClientBoundPacket::leave_room_result(r.clone()),
                ClientBoundPacket::lock_room_result(r.clone()),
                ClientBoundPacket::cycle_room_result(r.clone()),
                ClientBoundPacket::select_chart_result(r.clone()),
                ClientBoundPacket::request_start_result(r.clone()),
                ClientBoundPacket::ready_result(r.clone()),
                ClientBoundPacket::cancel_ready_result(r.clone()),
                ClientBoundPacket::played_result(r.clone()),
                ClientBoundPacket::abort_result(r.clone()),
            ] {
                let back = roundtrip(&p);
                assert_eq!(p.id(), back.id());
            }
        }
    }

    #[test]
    fn roundtrip_message_all_variants() {
        use crate::packet::message::Message;
        let msgs = [
            Message::Chat { user: 1, content: "hi".into() },
            Message::CreateRoom { user: 1 },
            Message::JoinRoom { user: 2, name: "B".into() },
            Message::LeaveRoom { user: 3, name: "C".into() },
            Message::LockRoom { lock: true },
            Message::CycleRoom { cycle: false },
            Message::SelectChart { user: 1, name: "S".into(), id: 42 },
            Message::GameStart { user: 1 },
            Message::Ready { user: 2 },
            Message::CancelReady { user: 2 },
            Message::CancelGame { user: 2 },
            Message::Played { user: 1, score: 999, accuracy: 0.99, full_combo: true },
            Message::Abort { user: 1 },
            Message::GameEnd,
            Message::NewHost { user: 2 },
            Message::StartPlaying,
        ];
        for m in msgs {
            let expected_id = m.id();
            let p = ClientBoundPacket::message(m);
            match roundtrip(&p) {
                ClientBoundPacket::Message { message, .. } => assert_eq!(message.id(), expected_id),
                other => panic!("expected Message, got {other:?}"),
            }
        }
    }

    #[test]
    fn roundtrip_change_state_all() {
        use crate::packet::state::GameState;
        for gs in [
            GameState::SelectChart { chart_id: None },
            GameState::SelectChart { chart_id: Some(42) },
            GameState::WaitForReady,
            GameState::Playing,
        ] {
            match roundtrip(&ClientBoundPacket::change_state(gs)) {
                ClientBoundPacket::ChangeState { .. } => {}
                other => panic!("expected ChangeState, got {other:?}"),
            }
        }
    }

    #[test]
    fn roundtrip_change_host_and_on_join() {
        for h in [true, false] {
            match roundtrip(&ClientBoundPacket::change_host(h)) {
                ClientBoundPacket::ChangeHost { is_host, .. } => assert_eq!(is_host, h),
                other => panic!("expected ChangeHost, got {other:?}"),
            }
        }
        let profile = crate::packet::data::FullUserProfile { user_id: 7, user_name: "X".into(), monitor: true };
        match roundtrip(&ClientBoundPacket::on_join_room(profile)) {
            ClientBoundPacket::OnJoinRoom { user_profile, .. } => {
                assert_eq!(user_profile.user_id, 7);
                assert!(user_profile.monitor);
            }
            other => panic!("expected OnJoinRoom, got {other:?}"),
        }
    }

    #[test]
    fn roundtrip_authenticate_with_room_info() {
        use crate::packet::data::{FullUserProfile, RoomInfo};
        use crate::packet::state::GameState;
        let p = ClientBoundPacket::Authenticate {
            result: PacketResult::Success(AuthenticateData {
                user_profile: FullUserProfile { user_id: 9, user_name: "Z".into(), monitor: false },
                room_info: Some(RoomInfo {
                    room_id: "R".into(),
                    state: GameState::Playing,
                    live: true,
                    locked: false,
                    cycle: true,
                    is_host: true,
                    is_ready: false,
                    users: vec![FullUserProfile { user_id: 9, user_name: "Z".into(), monitor: false }],
                }),
            }),
            trailer: None,
        };
        match roundtrip(&p) {
            ClientBoundPacket::Authenticate { result: PacketResult::Success(d), .. } => {
                assert_eq!(d.user_profile.user_id, 9);
                let info = d.room_info.expect("room_info should survive");
                assert_eq!(info.room_id, "R");
                assert!(info.cycle);
                assert!(info.is_host);
                assert_eq!(info.users.len(), 1);
            }
            other => panic!("expected Authenticate Success, got {other:?}"),
        }
    }

    #[test]
    fn roundtrip_join_room_data() {
        use crate::packet::data::FullUserProfile;
        use crate::packet::state::GameState;
        let data = JoinRoomData {
            game_state: GameState::WaitForReady,
            users: vec![
                FullUserProfile { user_id: 1, user_name: "A".into(), monitor: false },
                FullUserProfile { user_id: 2, user_name: "M".into(), monitor: true },
            ],
            live: true,
        };
        let p = ClientBoundPacket::join_room_result(PacketResult::Success(data));
        match roundtrip(&p) {
            ClientBoundPacket::JoinRoom { result: PacketResult::Success(d), .. } => {
                assert!(matches!(d.game_state, GameState::WaitForReady));
                assert_eq!(d.users.len(), 2);
                assert!(d.users[1].monitor);
                assert!(d.live);
            }
            other => panic!("expected JoinRoom Success, got {other:?}"),
        }
    }

    #[test]
    fn roundtrip_touches_judges_forward() {
        use crate::packet::data::{Judgement, JudgeEvent, TouchFrame};
        let t = ClientBoundPacket::Touches {
            from_player_id: 5,
            frames: vec![TouchFrame { time: 1.0, points: vec![] }],
            trailer: None,
        };
        match roundtrip(&t) {
            ClientBoundPacket::Touches { from_player_id, frames, .. } => {
                assert_eq!(from_player_id, 5);
                assert_eq!(frames.len(), 1);
            }
            other => panic!("expected Touches, got {other:?}"),
        }
        let j = ClientBoundPacket::Judges {
            from_player_id: 6,
            judges: vec![JudgeEvent { time: 2.0, line_id: 0, note_id: 1, judgement: Judgement::Perfect }],
            trailer: None,
        };
        match roundtrip(&j) {
            ClientBoundPacket::Judges { from_player_id, .. } => assert_eq!(from_player_id, 6),
            other => panic!("expected Judges, got {other:?}"),
        }
    }
}
