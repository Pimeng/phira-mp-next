//! 对局录制（第 10 节，可选扩展）：.phirarec 文件。
//!
//! JPhiraRec v1 格式：
//!   [8B] 魔数 "PHIRAREC"
//!   [4B LE] fileVersion = 1
//!   [1B] compressionType: 0x00=NONE, 0x01=ZSTD
//!   [载荷]: int id; long time(ms); int chart; String chartName(≤32767);
//!           int user; String userName(≤32767);
//!           List<TouchFrame>; List<JudgeEvent>

use crate::bytes::{self, Decode, Encode};
use crate::packet::data::{JudgeEvent, TouchFrame, MAX_STRING_RECORD};
use ::bytes::{Buf, BytesMut};
use std::io::Write;
use std::path::{Path, PathBuf};

pub const MAGIC: &[u8; 8] = b"PHIRAREC";
pub const FORMAT_VERSION: i32 = 1;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum CompressionType {
    None = 0x00,
    Zstd = 0x01,
}

#[derive(Debug, Clone, PartialEq)]
pub struct PhiraRecord {
    pub id: i32,
    pub time_ms: i64,
    pub chart_id: i32,
    pub chart_name: String,
    pub user_id: i32,
    pub user_name: String,
    pub touch_frames: Vec<TouchFrame>,
    pub judge_events: Vec<JudgeEvent>,
}

impl PhiraRecord {
    fn encode_payload(&self) -> BytesMut {
        let mut buf = BytesMut::new();
        bytes::write_i32(&mut buf, self.id);
        bytes::write_i64(&mut buf, self.time_ms);
        bytes::write_i32(&mut buf, self.chart_id);
        bytes::write_string(&mut buf, &self.chart_name);
        bytes::write_i32(&mut buf, self.user_id);
        bytes::write_string(&mut buf, &self.user_name);
        bytes::write_list(&mut buf, &self.touch_frames);
        bytes::write_list(&mut buf, &self.judge_events);
        buf
    }

    /// 写出 .phirarec 文件。返回文件路径。
    pub fn write_to(&self, dir: &Path, compression: CompressionType) -> std::io::Result<PathBuf> {
        std::fs::create_dir_all(dir)?;
        let payload = self.encode_payload();
        let (ctype, body): (CompressionType, Vec<u8>) = match compression {
            CompressionType::None => (CompressionType::None, payload.to_vec()),
            CompressionType::Zstd => (
                CompressionType::Zstd,
                zstd::bulk::compress(&payload, 3)?,
            ),
        };

        let mut out = BytesMut::new();
        out.extend_from_slice(MAGIC);
        bytes::write_i32(&mut out, FORMAT_VERSION);
        out.extend_from_slice(&[ctype as u8]);
        out.extend_from_slice(&body);

        let filename = format!("record-{}-{}.phirarec", self.user_id, self.id);
        let path = dir.join(filename);
        let mut f = std::fs::File::create(&path)?;
        f.write_all(&out)?;
        Ok(path)
    }

    fn decode_payload(mut buf: impl Buf) -> Result<Self, bytes::CodecError> {
        let id = bytes::read_i32(&mut buf)?;
        let time_ms = bytes::read_i64(&mut buf)?;
        let chart_id = bytes::read_i32(&mut buf)?;
        let chart_name = bytes::read_string(&mut buf, MAX_STRING_RECORD)?;
        let user_id = bytes::read_i32(&mut buf)?;
        let user_name = bytes::read_string(&mut buf, MAX_STRING_RECORD)?;
        let touch_frames = read_list::<TouchFrame>(&mut buf)?;
        let judge_events = read_list::<JudgeEvent>(&mut buf)?;
        Ok(Self { id, time_ms, chart_id, chart_name, user_id, user_name, touch_frames, judge_events })
    }

    /// 解析 .phirarec 字节（魔数/版本/压缩校验 + 载荷解码）。
    pub fn parse(data: &[u8]) -> Result<Self, String> {
        if data.len() < 13 {
            return Err("file too short".into());
        }
        if &data[..8] != MAGIC {
            return Err("bad magic".into());
        }
        let mut head = &data[8..13];
        let version = head.get_i32_le();
        if version != FORMAT_VERSION {
            return Err(format!("unsupported version {version}"));
        }
        let ctype = head.get_u8();
        let body = &data[13..];
        let payload: Vec<u8> = match ctype {
            0x00 => body.to_vec(),
            0x01 => {
                let mut out = Vec::new();
                zstd::stream::Decoder::new(body)
                    .and_then(|mut d| std::io::Read::read_to_end(&mut d, &mut out))
                    .map_err(|e| format!("zstd decompress: {e}"))?;
                out
            }
            _ => return Err(format!("unknown compression {ctype:#x}")),
        };
        Self::decode_payload(&payload[..]).map_err(|e| format!("decode payload: {e}"))
    }

    /// 从文件读取并解析。
    pub fn read_from(path: &Path) -> Result<Self, String> {
        let data = std::fs::read(path).map_err(|e| format!("read file: {e}"))?;
        Self::parse(&data)
    }
}

/// 解码 VarInt 计数列表。
fn read_list<T: Decode>(buf: &mut impl Buf) -> Result<Vec<T>, bytes::CodecError> {
    let count = bytes::read_varint(buf)?;
    if count < 0 {
        return Err(bytes::CodecError::BadStringLength(count));
    }
    let mut out = Vec::with_capacity(count as usize);
    for _ in 0..count {
        out.push(T::decode(buf)?);
    }
    Ok(out)
}

/// played 时构造录制对象（触发条件：触摸/判定非空，6.5/10 节）。
pub fn maybe_build_record(
    record_id: i32,
    user_id: i32,
    user_name: &str,
    chart_id: Option<i32>,
    chart_name: Option<String>,
    touch_frames: Vec<TouchFrame>,
    judge_events: Vec<JudgeEvent>,
) -> Option<PhiraRecord> {
    if touch_frames.is_empty() && judge_events.is_empty() {
        return None;
    }
    let now_ms = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0);
    Some(PhiraRecord {
        id: record_id,
        time_ms: now_ms,
        chart_id: chart_id.unwrap_or(0),
        chart_name: chart_name.unwrap_or_default(),
        user_id,
        user_name: user_name.to_string(),
        touch_frames,
        judge_events,
    })
}

#[allow(dead_code)]
fn _assert_limits() {
    let _ = MAX_STRING_RECORD; // 协议常量使用点
    fn _t(_: &dyn Encode) {}
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::packet::data::{CompactPos, Judgement, TouchPoint};

    fn sample_record() -> PhiraRecord {
        PhiraRecord {
            id: 1001,
            time_ms: 1_700_000_000_000,
            chart_id: 42,
            chart_name: "TestChart 测试".into(),
            user_id: 7,
            user_name: "Tester 玩家".into(),
            touch_frames: vec![
                TouchFrame {
                    time: 1.0,
                    points: vec![TouchPoint { id: 0, pos: CompactPos::from_f32(0.5, 0.25) }],
                },
                TouchFrame { time: 2.5, points: vec![] },
            ],
            judge_events: vec![
                JudgeEvent { time: 1.0, line_id: 0, note_id: 1, judgement: Judgement::Perfect },
                JudgeEvent { time: 2.0, line_id: 3, note_id: -1, judgement: Judgement::HoldGood },
            ],
        }
    }

    fn assert_roundtrip(compression: CompressionType) {
        let rec = sample_record();
        let dir = std::env::temp_dir().join(format!(
            "phira-mp-rec-test-{}-{:?}", std::process::id(), compression
        ));
        let path = rec.write_to(&dir, compression).unwrap();
        let parsed = PhiraRecord::read_from(&path).unwrap();
        assert_eq!(parsed, rec);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn record_roundtrip_none() {
        assert_roundtrip(CompressionType::None);
    }

    #[test]
    fn record_roundtrip_zstd() {
        assert_roundtrip(CompressionType::Zstd);
    }

    #[test]
    fn record_reject_bad_magic_and_version() {
        let rec = sample_record();
        let dir = std::env::temp_dir().join(format!("phira-mp-rec-bad-{}", std::process::id()));
        let path = rec.write_to(&dir, CompressionType::None).unwrap();
        let mut data = std::fs::read(&path).unwrap();

        data[0] = b'X';
        assert!(PhiraRecord::parse(&data).unwrap_err().contains("magic"));

        let mut data = std::fs::read(&path).unwrap();
        data[8] = 99; // version 低字节
        assert!(PhiraRecord::parse(&data).unwrap_err().contains("version"));

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn maybe_build_record_empty_returns_none() {
        assert!(maybe_build_record(1, 1, "u", Some(1), None, vec![], vec![]).is_none());
        assert!(maybe_build_record(
            1, 1, "u", Some(1), None,
            vec![TouchFrame { time: 0.0, points: vec![] }],
            vec![],
        ).is_some());
    }
}
