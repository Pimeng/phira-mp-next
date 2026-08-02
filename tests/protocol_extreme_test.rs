//! 协议编解码极限测试（纯逻辑，无需网络）。
//!
//! 覆盖：
//! - VarInt：边界值 / 多字节 / 超长未终止 / 截断 / 负数
//! - 字符串：长度边界 / 超长 / 负数长度 / 截断 / 非法 UTF-8
//! - 帧协议：前导 NUL / 空帧 / 负长度 / 坏 VarInt / 超大帧分片 / 多帧合流
//! - float16：±0 / 次正规 / Inf / NaN 静默化 / 舍入边界 / round-trip
//! - Message / GameState / PacketResult 全量 round-trip
//! - ServerBoundPacket 16 种 / ClientBoundPacket 20 种 round-trip（含 Trailer）
//! - 未知 ID / 长度超限 / 单例忽略多余字节等错误与宽容路径

use bytes::{BufMut, Bytes, BytesMut};
use phira_mp::bytes::{CodecError, Decode, VarIntError};
use phira_mp::float16::{float_to_half, half_to_float};
use phira_mp::frame::{FrameDecoder, FrameError, encode_frame};
use phira_mp::packet::PacketResult;
use phira_mp::packet::clientbound::{
    AuthenticateData, ClientBoundPacket, JoinRoomData, encode_packet,
};
use phira_mp::packet::data::{
    CompactPos, FullUserProfile, JudgeEvent, Judgement, RoomInfo, TouchFrame, TouchPoint,
    UserProfile,
};
use phira_mp::packet::message::Message;
use phira_mp::packet::serverbound::ServerBoundPacket;
use phira_mp::packet::state::GameState;

// ============================== VarInt ==============================

/// 确定性 LCG 伪随机（避免依赖外部 rand crate）。
struct Lcg(u64);
impl Lcg {
    fn next(&mut self) -> u64 {
        self.0 = self
            .0
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        self.0
    }
}

fn varint_bytes(v: i32) -> Vec<u8> {
    let mut buf = BytesMut::new();
    phira_mp::bytes::write_varint(&mut buf, v);
    buf.to_vec()
}

#[test]
fn varint_single_byte_boundaries() {
    assert_eq!(varint_bytes(0), [0x00]);
    assert_eq!(varint_bytes(1), [0x01]);
    assert_eq!(varint_bytes(127), [0x7F]);
}

#[test]
fn varint_multi_byte_boundaries() {
    assert_eq!(varint_bytes(128), [0x80, 0x01]);
    assert_eq!(varint_bytes(16383), [0xFF, 0x7F]);
    assert_eq!(varint_bytes(16384), [0x80, 0x80, 0x01]);
    assert_eq!(varint_bytes(2097151), [0xFF, 0xFF, 0x7F]);
    assert_eq!(varint_bytes(2097152), [0x80, 0x80, 0x80, 0x01]);
    // i32::MAX = 0x7FFF_FFFF
    assert_eq!(varint_bytes(i32::MAX), [0xFF, 0xFF, 0xFF, 0xFF, 0x07]);
    // 负数：i32 语义 5 字节
    assert_eq!(varint_bytes(-1), [0xFF, 0xFF, 0xFF, 0xFF, 0x0F]);
    assert_eq!(varint_bytes(i32::MIN), [0x80, 0x80, 0x80, 0x80, 0x08]);
}

#[test]
fn varint_roundtrip_random_values() {
    let mut rng = Lcg(0xDEAD_BEEF);
    let mut buf = BytesMut::new();
    for _ in 0..100_000 {
        let v = rng.next() as i32;
        buf.clear();
        phira_mp::bytes::write_varint(&mut buf, v);
        let (decoded, consumed) = phira_mp::bytes::decode_varint(&buf).unwrap();
        assert_eq!(decoded, v, "roundtrip {v}");
        assert_eq!(consumed, buf.len());
    }
    // 饱和边界
    for v in [i32::MIN, -1, 0, 1, i32::MAX] {
        buf.clear();
        phira_mp::bytes::write_varint(&mut buf, v);
        let (decoded, _) = phira_mp::bytes::decode_varint(&buf).unwrap();
        assert_eq!(decoded, v);
    }
}

#[test]
fn varint_too_long_rejected() {
    // 5 字节都带续位且第 6 字节仍未终止 → BadVarInt
    let err = phira_mp::bytes::decode_varint(&[0x80, 0x80, 0x80, 0x80, 0x80]).unwrap_err();
    assert_eq!(err, VarIntError::BadVarInt);
    let err = phira_mp::bytes::decode_varint(&[0x80, 0x80, 0x80, 0x80, 0x80, 0x01]).unwrap_err();
    assert_eq!(err, VarIntError::BadVarInt);
}

#[test]
fn varint_truncated_waits() {
    assert_eq!(
        phira_mp::bytes::decode_varint(&[]).unwrap_err(),
        VarIntError::NeedMoreData
    );
    assert_eq!(
        phira_mp::bytes::decode_varint(&[0x80]).unwrap_err(),
        VarIntError::NeedMoreData
    );
    assert_eq!(
        phira_mp::bytes::decode_varint(&[0x80, 0x80]).unwrap_err(),
        VarIntError::NeedMoreData
    );
    assert_eq!(
        phira_mp::bytes::decode_varint(&[0xFF, 0xFF, 0x7F]).unwrap(),
        (2097151, 3)
    );
}

#[test]
fn read_varint_from_empty_buffer_fails() {
    let mut empty: &[u8] = &[];
    assert!(phira_mp::bytes::read_varint(&mut empty).is_err());
    let mut partial: &[u8] = &[0x80];
    assert!(phira_mp::bytes::read_varint(&mut partial).is_err());
}

// ============================== 字符串 ==============================

fn roundtrip_string(s: &str) {
    let mut buf = BytesMut::new();
    phira_mp::bytes::write_string(&mut buf, s);
    let mut reader: &[u8] = &buf;
    let out = phira_mp::bytes::read_string(&mut reader, 131072).unwrap();
    assert_eq!(out, s);
}

#[test]
fn string_roundtrip_various() {
    roundtrip_string("");
    roundtrip_string("hello");
    roundtrip_string("中文混合 abc 123");
    roundtrip_string(&"x".repeat(131072)); // 恰好上限
}

#[test]
fn string_exceeds_max_len() {
    let mut buf = BytesMut::new();
    phira_mp::bytes::write_string(&mut buf, "toolong");
    let mut reader: &[u8] = &buf;
    let err = phira_mp::bytes::read_string(&mut reader, 5).unwrap_err();
    assert!(matches!(err, CodecError::BadStringLength(7)));
}

#[test]
fn string_negative_len() {
    let mut buf = BytesMut::new();
    phira_mp::bytes::write_varint(&mut buf, -1);
    let mut reader: &[u8] = &buf;
    let err = phira_mp::bytes::read_string(&mut reader, 100).unwrap_err();
    assert!(matches!(err, CodecError::BadStringLength(n) if n < 0));
}

#[test]
fn string_truncated_payload() {
    let mut buf = BytesMut::new();
    phira_mp::bytes::write_varint(&mut buf, 10);
    buf.extend_from_slice(b"abc"); // 只有 3 字节
    let mut reader: &[u8] = &buf;
    assert!(matches!(
        phira_mp::bytes::read_string(&mut reader, 100).unwrap_err(),
        CodecError::Other(_)
    ));
}

#[test]
fn string_invalid_utf8() {
    let mut buf = BytesMut::new();
    phira_mp::bytes::write_varint(&mut buf, 2);
    buf.extend_from_slice(&[0xFF, 0xFE]);
    let mut reader: &[u8] = &buf;
    assert!(matches!(
        phira_mp::bytes::read_string(&mut reader, 100).unwrap_err(),
        CodecError::Other(_)
    ));
}

// ============================== 帧协议 ==============================

fn decoder_feed_all(parts: &[&[u8]]) -> Vec<Bytes> {
    let mut dec = FrameDecoder::new();
    let mut out = Vec::new();
    for p in parts {
        dec.feed(p);
        while let Ok(Some(f)) = dec.next_frame() {
            out.push(f);
        }
    }
    out
}

#[test]
fn frame_all_nul_cleared_and_waits() {
    let mut dec = FrameDecoder::new();
    dec.feed(&[0u8; 16]);
    assert!(dec.next_frame().unwrap().is_none());
    // 后续正常帧仍可解码（NUL 已清空）
    let frame = encode_frame(b"ok");
    dec.feed(&frame);
    assert_eq!(&dec.next_frame().unwrap().unwrap()[..], b"ok");
}

#[test]
fn frame_empty_payload_is_indistinguishable_from_nul() {
    // 空 payload 帧 = [0x00]：0x00 既是 varint(0) 也是 NUL 前缀，
    // FrameDecoder 把它当作前导 NUL 清空等待（协议中空帧无意义，单例包仍含 id 字节）
    let frame = encode_frame(b"");
    assert_eq!(frame[0], 0x00);
    let mut dec = FrameDecoder::new();
    dec.feed(&frame);
    assert!(
        dec.next_frame().unwrap().is_none(),
        "空帧与 NUL 前缀不可区分 → 等待"
    );
}

#[test]
fn frame_negative_length() {
    let mut dec = FrameDecoder::new();
    // varint -1 = FF FF FF FF 0F
    dec.feed(&[0xFF, 0xFF, 0xFF, 0xFF, 0x0F, 0x00]);
    assert!(matches!(dec.next_frame(), Err(FrameError::NegativeLength)));
}

#[test]
fn frame_bad_varint() {
    let mut dec = FrameDecoder::new();
    dec.feed(&[0x80, 0x80, 0x80, 0x80, 0x80, 0x01]);
    assert!(matches!(dec.next_frame(), Err(FrameError::BadVarInt)));
}

#[test]
fn frame_large_payload_fragmented() {
    // 100KB payload，逐块喂入（跨 read buffer 模拟）
    let payload = vec![0xABu8; 100_000];
    let frame = encode_frame(&payload);
    let mut dec = FrameDecoder::new();
    let chunks: Vec<&[u8]> = frame.chunks(7000).collect();
    for (i, chunk) in chunks.iter().enumerate() {
        dec.feed(chunk);
        if i < chunks.len() - 1 {
            assert!(
                dec.next_frame().unwrap().is_none(),
                "应等待完整帧 (chunk {i})"
            );
        }
    }
    let out = dec.next_frame().unwrap().expect("large frame");
    assert_eq!(out.len(), 100_000);
    assert_eq!(&out[..], &payload[..]);
}

#[test]
fn frame_multiple_frames_one_feed() {
    let f1 = encode_frame(b"aa");
    let f2 = encode_frame(b"bbb");
    let f3 = encode_frame(b"c");
    let mut data = f1.to_vec();
    data.extend_from_slice(&f2);
    data.extend_from_slice(&f3);
    let out = decoder_feed_all(&[&data]);
    assert_eq!(out.len(), 3);
    assert_eq!(&out[0][..], b"aa");
    assert_eq!(&out[1][..], b"bbb");
    assert_eq!(&out[2][..], b"c");
}

#[test]
fn frame_nul_prefix_before_frame() {
    let frame = encode_frame(b"abc");
    let mut data = vec![0u8, 0, 0];
    data.extend_from_slice(&frame);
    let out = decoder_feed_all(&[&data]);
    assert_eq!(&out[0][..], b"abc");
}

#[test]
fn frame_roundtrip_encode_decode() {
    let payload = vec![0x01, 0x02, 0x03, 0x7F, 0xFF];
    let frame = encode_frame(&payload);
    let mut dec = FrameDecoder::new();
    dec.feed(&frame);
    assert_eq!(&dec.next_frame().unwrap().unwrap()[..], &payload[..]);
}

// ============================== float16 ==============================

#[test]
fn half_to_float_golden() {
    assert_eq!(half_to_float(0x0000), 0.0);
    assert_eq!(half_to_float(0x8000), -0.0);
    assert!(half_to_float(0x8000).is_sign_negative());
    assert_eq!(half_to_float(0x3C00), 1.0);
    assert_eq!(half_to_float(0xC000), -2.0);
    assert_eq!(half_to_float(0x7BFF), 65504.0); // 最大有限
    assert_eq!(half_to_float(0xFBFF), -65504.0);
    assert_eq!(half_to_float(0x7C00), f32::INFINITY);
    assert_eq!(half_to_float(0xFC00), f32::NEG_INFINITY);
    // 最小次正规 2^-24
    assert_eq!(half_to_float(0x0001), 2f32.powi(-24));
    assert_eq!(half_to_float(0x8001), -2f32.powi(-24));
}

#[test]
fn half_nan_quiet_and_signals() {
    // 任意 NaN 位模式（含 signaling）→ f32 NaN
    for h in [0x7C01, 0x7E00, 0x7FFF, 0xFC01] {
        let f = half_to_float(h);
        assert!(f.is_nan(), "half {h:#06x} should be NaN");
    }
}

#[test]
fn float_to_half_golden() {
    assert_eq!(float_to_half(0.0), 0x0000);
    assert_eq!(float_to_half(-0.0), 0x8000);
    assert_eq!(float_to_half(1.0), 0x3C00);
    assert_eq!(float_to_half(-2.0), 0xC000);
    assert_eq!(float_to_half(65504.0), 0x7BFF);
    assert_eq!(float_to_half(-65504.0), 0xFBFF);
    // 上溢 → Inf
    assert_eq!(float_to_half(65520.0), 0x7C00);
    assert_eq!(float_to_half(1e10), 0x7C00);
    assert_eq!(float_to_half(-1e10), 0xFC00);
    // 下溢 → ±0
    assert_eq!(float_to_half(1e-10), 0x0000);
    assert_eq!(float_to_half(-1e-10), 0x8000);
    // Inf/NaN
    assert_eq!(float_to_half(f32::INFINITY), 0x7C00);
    assert_eq!(float_to_half(f32::NEG_INFINITY), 0xFC00);
    let nan = float_to_half(f32::NAN);
    assert!(
        nan & 0x7C00 == 0x7C00 && nan & 0x03FF != 0,
        "NaN preserved: {nan:#06x}"
    );
}

#[test]
fn float16_round_trip_half_lossless() {
    // half → float → half 必须无损失
    let mut rng = Lcg(0x1234_5678);
    for _ in 0..100_000 {
        let h = (rng.next() & 0xFFFF) as u16;
        if h & 0x7C00 == 0x7C00 {
            continue; // Inf/NaN 无损失比较意义
        }
        let f = half_to_float(h);
        assert_eq!(float_to_half(f), h, "half {h:#06x}");
    }
}

#[test]
fn float16_round_trip_float_approx() {
    // float → half → float：误差不超过 half 精度（ULP ≤ 2^-10 相对）
    let mut rng = Lcg(0xCAFE_0001);
    for _ in 0..100_000 {
        let bits = rng.next() as u32;
        let f = f32::from_bits(bits);
        if !f.is_finite() || f.abs() > 65504.0 {
            continue;
        }
        let h = float_to_half(f);
        let back = half_to_float(h);
        if back == 0.0 {
            // 下溢到 ±0：原值必须小到无法由 half 表示
            assert!(f.abs() < 2f32.powi(-24) * 1.5, "f={f} 下溢但并非极小");
            continue;
        }
        if f.abs() < 2f32.powi(-14) {
            // 次正规区：绝对误差 ≤ 半个最小次正规 ULP（round-to-nearest）
            assert!(
                (back - f).abs() <= 2f32.powi(-24) * 1.5,
                "f={f} back={back}"
            );
            continue;
        }
        let rel = ((back - f).abs() / f.abs()).max(1e-30);
        assert!(rel < 0.001, "f={f} back={back} rel={rel}");
    }
}

#[test]
fn float16_round_to_nearest_even() {
    // 1.0 + 2^-11 恰在中间 → round-to-even 到 1.0
    assert_eq!(float_to_half(1.0 + 2f32.powi(-11)), 0x3C00);
    // 1.0 + 2^-11 + 2^-20（明确大于中间值）→ 向上到 1.0 + 2^-10
    assert_eq!(float_to_half(1.0 + 2f32.powi(-11) + 2f32.powi(-20)), 0x3C01);
    // 0.5 + 2^-12 中间 → round-to-even 到 0.5（mantissa LSB=0）
    assert_eq!(float_to_half(0.5 + 2f32.powi(-12)), 0x3800);
    // 0.5 + 2^-12 + 2^-21 → 向上到 0.5 + 2^-11
    assert_eq!(float_to_half(0.5 + 2f32.powi(-12) + 2f32.powi(-21)), 0x3801);
}

// ============================== Message ==============================

fn msg_roundtrip(m: &Message) {
    let mut buf = BytesMut::new();
    phira_mp::bytes::Encode::encode(m, &mut buf);
    let mut reader: &[u8] = &buf;
    let out = Message::decode(&mut reader).unwrap();
    assert_eq!(format!("{m:?}"), format!("{out:?}"), "message roundtrip");
}

#[test]
fn message_all_variants_roundtrip() {
    msg_roundtrip(&Message::Chat {
        user: 1,
        content: "hi".into(),
    });
    msg_roundtrip(&Message::CreateRoom { user: 2 });
    msg_roundtrip(&Message::JoinRoom {
        user: 3,
        name: "N".into(),
    });
    msg_roundtrip(&Message::LeaveRoom {
        user: 4,
        name: "N".into(),
    });
    msg_roundtrip(&Message::NewHost { user: 5 });
    msg_roundtrip(&Message::SelectChart {
        user: 6,
        name: "C".into(),
        id: 42,
    });
    msg_roundtrip(&Message::GameStart { user: 7 });
    msg_roundtrip(&Message::Ready { user: 8 });
    msg_roundtrip(&Message::CancelReady { user: 9 });
    msg_roundtrip(&Message::CancelGame { user: 10 });
    msg_roundtrip(&Message::StartPlaying);
    msg_roundtrip(&Message::Played {
        user: 11,
        score: 999_999,
        accuracy: 100.0,
        full_combo: true,
    });
    msg_roundtrip(&Message::Played {
        user: 12,
        score: 0,
        accuracy: 0.0,
        full_combo: false,
    });
    msg_roundtrip(&Message::GameEnd);
    msg_roundtrip(&Message::Abort { user: 13 });
    msg_roundtrip(&Message::LockRoom { lock: true });
    msg_roundtrip(&Message::LockRoom { lock: false });
    msg_roundtrip(&Message::CycleRoom { cycle: true });
}

#[test]
fn message_unknown_id() {
    let mut reader: &[u8] = &[0x7F];
    assert!(matches!(
        Message::decode(&mut reader).unwrap_err(),
        CodecError::UnknownId(_, 0x7F)
    ));
}

#[test]
fn message_ids_stable() {
    // 协议 ID 必须稳定（0x00..=0x0F）
    let ids = [
        Message::Chat {
            user: 0,
            content: String::new(),
        },
        Message::CreateRoom { user: 0 },
        Message::JoinRoom {
            user: 0,
            name: String::new(),
        },
        Message::LeaveRoom {
            user: 0,
            name: String::new(),
        },
        Message::NewHost { user: 0 },
        Message::SelectChart {
            user: 0,
            name: String::new(),
            id: 0,
        },
        Message::GameStart { user: 0 },
        Message::Ready { user: 0 },
        Message::CancelReady { user: 0 },
        Message::CancelGame { user: 0 },
        Message::StartPlaying,
        Message::Played {
            user: 0,
            score: 0,
            accuracy: 0.0,
            full_combo: false,
        },
        Message::GameEnd,
        Message::Abort { user: 0 },
        Message::LockRoom { lock: false },
        Message::CycleRoom { cycle: false },
    ];
    for (i, m) in ids.iter().enumerate() {
        assert_eq!(m.id(), i as u8, "message id {i}");
    }
}

// ============================== GameState / PacketResult ==============================

fn state_roundtrip(s: &GameState) {
    let mut buf = BytesMut::new();
    phira_mp::bytes::Encode::encode(s, &mut buf);
    let mut reader: &[u8] = &buf;
    assert_eq!(*s, GameState::decode(&mut reader).unwrap());
}

#[test]
fn game_state_all_variants() {
    state_roundtrip(&GameState::SelectChart { chart_id: None });
    state_roundtrip(&GameState::SelectChart { chart_id: Some(0) });
    state_roundtrip(&GameState::SelectChart {
        chart_id: Some(i32::MAX),
    });
    state_roundtrip(&GameState::SelectChart {
        chart_id: Some(i32::MIN),
    });
    state_roundtrip(&GameState::WaitForReady);
    state_roundtrip(&GameState::Playing);
}

#[test]
fn game_state_unknown_id() {
    let mut reader: &[u8] = &[0x7F];
    assert!(matches!(
        GameState::decode(&mut reader).unwrap_err(),
        CodecError::UnknownId(_, 0x7F)
    ));
}

#[test]
fn packet_result_roundtrip() {
    use phira_mp::packet::encode_void_result;
    // 成功带载荷（void：无额外字节）
    let mut buf = BytesMut::new();
    encode_void_result(&PacketResult::Success(()), &mut buf);
    assert_eq!(buf.to_vec(), vec![0x01]);
    let mut r: &[u8] = &buf;
    assert!(matches!(
        PacketResult::<()>::decode(&mut r).unwrap(),
        PacketResult::Success(())
    ));

    // 失败消息（含超长消息）
    for msg in vec!["".to_string(), "error".to_string(), "x".repeat(131072)] {
        buf.clear();
        encode_void_result(&PacketResult::Failed(msg.clone()), &mut buf);
        let mut r: &[u8] = &buf;
        assert!(
            matches!(PacketResult::<()>::decode(&mut r).unwrap(), PacketResult::Failed(m) if m == msg)
        );
    }
}

// ============================== ServerBoundPacket ==============================

fn sb_roundtrip(p: &ServerBoundPacket) {
    let mut body = BytesMut::new();
    phira_mp::bytes::Encode::encode(p, &mut body);
    let dec = ServerBoundPacket::decode_frame(&body).unwrap();
    assert_eq!(
        format!("{p:?}"),
        format!("{dec:?}"),
        "serverbound roundtrip"
    );
}

#[test]
fn serverbound_all_packets_roundtrip() {
    let trailer = Some(Bytes::from_static(&[0xDE, 0xAD, 0xBE, 0xEF]));
    let tframe = TouchFrame {
        time: 12.5,
        points: vec![
            TouchPoint {
                id: 0,
                pos: CompactPos::from_f32(0.25, 0.75),
            },
            TouchPoint {
                id: -1,
                pos: CompactPos { x: 0, y: 0 },
            },
        ],
    };
    let judges = vec![
        JudgeEvent {
            time: 1.0,
            line_id: 2,
            note_id: 3,
            judgement: Judgement::Perfect,
        },
        JudgeEvent {
            time: 2.5,
            line_id: -1,
            note_id: 99,
            judgement: Judgement::HoldGood,
        },
    ];
    sb_roundtrip(&ServerBoundPacket::Ping);
    sb_roundtrip(&ServerBoundPacket::Authenticate {
        token: "tok".into(),
        trailer: None,
    });
    sb_roundtrip(&ServerBoundPacket::Authenticate {
        token: "tok".into(),
        trailer: trailer.clone(),
    });
    sb_roundtrip(&ServerBoundPacket::Chat {
        message: "hi".into(),
        trailer: None,
    });
    sb_roundtrip(&ServerBoundPacket::Chat {
        message: "hi".into(),
        trailer: trailer.clone(),
    });
    sb_roundtrip(&ServerBoundPacket::Touches {
        frames: vec![],
        trailer: None,
    });
    sb_roundtrip(&ServerBoundPacket::Touches {
        frames: vec![tframe.clone()],
        trailer: None,
    });
    sb_roundtrip(&ServerBoundPacket::Touches {
        frames: vec![tframe.clone()],
        trailer: trailer.clone(),
    });
    sb_roundtrip(&ServerBoundPacket::Judges {
        judges: vec![],
        trailer: None,
    });
    sb_roundtrip(&ServerBoundPacket::Judges {
        judges: judges.clone(),
        trailer: None,
    });
    sb_roundtrip(&ServerBoundPacket::Judges {
        judges,
        trailer: trailer.clone(),
    });
    sb_roundtrip(&ServerBoundPacket::CreateRoom {
        room_id: "R".into(),
        trailer: None,
    });
    sb_roundtrip(&ServerBoundPacket::CreateRoom {
        room_id: "R".into(),
        trailer: trailer.clone(),
    });
    sb_roundtrip(&ServerBoundPacket::JoinRoom {
        room_id: "R".into(),
        monitor: true,
        trailer: None,
    });
    sb_roundtrip(&ServerBoundPacket::JoinRoom {
        room_id: "R".into(),
        monitor: false,
        trailer: trailer.clone(),
    });
    sb_roundtrip(&ServerBoundPacket::LeaveRoom { trailer: None });
    sb_roundtrip(&ServerBoundPacket::LeaveRoom {
        trailer: trailer.clone(),
    });
    sb_roundtrip(&ServerBoundPacket::LockRoom {
        lock: true,
        trailer: None,
    });
    sb_roundtrip(&ServerBoundPacket::CycleRoom {
        cycle: false,
        trailer: trailer.clone(),
    });
    sb_roundtrip(&ServerBoundPacket::SelectChart {
        id: 42,
        trailer: None,
    });
    sb_roundtrip(&ServerBoundPacket::SelectChart {
        id: -1,
        trailer: trailer.clone(),
    });
    sb_roundtrip(&ServerBoundPacket::RequestStart { trailer: None });
    sb_roundtrip(&ServerBoundPacket::Ready {
        trailer: trailer.clone(),
    });
    sb_roundtrip(&ServerBoundPacket::CancelReady { trailer: None });
    sb_roundtrip(&ServerBoundPacket::Played {
        record_id: 1001,
        trailer: None,
    });
    sb_roundtrip(&ServerBoundPacket::Played {
        record_id: i32::MIN,
        trailer: trailer.clone(),
    });
    sb_roundtrip(&ServerBoundPacket::Abort { trailer: None });
    sb_roundtrip(&ServerBoundPacket::Abort {
        trailer: trailer.clone(),
    });
}

#[test]
fn serverbound_ping_ignores_extra_bytes() {
    // 单例 Ping 忽略多余字节（易错点 6）
    let mut body = BytesMut::new();
    body.put_u8(0x00);
    body.extend_from_slice(&[0xAA, 0xBB, 0xCC]);
    assert!(matches!(
        ServerBoundPacket::decode_frame(&body).unwrap(),
        ServerBoundPacket::Ping
    ));
}

#[test]
fn serverbound_unknown_id() {
    assert!(matches!(
        ServerBoundPacket::decode_frame(&[0x7F]).unwrap_err(),
        CodecError::UnknownId(_, 0x7F)
    ));
    assert!(matches!(
        ServerBoundPacket::decode_frame(&[0x10]).unwrap_err(),
        CodecError::UnknownId(_, 0x10)
    ));
}

#[test]
fn serverbound_oversized_fields_rejected() {
    use phira_mp::bytes::Encode as _;
    // token > 32
    let pkt = ServerBoundPacket::Authenticate {
        token: "x".repeat(33),
        trailer: None,
    };
    let mut body = BytesMut::new();
    pkt.encode(&mut body);
    assert!(matches!(
        ServerBoundPacket::decode_frame(&body).unwrap_err(),
        CodecError::BadStringLength(33)
    ));
    // room_id > 20
    let pkt = ServerBoundPacket::CreateRoom {
        room_id: "y".repeat(21),
        trailer: None,
    };
    let mut body = BytesMut::new();
    pkt.encode(&mut body);
    assert!(matches!(
        ServerBoundPacket::decode_frame(&body).unwrap_err(),
        CodecError::BadStringLength(21)
    ));
    // chat > 200
    let pkt = ServerBoundPacket::Chat {
        message: "z".repeat(201),
        trailer: None,
    };
    let mut body = BytesMut::new();
    pkt.encode(&mut body);
    assert!(matches!(
        ServerBoundPacket::decode_frame(&body).unwrap_err(),
        CodecError::BadStringLength(201)
    ));
}

#[test]
fn serverbound_touches_negative_count_tolerated() {
    // 负计数 → 空列表（0..negative 空 range，宽容）
    let mut body = BytesMut::new();
    body.put_u8(0x03);
    phira_mp::bytes::write_varint(&mut body, -1);
    match ServerBoundPacket::decode_frame(&body).unwrap() {
        ServerBoundPacket::Touches { frames, .. } => assert!(frames.is_empty()),
        other => panic!("expected Touches, got {other:?}"),
    }
}

#[test]
fn serverbound_truncated_body() {
    // 声明了长度但字节不足 → 解码错误
    let mut body = BytesMut::new();
    body.put_u8(0x01); // Authenticate
    phira_mp::bytes::write_varint(&mut body, 10); // 声称 10 字节 token
    body.extend_from_slice(b"abc");
    assert!(ServerBoundPacket::decode_frame(&body).is_err());
}

// ============================== ClientBoundPacket ==============================

fn cb_roundtrip(p: &ClientBoundPacket) {
    // encode_packet 产出带 VarInt 帧头的完整帧；decode_frame 期望剥帧头后的 payload
    let frame = encode_packet(p);
    let mut dec = FrameDecoder::new();
    dec.feed(&frame);
    let payload = dec
        .next_frame()
        .unwrap()
        .unwrap_or_else(|| panic!("no frame produced for {p:?}"));
    let dec = ClientBoundPacket::decode_frame(&payload).unwrap_or_else(|e| {
        panic!(
            "roundtrip failed for {p:?}: {e:?} (frame={:02x?})",
            &frame[..frame.len().min(64)]
        )
    });
    assert_eq!(
        format!("{p:?}"),
        format!("{dec:?}"),
        "clientbound roundtrip"
    );
}

fn sample_user(id: i32) -> FullUserProfile {
    FullUserProfile {
        user_id: id,
        user_name: format!("U{id}"),
        monitor: id % 2 == 0,
    }
}

fn sample_room_info() -> RoomInfo {
    RoomInfo {
        room_id: "R1".into(),
        state: GameState::SelectChart { chart_id: Some(7) },
        live: true,
        locked: false,
        cycle: true,
        is_host: true,
        is_ready: false,
        users: vec![sample_user(1), sample_user(2)],
    }
}

#[test]
fn clientbound_all_packets_roundtrip() {
    let trailer = Some(Bytes::from_static(&[0x01, 0x02, 0x03]));
    let tframe = TouchFrame {
        time: 1.5,
        points: vec![TouchPoint {
            id: 3,
            pos: CompactPos::from_f32(0.1, 0.9),
        }],
    };
    let judges = vec![JudgeEvent {
        time: 2.0,
        line_id: 0,
        note_id: 5,
        judgement: Judgement::Miss,
    }];
    let auth_ok = AuthenticateData {
        user_profile: sample_user(1),
        room_info: Some(sample_room_info()),
    };
    let join_ok = JoinRoomData {
        game_state: GameState::WaitForReady,
        users: vec![sample_user(1), sample_user(2), sample_user(3)],
        live: true,
    };

    cb_roundtrip(&ClientBoundPacket::Pong);
    cb_roundtrip(&ClientBoundPacket::Authenticate {
        result: PacketResult::Success(auth_ok),
        trailer: None,
    });
    cb_roundtrip(&ClientBoundPacket::Authenticate {
        result: PacketResult::Failed("denied".into()),
        trailer: trailer.clone(),
    });
    cb_roundtrip(&ClientBoundPacket::Chat {
        result: PacketResult::ok(),
        trailer: None,
    });
    cb_roundtrip(&ClientBoundPacket::Chat {
        result: PacketResult::failed("err"),
        trailer: trailer.clone(),
    });
    cb_roundtrip(&ClientBoundPacket::Touches {
        from_player_id: 9,
        frames: vec![],
        trailer: None,
    });
    cb_roundtrip(&ClientBoundPacket::Touches {
        from_player_id: 9,
        frames: vec![tframe.clone()],
        trailer: trailer.clone(),
    });
    cb_roundtrip(&ClientBoundPacket::Judges {
        from_player_id: 9,
        judges: vec![],
        trailer: None,
    });
    cb_roundtrip(&ClientBoundPacket::Judges {
        from_player_id: 9,
        judges: judges.clone(),
        trailer: trailer.clone(),
    });
    cb_roundtrip(&ClientBoundPacket::Message {
        message: Message::StartPlaying,
        trailer: None,
    });
    cb_roundtrip(&ClientBoundPacket::Message {
        message: Message::Played {
            user: 1,
            score: 1,
            accuracy: 1.0,
            full_combo: true,
        },
        trailer: trailer.clone(),
    });
    cb_roundtrip(&ClientBoundPacket::ChangeState {
        game_state: GameState::Playing,
        trailer: None,
    });
    cb_roundtrip(&ClientBoundPacket::ChangeState {
        game_state: GameState::SelectChart { chart_id: Some(-1) },
        trailer: trailer.clone(),
    });
    cb_roundtrip(&ClientBoundPacket::ChangeHost {
        is_host: true,
        trailer: None,
    });
    cb_roundtrip(&ClientBoundPacket::ChangeHost {
        is_host: false,
        trailer: trailer.clone(),
    });
    cb_roundtrip(&ClientBoundPacket::CreateRoom {
        result: PacketResult::ok(),
        trailer: None,
    });
    cb_roundtrip(&ClientBoundPacket::JoinRoom {
        result: PacketResult::Success(join_ok),
        trailer: None,
    });
    cb_roundtrip(&ClientBoundPacket::JoinRoom {
        result: PacketResult::failed("full"),
        trailer: trailer.clone(),
    });
    cb_roundtrip(&ClientBoundPacket::OnJoinRoom {
        user_profile: sample_user(4),
        trailer: None,
    });
    cb_roundtrip(&ClientBoundPacket::OnJoinRoom {
        user_profile: sample_user(5),
        trailer: trailer.clone(),
    });
    cb_roundtrip(&ClientBoundPacket::LeaveRoom {
        result: PacketResult::ok(),
        trailer: None,
    });
    cb_roundtrip(&ClientBoundPacket::LockRoom {
        result: PacketResult::ok(),
        trailer: trailer.clone(),
    });
    cb_roundtrip(&ClientBoundPacket::CycleRoom {
        result: PacketResult::failed("no"),
        trailer: None,
    });
    cb_roundtrip(&ClientBoundPacket::SelectChart {
        result: PacketResult::ok(),
        trailer: trailer.clone(),
    });
    cb_roundtrip(&ClientBoundPacket::RequestStart {
        result: PacketResult::failed("x"),
        trailer: None,
    });
    cb_roundtrip(&ClientBoundPacket::Ready {
        result: PacketResult::ok(),
        trailer: trailer.clone(),
    });
    cb_roundtrip(&ClientBoundPacket::CancelReady {
        result: PacketResult::ok(),
        trailer: None,
    });
    cb_roundtrip(&ClientBoundPacket::Played {
        result: PacketResult::ok(),
        trailer: trailer.clone(),
    });
    cb_roundtrip(&ClientBoundPacket::Abort {
        result: PacketResult::ok(),
        trailer: None,
    });
    cb_roundtrip(&ClientBoundPacket::Abort {
        result: PacketResult::failed("no"),
        trailer: trailer.clone(),
    });
}

#[test]
fn clientbound_unknown_id() {
    assert!(matches!(
        ClientBoundPacket::decode_frame(&[0x14]).unwrap_err(),
        CodecError::UnknownId(_, 0x14)
    ));
}

#[test]
fn clientbound_ids_stable() {
    let ids = [
        ClientBoundPacket::Pong,
        ClientBoundPacket::Authenticate {
            result: PacketResult::Success(AuthenticateData {
                user_profile: sample_user(0),
                room_info: None,
            }),
            trailer: None,
        },
        ClientBoundPacket::Chat {
            result: PacketResult::ok(),
            trailer: None,
        },
        ClientBoundPacket::Touches {
            from_player_id: 0,
            frames: vec![],
            trailer: None,
        },
        ClientBoundPacket::Judges {
            from_player_id: 0,
            judges: vec![],
            trailer: None,
        },
        ClientBoundPacket::Message {
            message: Message::StartPlaying,
            trailer: None,
        },
        ClientBoundPacket::ChangeState {
            game_state: GameState::Playing,
            trailer: None,
        },
        ClientBoundPacket::ChangeHost {
            is_host: false,
            trailer: None,
        },
        ClientBoundPacket::CreateRoom {
            result: PacketResult::ok(),
            trailer: None,
        },
        ClientBoundPacket::JoinRoom {
            result: PacketResult::failed(""),
            trailer: None,
        },
        ClientBoundPacket::OnJoinRoom {
            user_profile: sample_user(0),
            trailer: None,
        },
        ClientBoundPacket::LeaveRoom {
            result: PacketResult::ok(),
            trailer: None,
        },
        ClientBoundPacket::LockRoom {
            result: PacketResult::ok(),
            trailer: None,
        },
        ClientBoundPacket::CycleRoom {
            result: PacketResult::ok(),
            trailer: None,
        },
        ClientBoundPacket::SelectChart {
            result: PacketResult::ok(),
            trailer: None,
        },
        ClientBoundPacket::RequestStart {
            result: PacketResult::ok(),
            trailer: None,
        },
        ClientBoundPacket::Ready {
            result: PacketResult::ok(),
            trailer: None,
        },
        ClientBoundPacket::CancelReady {
            result: PacketResult::ok(),
            trailer: None,
        },
        ClientBoundPacket::Played {
            result: PacketResult::ok(),
            trailer: None,
        },
        ClientBoundPacket::Abort {
            result: PacketResult::ok(),
            trailer: None,
        },
    ];
    for (i, p) in ids.iter().enumerate() {
        assert_eq!(p.id(), i as u8, "clientbound id {i}");
    }
}

// ---------------- 数据结构的额外边界 ----------------

#[test]
fn compact_pos_f32_roundtrip() {
    for (x, y) in [
        (0.0, 0.0),
        (0.5, 0.5),
        (-1.0, 1.0),
        (1.0, -1.0),
        (0.123, 0.987),
    ] {
        let p = CompactPos::from_f32(x, y);
        let dx = (p.x_f32() - x).abs();
        let dy = (p.y_f32() - y).abs();
        assert!(
            dx < 0.001 && dy < 0.001,
            "({x},{y}) -> ({}, {})",
            p.x_f32(),
            p.y_f32()
        );
    }
}

#[test]
fn compact_pos_encode_decode() {
    let p = CompactPos::from_f32(0.3, 0.6);
    let mut buf = BytesMut::new();
    phira_mp::bytes::Encode::encode(&p, &mut buf);
    assert_eq!(buf.len(), 4);
    let mut r: &[u8] = &buf;
    assert_eq!(CompactPos::decode(&mut r).unwrap(), p);
    // 截断
    let mut r2: &[u8] = &buf[..2];
    assert!(CompactPos::decode(&mut r2).is_err());
}

#[test]
fn judgement_id_mapping() {
    assert_eq!(Judgement::from_id(0).unwrap(), Judgement::Perfect);
    assert_eq!(Judgement::from_id(5).unwrap(), Judgement::HoldGood);
    assert!(matches!(
        Judgement::from_id(6).unwrap_err(),
        CodecError::UnknownId(_, 6)
    ));
    assert!(matches!(
        Judgement::from_id(0xFF).unwrap_err(),
        CodecError::UnknownId(_, 0xFF)
    ));
}

#[test]
fn user_profile_and_full_roundtrip() {
    let u = UserProfile {
        user_id: -5,
        user_name: "中文名".into(),
    };
    let mut buf = BytesMut::new();
    phira_mp::bytes::Encode::encode(&u, &mut buf);
    let mut r: &[u8] = &buf;
    assert_eq!(UserProfile::decode(&mut r).unwrap(), u);

    let f = FullUserProfile {
        user_id: 5,
        user_name: "M".into(),
        monitor: true,
    };
    let mut buf = BytesMut::new();
    phira_mp::bytes::Encode::encode(&f, &mut buf);
    let mut r: &[u8] = &buf;
    assert_eq!(FullUserProfile::decode(&mut r).unwrap(), f);
}

#[test]
fn room_info_roundtrip_with_redundant_user_ids() {
    let info = sample_room_info();
    let mut buf = BytesMut::new();
    phira_mp::bytes::Encode::encode(&info, &mut buf);
    let mut r: &[u8] = &buf;
    let dec = RoomInfo::decode(&mut r).unwrap();
    assert_eq!(dec.room_id, "R1");
    assert_eq!(dec.state, GameState::SelectChart { chart_id: Some(7) });
    assert_eq!(dec.users.len(), 2);
    assert_eq!(dec.users[0].user_id, 1);
    assert_eq!(dec.users[1].user_name, "U2");
}

#[test]
fn touch_frame_and_judge_event_roundtrip() {
    let tf = TouchFrame {
        time: -1.5,
        points: vec![
            TouchPoint {
                id: 0,
                pos: CompactPos::from_f32(1.0, 1.0),
            },
            TouchPoint {
                id: 127,
                pos: CompactPos {
                    x: 0xFFFF,
                    y: 0x0000,
                },
            },
        ],
    };
    let mut buf = BytesMut::new();
    phira_mp::bytes::Encode::encode(&tf, &mut buf);
    let mut r: &[u8] = &buf;
    assert_eq!(TouchFrame::decode(&mut r).unwrap(), tf);

    let je = JudgeEvent {
        time: 3.25,
        line_id: 7,
        note_id: -8,
        judgement: Judgement::Bad,
    };
    let mut buf = BytesMut::new();
    phira_mp::bytes::Encode::encode(&je, &mut buf);
    let mut r: &[u8] = &buf;
    assert_eq!(JudgeEvent::decode(&mut r).unwrap(), je);
}

#[test]
fn encode_packet_frame_header_is_valid() {
    // encode_packet 产出「VarInt 帧头 + payload」，可经 FrameDecoder 还原
    let pkt = ClientBoundPacket::Pong;
    let frame = encode_packet(&pkt);
    let mut dec = FrameDecoder::new();
    dec.feed(&frame);
    let payload = dec.next_frame().unwrap().unwrap();
    assert!(matches!(
        ClientBoundPacket::decode_frame(&payload).unwrap(),
        ClientBoundPacket::Pong
    ));
}

#[test]
fn zero_copy_shared_frame_identity() {
    // encode_shared 两次编码同一包 → 字节一致；不同包 → 不一致
    use phira_mp::packet::clientbound::encode_shared;
    let a = encode_shared(&ClientBoundPacket::Pong);
    let b = encode_shared(&ClientBoundPacket::Pong);
    let c = encode_shared(&ClientBoundPacket::Chat {
        result: PacketResult::ok(),
        trailer: None,
    });
    assert_eq!(a.as_ref().as_ref(), b.as_ref().as_ref());
    assert_ne!(a.as_ref().as_ref(), c.as_ref().as_ref());
}
