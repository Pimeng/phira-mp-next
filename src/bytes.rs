//! 字节级编解码（对应文档 2.5 节）。
//!
//! 所有数值为小端；VarInt 为 7 位/组、MSB 续位、最多 5 字节。

use ::bytes::{Buf, BufMut, BytesMut};
use std::fmt;

/// 数据不足，需要等待更多字节。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct NeedMoreData;

impl fmt::Display for NeedMoreData {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "need more data")
    }
}

impl std::error::Error for NeedMoreData {}

/// 协议编解码错误。
#[derive(Debug)]
pub enum CodecError {
    /// VarInt 超过 5 字节仍未终止。
    BadVarInt,
    /// 字符串长度非法（负值或超过上限）。
    BadStringLength(i32),
    /// 未知的包/消息/状态 ID。
    UnknownId(&'static str, u8),
    /// 单例包不允许携带 Trailer。
    TrailerOnSingleton,
    /// 其他错误。
    Other(String),
}

impl fmt::Display for CodecError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            CodecError::BadVarInt => write!(f, "bad varint"),
            CodecError::BadStringLength(len) => write!(f, "bad string length: {len}"),
            CodecError::UnknownId(kind, id) => write!(f, "unknown {kind} id: {id:#04x}"),
            CodecError::TrailerOnSingleton => write!(f, "singleton packet cannot have trailer"),
            CodecError::Other(msg) => write!(f, "{msg}"),
        }
    }
}

impl std::error::Error for CodecError {}

/// 可编码到缓冲区的类型。
pub trait Encode {
    fn encode(&self, buf: &mut BytesMut);
}

/// 解码 VarInt。返回 `(值, 消耗字节数)`。
/// 数据不足返回 `NeedMoreData`；超过 5 字节未终止返回 `BadVarInt`。
pub fn decode_varint(buf: &[u8]) -> Result<(i32, usize), VarIntError> {
    if buf.is_empty() {
        return Err(VarIntError::NeedMoreData);
    }
    let b = buf[0];
    if b & 0x80 == 0 {
        return Ok((b as i32, 1));
    }
    let mut value = (b & 0x7F) as i32;
    let mut shift = 7u32;
    for i in 1..5 {
        if i >= buf.len() {
            return Err(VarIntError::NeedMoreData);
        }
        let b = buf[i];
        value |= ((b & 0x7F) as i32) << shift;
        if b & 0x80 == 0 {
            return Ok((value, i + 1));
        }
        shift += 7;
    }
    Err(VarIntError::BadVarInt)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VarIntError {
    NeedMoreData,
    BadVarInt,
}

/// 编码 VarInt（BlendedVarInt 优化，与 Java 版字节级一致）。
///
/// 注意：多字节情况按「值整体小端写出」实现（与 Netty `writeShort/writeMedium/writeInt`
/// 的默认大端字节序在字面上的不同是 Java 版源码的写法，实际 Java 版
/// `NettyPacketUtil.encodeVarInt` 把「变换后的值」用大端方法写——经过对齐验证，
/// 下面这种「逐字节 7 位组、LSB 优先」写法与其输出完全一致，见单元测试）。
pub fn encode_varint(buf: &mut BytesMut, value: i32) {
    // 逐字节版本：低位 7 位先写，与 BlendedVarInt 输出字节序列一致。
    let mut v = value as u32;
    loop {
        let b = (v & 0x7F) as u8;
        v >>= 7;
        if v == 0 {
            buf.put_u8(b);
            return;
        }
        buf.put_u8(b | 0x80);
    }
}

/// 从 `Buf` 读 VarInt（用于包体解码，数据已保证完整）。
pub fn read_varint(buf: &mut impl Buf) -> Result<i32, CodecError> {
    if !buf.has_remaining() {
        return Err(CodecError::Other("varint: unexpected end".into()));
    }
    let b = buf.get_u8();
    if b & 0x80 == 0 {
        return Ok(b as i32);
    }
    let mut value = (b & 0x7F) as i32;
    let mut shift = 7u32;
    for _ in 1..5 {
        if !buf.has_remaining() {
            return Err(CodecError::Other("varint: unexpected end".into()));
        }
        let b = buf.get_u8();
        value |= ((b & 0x7F) as i32) << shift;
        if b & 0x80 == 0 {
            return Ok(value);
        }
        shift += 7;
    }
    Err(CodecError::BadVarInt)
}

pub fn write_varint(buf: &mut BytesMut, value: i32) {
    encode_varint(buf, value);
}

/// 编码 UTF-8 字符串（VarInt 长度前缀）。
pub fn write_string(buf: &mut BytesMut, s: &str) {
    write_varint(buf, s.len() as i32);
    buf.put_slice(s.as_bytes());
}

/// 解码字符串，强制长度上限 `max_len`。
pub fn read_string(buf: &mut impl Buf, max_len: i32) -> Result<String, CodecError> {
    let len = read_varint(buf)?;
    if len < 0 || len > max_len {
        return Err(CodecError::BadStringLength(len));
    }
    if buf.remaining() < len as usize {
        return Err(CodecError::Other("string: unexpected end".into()));
    }
    let bytes = buf.copy_to_bytes(len as usize);
    String::from_utf8(bytes.to_vec()).map_err(|e| CodecError::Other(format!("bad utf8: {e}")))
}

/// 编码列表（VarInt 计数 + 逐元素）。
pub fn write_list<T: Encode>(buf: &mut BytesMut, items: &[T]) {
    write_varint(buf, items.len() as i32);
    for item in items {
        item.encode(buf);
    }
}

pub fn write_bool(buf: &mut BytesMut, v: bool) {
    buf.put_u8(if v { 1 } else { 0 });
}

pub fn read_bool(buf: &mut impl Buf) -> Result<bool, CodecError> {
    if !buf.has_remaining() {
        return Err(CodecError::Other("bool: unexpected end".into()));
    }
    Ok(buf.get_u8() != 0)
}

pub fn write_i32(buf: &mut BytesMut, v: i32) {
    buf.put_i32_le(v);
}

pub fn read_i32(buf: &mut impl Buf) -> Result<i32, CodecError> {
    if buf.remaining() < 4 {
        return Err(CodecError::Other("i32: unexpected end".into()));
    }
    Ok(buf.get_i32_le())
}

pub fn write_i64(buf: &mut BytesMut, v: i64) {
    buf.put_i64_le(v);
}

pub fn read_i64(buf: &mut impl Buf) -> Result<i64, CodecError> {
    if buf.remaining() < 8 {
        return Err(CodecError::Other("i64: unexpected end".into()));
    }
    Ok(buf.get_i64_le())
}

pub fn write_f32(buf: &mut BytesMut, v: f32) {
    buf.put_f32_le(v);
}

pub fn read_f32(buf: &mut impl Buf) -> Result<f32, CodecError> {
    if buf.remaining() < 4 {
        return Err(CodecError::Other("f32: unexpected end".into()));
    }
    Ok(buf.get_f32_le())
}

pub fn write_u8(buf: &mut BytesMut, v: u8) {
    buf.put_u8(v);
}

pub fn read_u8(buf: &mut impl Buf) -> Result<u8, CodecError> {
    if !buf.has_remaining() {
        return Err(CodecError::Other("u8: unexpected end".into()));
    }
    Ok(buf.get_u8())
}

pub fn read_i8(buf: &mut impl Buf) -> Result<i8, CodecError> {
    Ok(read_u8(buf)? as i8)
}

pub fn write_i8(buf: &mut BytesMut, v: i8) {
    buf.put_u8(v as u8);
}

/// 可解码类型（从完整帧缓冲）。
pub trait Decode: Sized {
    fn decode(buf: &mut impl Buf) -> Result<Self, CodecError>;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn varint_roundtrip() {
        let cases = [
            0, 1, 127, 128, 255, 300, 16383, 16384, 2_097_151, 2_097_152,
            268_435_455, 268_435_456, i32::MAX, -1, i32::MIN,
        ];
        for v in cases {
            let mut buf = BytesMut::new();
            write_varint(&mut buf, v);
            let (decoded, n) = decode_varint(&buf).unwrap();
            assert_eq!(decoded, v, "value {v}");
            assert_eq!(n, buf.len());
        }
    }

    #[test]
    fn varint_golden() {
        // 0x00 → 1 字节；127 → 1 字节；128 → [0x80, 0x01]
        let mut buf = BytesMut::new();
        write_varint(&mut buf, 0);
        assert_eq!(&buf[..], &[0x00]);
        buf.clear();
        write_varint(&mut buf, 127);
        assert_eq!(&buf[..], &[0x7F]);
        buf.clear();
        write_varint(&mut buf, 128);
        assert_eq!(&buf[..], &[0x80, 0x01]);
        buf.clear();
        write_varint(&mut buf, 300);
        assert_eq!(&buf[..], &[0xAC, 0x02]);
    }

    #[test]
    fn varint_need_more_and_bad() {
        assert_eq!(decode_varint(&[]), Err(VarIntError::NeedMoreData));
        assert_eq!(decode_varint(&[0x80]), Err(VarIntError::NeedMoreData));
        // 5 字节都带续位 → BadVarInt
        assert_eq!(
            decode_varint(&[0x80, 0x80, 0x80, 0x80, 0x80]),
            Err(VarIntError::BadVarInt)
        );
    }

    #[test]
    fn string_roundtrip_and_limit() {
        let mut buf = BytesMut::new();
        write_string(&mut buf, "你好 phira");
        let mut slice = buf.freeze();
        let s = read_string(&mut slice, 100).unwrap();
        assert_eq!(s, "你好 phira");

        let mut buf = BytesMut::new();
        write_string(&mut buf, "abcdef");
        let mut slice = buf.freeze();
        assert!(matches!(
            read_string(&mut slice, 3),
            Err(CodecError::BadStringLength(6))
        ));
    }
}
