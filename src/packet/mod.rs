//! 协议包定义（对应文档第 3 章）。

pub mod clientbound;
pub mod data;
pub mod message;
pub mod serverbound;
pub mod state;

use crate::bytes::{CodecError, Decode, Encode};
use ::bytes::{Buf, BytesMut};

/// 所有包的公共行为：可选 Trailer（前向兼容尾部字节）。
pub trait Packet: Encode {
    /// 单例包（Ping/Pong/StartPlaying/GameEnd 等）不允许 Trailer。
    fn is_singleton(&self) -> bool {
        false
    }
}

/// 从完整帧解码 Trailer：读完字段后的剩余字节原样保留。
pub(crate) fn take_trailer(buf: &mut impl Buf) -> Option<bytes::Bytes> {
    if buf.has_remaining() {
        Some(buf.copy_to_bytes(buf.remaining()))
    } else {
        None
    }
}

pub(crate) fn put_trailer(out: &mut BytesMut, trailer: &Option<bytes::Bytes>) {
    if let Some(t) = trailer {
        out.extend_from_slice(t);
    }
}

/// 解码 VarInt 计数列表。
pub fn read_list<T: Decode>(buf: &mut impl Buf) -> Result<Vec<T>, CodecError> {
    let count = crate::bytes::read_varint(buf)?;
    if count < 0 {
        return Err(CodecError::BadStringLength(count));
    }
    let mut out = Vec::with_capacity(count as usize);
    for _ in 0..count {
        out.push(T::decode(buf)?);
    }
    Ok(out)
}

/// 通用结果包装器（3.5 节）。
#[derive(Debug, Clone)]
pub enum PacketResult<T> {
    Success(T),
    Failed(String),
}

/// 单位载荷标记（`PacketResult<()>` 的 encode 通过 `VoidPayload` 适配）。
pub struct VoidPayload;

impl Encode for VoidPayload {
    fn encode(&self, _buf: &mut BytesMut) {}
}

impl<T: Encode> Encode for PacketResult<T> {
    fn encode(&self, buf: &mut BytesMut) {
        match self {
            PacketResult::Success(v) => {
                crate::bytes::write_bool(buf, true);
                v.encode(buf);
            }
            PacketResult::Failed(msg) => {
                crate::bytes::write_bool(buf, false);
                crate::bytes::write_string(buf, msg);
            }
        }
    }
}

/// `PacketResult<()>` 的编码（void = 无载荷）。
pub fn encode_void_result(result: &PacketResult<()>, buf: &mut BytesMut) {
    match result {
        PacketResult::Success(()) => {
            crate::bytes::write_bool(buf, true);
        }
        PacketResult::Failed(msg) => {
            crate::bytes::write_bool(buf, false);
            crate::bytes::write_string(buf, msg);
        }
    }
}

impl PacketResult<()> {
    pub fn ok() -> Self {
        PacketResult::Success(())
    }
}

impl<T> PacketResult<T> {
    pub fn failed(msg: impl Into<String>) -> Self {
        PacketResult::Failed(msg.into())
    }
}

impl<T: DecodeSized> PacketResult<T> {
    pub fn decode(buf: &mut impl Buf) -> Result<Self, CodecError> {
        let success = crate::bytes::read_bool(buf)?;
        if success {
            Ok(PacketResult::Success(T::decode_sized(buf)?))
        } else {
            Ok(PacketResult::Failed(crate::bytes::read_string(
                buf, 131072,
            )?))
        }
    }
}

/// 与 `Decode` 相同语义，独立 trait 以避免泛型冲突。
pub trait DecodeSized: Sized {
    fn decode_sized(buf: &mut impl Buf) -> Result<Self, CodecError>;
}

impl DecodeSized for () {
    fn decode_sized(_buf: &mut impl Buf) -> Result<Self, CodecError> {
        Ok(())
    }
}
