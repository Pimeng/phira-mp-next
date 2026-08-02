//! 帧协议：VarInt 长度前缀 + payload（对应文档 2.3 节）。
//!
//! 解码行为与 Java `FrameDecoder` 精确一致：
//! 1. 跳过所有前导 0x00 字节（容错）；全是 NUL → 等待更多。
//! 2. VarInt 数据不足 → 等待；超过 5 字节未终止 → BadVarInt。
//! 3. `remaining < L` → 等待完整帧。
//! 4. 产出完整帧。

use crate::bytes::{decode_varint, encode_varint, VarIntError};
use ::bytes::{Bytes, BytesMut};

/// 帧解码错误（导致连接关闭）。
#[derive(Debug)]
pub enum FrameError {
    BadVarInt,
    NegativeLength,
}

impl std::fmt::Display for FrameError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            FrameError::BadVarInt => write!(f, "bad varint"),
            FrameError::NegativeLength => write!(f, "negative frame length"),
        }
    }
}

impl std::error::Error for FrameError {}

/// 增量帧解码器。持续 `feed` 数据并 `next_frame` 取帧。
#[derive(Default)]
pub struct FrameDecoder {
    buf: BytesMut,
}

impl FrameDecoder {
    pub fn new() -> Self {
        Self { buf: BytesMut::new() }
    }

    pub fn feed(&mut self, data: &[u8]) {
        self.buf.extend_from_slice(data);
    }

    /// 尝试取出下一个完整帧。无完整帧返回 `Ok(None)`。
    pub fn next_frame(&mut self) -> Result<Option<Bytes>, FrameError> {
        // 1. 跳过前导 NUL
        let non_nul = self.buf.iter().position(|&b| b != 0);
        match non_nul {
            None => {
                // 全是 NUL（或空）→ 清空等待
                self.buf.clear();
                return Ok(None);
            }
            Some(pos) if pos > 0 => {
                let _ = self.buf.split_to(pos);
            }
            _ => {}
        }

        // 2. 解 VarInt 长度
        let (len, consumed) = match decode_varint(&self.buf) {
            Ok(v) => v,
            Err(VarIntError::NeedMoreData) => return Ok(None),
            Err(VarIntError::BadVarInt) => return Err(FrameError::BadVarInt),
        };
        if len < 0 {
            return Err(FrameError::NegativeLength);
        }
        let len = len as usize;

        // 3. 等待完整帧
        if self.buf.len() - consumed < len {
            return Ok(None);
        }

        // 4. 产出帧
        let _ = self.buf.split_to(consumed);
        let frame = self.buf.split_to(len).freeze();
        Ok(Some(frame))
    }
}

/// 编码帧：前置 VarInt 长度。
pub fn encode_frame(payload: &[u8]) -> BytesMut {
    let mut out = BytesMut::with_capacity(payload.len() + 5);
    encode_varint(&mut out, payload.len() as i32);
    out.extend_from_slice(payload);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn frame_roundtrip() {
        let payload = b"\x01\x02\x03hello";
        let encoded = encode_frame(payload);
        let mut dec = FrameDecoder::new();
        dec.feed(&encoded);
        let frame = dec.next_frame().unwrap().unwrap();
        assert_eq!(&frame[..], payload);
        assert!(dec.next_frame().unwrap().is_none());
    }

    #[test]
    fn frame_skip_nul_prefix() {
        let encoded = encode_frame(b"abc");
        let mut data = vec![0u8, 0, 0];
        data.extend_from_slice(&encoded);
        let mut dec = FrameDecoder::new();
        dec.feed(&data);
        assert_eq!(&dec.next_frame().unwrap().unwrap()[..], b"abc");
    }

    #[test]
    fn frame_partial_feed() {
        let encoded = encode_frame(b"0123456789");
        let mut dec = FrameDecoder::new();
        // 逐字节喂入
        for (i, b) in encoded.iter().enumerate() {
            dec.feed(&[*b]);
            let frame = dec.next_frame().unwrap();
            if i < encoded.len() - 1 {
                assert!(frame.is_none(), "should wait at byte {i}");
            } else {
                assert_eq!(&frame.unwrap()[..], b"0123456789");
            }
        }
    }

    #[test]
    fn frame_multiple_in_one_feed() {
        let f1 = encode_frame(b"aa");
        let f2 = encode_frame(b"bbb");
        let mut data = f1.to_vec();
        data.extend_from_slice(&f2);
        let mut dec = FrameDecoder::new();
        dec.feed(&data);
        assert_eq!(&dec.next_frame().unwrap().unwrap()[..], b"aa");
        assert_eq!(&dec.next_frame().unwrap().unwrap()[..], b"bbb");
        assert!(dec.next_frame().unwrap().is_none());
    }

    #[test]
    fn frame_bad_varint() {
        let mut dec = FrameDecoder::new();
        dec.feed(&[0x80, 0x80, 0x80, 0x80, 0x80, 0x01]);
        assert!(matches!(dec.next_frame(), Err(FrameError::BadVarInt)));
    }

    #[test]
    fn frame_all_nul_waits() {
        let mut dec = FrameDecoder::new();
        dec.feed(&[0, 0, 0]);
        assert!(dec.next_frame().unwrap().is_none());
    }
}
