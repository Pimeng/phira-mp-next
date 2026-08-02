//! ServerBound 包（客户端 → 服务端，3.2 节，16 个）。

use crate::bytes::{self, CodecError, Encode};
use crate::packet::data::{
    JudgeEvent, MAX_STRING_CHAT, MAX_STRING_ROOM_ID, MAX_STRING_TOKEN, TouchFrame,
};
use ::bytes::{Bytes, BytesMut};

#[derive(Debug, Clone)]
pub enum ServerBoundPacket {
    /// 0x00
    Ping,
    /// 0x01 token(≤32)
    Authenticate {
        token: String,
        trailer: Option<Bytes>,
    },
    /// 0x02 message(≤200)
    Chat {
        message: String,
        trailer: Option<Bytes>,
    },
    /// 0x03
    Touches {
        frames: Vec<TouchFrame>,
        trailer: Option<Bytes>,
    },
    /// 0x04
    Judges {
        judges: Vec<JudgeEvent>,
        trailer: Option<Bytes>,
    },
    /// 0x05 roomId(≤20)
    CreateRoom {
        room_id: String,
        trailer: Option<Bytes>,
    },
    /// 0x06
    JoinRoom {
        room_id: String,
        monitor: bool,
        trailer: Option<Bytes>,
    },
    /// 0x07
    LeaveRoom { trailer: Option<Bytes> },
    /// 0x08（注意：服务端忽略值按切换处理）
    LockRoom { lock: bool, trailer: Option<Bytes> },
    /// 0x09（同上）
    CycleRoom { cycle: bool, trailer: Option<Bytes> },
    /// 0x0A
    SelectChart { id: i32, trailer: Option<Bytes> },
    /// 0x0B
    RequestStart { trailer: Option<Bytes> },
    /// 0x0C
    Ready { trailer: Option<Bytes> },
    /// 0x0D
    CancelReady { trailer: Option<Bytes> },
    /// 0x0E
    Played {
        record_id: i32,
        trailer: Option<Bytes>,
    },
    /// 0x0F
    Abort { trailer: Option<Bytes> },
}

impl ServerBoundPacket {
    /// 从完整帧解码（帧 = u8 packetId + 字段体 + Trailer）。
    pub fn decode_frame(frame: &[u8]) -> Result<Self, CodecError> {
        let mut buf = frame;
        let id = bytes::read_u8(&mut buf)?;
        use crate::packet::take_trailer as tr;
        Ok(match id {
            0x00 => {
                // Ping 单例：忽略多余字节（易错点 6）
                ServerBoundPacket::Ping
            }
            0x01 => ServerBoundPacket::Authenticate {
                token: bytes::read_string(&mut buf, MAX_STRING_TOKEN)?,
                trailer: tr(&mut buf),
            },
            0x02 => ServerBoundPacket::Chat {
                message: bytes::read_string(&mut buf, MAX_STRING_CHAT)?,
                trailer: tr(&mut buf),
            },
            0x03 => {
                let count = bytes::read_varint(&mut buf)?;
                let mut frames = Vec::with_capacity(count.max(0) as usize);
                for _ in 0..count {
                    frames.push(crate::bytes::Decode::decode(&mut buf)?);
                }
                ServerBoundPacket::Touches {
                    frames,
                    trailer: tr(&mut buf),
                }
            }
            0x04 => {
                let count = bytes::read_varint(&mut buf)?;
                let mut judges = Vec::with_capacity(count.max(0) as usize);
                for _ in 0..count {
                    judges.push(crate::bytes::Decode::decode(&mut buf)?);
                }
                ServerBoundPacket::Judges {
                    judges,
                    trailer: tr(&mut buf),
                }
            }
            0x05 => ServerBoundPacket::CreateRoom {
                room_id: bytes::read_string(&mut buf, MAX_STRING_ROOM_ID)?,
                trailer: tr(&mut buf),
            },
            0x06 => ServerBoundPacket::JoinRoom {
                room_id: bytes::read_string(&mut buf, MAX_STRING_ROOM_ID)?,
                monitor: bytes::read_bool(&mut buf)?,
                trailer: tr(&mut buf),
            },
            0x07 => ServerBoundPacket::LeaveRoom {
                trailer: tr(&mut buf),
            },
            0x08 => ServerBoundPacket::LockRoom {
                lock: bytes::read_bool(&mut buf)?,
                trailer: tr(&mut buf),
            },
            0x09 => ServerBoundPacket::CycleRoom {
                cycle: bytes::read_bool(&mut buf)?,
                trailer: tr(&mut buf),
            },
            0x0A => ServerBoundPacket::SelectChart {
                id: bytes::read_i32(&mut buf)?,
                trailer: tr(&mut buf),
            },
            0x0B => ServerBoundPacket::RequestStart {
                trailer: tr(&mut buf),
            },
            0x0C => ServerBoundPacket::Ready {
                trailer: tr(&mut buf),
            },
            0x0D => ServerBoundPacket::CancelReady {
                trailer: tr(&mut buf),
            },
            0x0E => ServerBoundPacket::Played {
                record_id: bytes::read_i32(&mut buf)?,
                trailer: tr(&mut buf),
            },
            0x0F => ServerBoundPacket::Abort {
                trailer: tr(&mut buf),
            },
            _ => return Err(CodecError::UnknownId("serverbound packet", id)),
        })
    }
}

/// 编码辅助：直接写入调用方缓冲（零中间拷贝）。
fn encode_with_trailer(
    out: &mut BytesMut,
    id: u8,
    body: impl FnOnce(&mut BytesMut),
    trailer: &Option<Bytes>,
) {
    bytes::write_u8(out, id);
    body(out);
    crate::packet::put_trailer(out, trailer);
}

impl Encode for ServerBoundPacket {
    fn encode(&self, buf: &mut BytesMut) {
        match self {
            ServerBoundPacket::Ping => encode_with_trailer(buf, 0x00, |_| {}, &None),
            ServerBoundPacket::Authenticate { token, trailer } => {
                encode_with_trailer(buf, 0x01, |b| bytes::write_string(b, token), trailer)
            }
            ServerBoundPacket::Chat { message, trailer } => {
                encode_with_trailer(buf, 0x02, |b| bytes::write_string(b, message), trailer)
            }
            ServerBoundPacket::Touches { frames, trailer } => {
                encode_with_trailer(buf, 0x03, |b| bytes::write_list(b, frames), trailer)
            }
            ServerBoundPacket::Judges { judges, trailer } => {
                encode_with_trailer(buf, 0x04, |b| bytes::write_list(b, judges), trailer)
            }
            ServerBoundPacket::CreateRoom { room_id, trailer } => {
                encode_with_trailer(buf, 0x05, |b| bytes::write_string(b, room_id), trailer)
            }
            ServerBoundPacket::JoinRoom {
                room_id,
                monitor,
                trailer,
            } => encode_with_trailer(
                buf,
                0x06,
                |b| {
                    bytes::write_string(b, room_id);
                    bytes::write_bool(b, *monitor);
                },
                trailer,
            ),
            ServerBoundPacket::LeaveRoom { trailer } => {
                encode_with_trailer(buf, 0x07, |_| {}, trailer)
            }
            ServerBoundPacket::LockRoom { lock, trailer } => {
                encode_with_trailer(buf, 0x08, |b| bytes::write_bool(b, *lock), trailer)
            }
            ServerBoundPacket::CycleRoom { cycle, trailer } => {
                encode_with_trailer(buf, 0x09, |b| bytes::write_bool(b, *cycle), trailer)
            }
            ServerBoundPacket::SelectChart { id, trailer } => {
                encode_with_trailer(buf, 0x0A, |b| bytes::write_i32(b, *id), trailer)
            }
            ServerBoundPacket::RequestStart { trailer } => {
                encode_with_trailer(buf, 0x0B, |_| {}, trailer)
            }
            ServerBoundPacket::Ready { trailer } => encode_with_trailer(buf, 0x0C, |_| {}, trailer),
            ServerBoundPacket::CancelReady { trailer } => {
                encode_with_trailer(buf, 0x0D, |_| {}, trailer)
            }
            ServerBoundPacket::Played { record_id, trailer } => {
                encode_with_trailer(buf, 0x0E, |b| bytes::write_i32(b, *record_id), trailer)
            }
            ServerBoundPacket::Abort { trailer } => encode_with_trailer(buf, 0x0F, |_| {}, trailer),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn roundtrip(p: &ServerBoundPacket) -> ServerBoundPacket {
        let mut buf = BytesMut::new();
        p.encode(&mut buf);
        ServerBoundPacket::decode_frame(&buf).unwrap()
    }

    #[test]
    fn roundtrip_basic() {
        assert!(matches!(
            roundtrip(&ServerBoundPacket::Ping),
            ServerBoundPacket::Ping
        ));
        match roundtrip(&ServerBoundPacket::Authenticate {
            token: "tok".into(),
            trailer: None,
        }) {
            ServerBoundPacket::Authenticate { token, .. } => assert_eq!(token, "tok"),
            _ => panic!(),
        }
        match roundtrip(&ServerBoundPacket::JoinRoom {
            room_id: "R1".into(),
            monitor: true,
            trailer: None,
        }) {
            ServerBoundPacket::JoinRoom {
                room_id, monitor, ..
            } => {
                assert_eq!(room_id, "R1");
                assert!(monitor);
            }
            _ => panic!(),
        }
    }

    #[test]
    fn roundtrip_monitor_data() {
        use crate::packet::data::{CompactPos, JudgeEvent, Judgement, TouchFrame, TouchPoint};
        let touches = ServerBoundPacket::Touches {
            frames: vec![TouchFrame {
                time: 1.5,
                points: vec![TouchPoint {
                    id: -3,
                    pos: CompactPos::from_f32(0.25, -0.5),
                }],
            }],
            trailer: None,
        };
        match roundtrip(&touches) {
            ServerBoundPacket::Touches { frames, .. } => {
                assert_eq!(frames.len(), 1);
                assert_eq!(frames[0].time, 1.5);
                assert_eq!(frames[0].points[0].id, -3);
                let p = frames[0].points[0].pos;
                assert!((p.x_f32() - 0.25).abs() < 1e-3);
                assert!((p.y_f32() + 0.5).abs() < 1e-3);
            }
            _ => panic!(),
        }
        let judges = ServerBoundPacket::Judges {
            judges: vec![JudgeEvent {
                time: 2.0,
                line_id: 1,
                note_id: 42,
                judgement: Judgement::HoldGood,
            }],
            trailer: None,
        };
        match roundtrip(&judges) {
            ServerBoundPacket::Judges { judges, .. } => {
                assert_eq!(judges[0].judgement, Judgement::HoldGood);
            }
            _ => panic!(),
        }
    }

    #[test]
    fn unknown_id_fails() {
        assert!(ServerBoundPacket::decode_frame(&[0x7F]).is_err());
    }
}
