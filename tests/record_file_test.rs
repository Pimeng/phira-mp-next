//! 对局录制文件测试（.phirarec 格式）。
//!
//! 覆盖：NONE/ZSTD 压缩写读 round-trip、损坏文件各错误路径
//! （太短 / 坏魔数 / 坏版本 / 未知压缩 / 载荷解码错误）、
//! `maybe_build_record` 触发条件。

use phira_mp::packet::data::{CompactPos, JudgeEvent, Judgement, TouchFrame, TouchPoint};
use phira_mp::record::{maybe_build_record, CompressionType, PhiraRecord, FORMAT_VERSION, MAGIC};
use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};

static DIR_COUNTER: AtomicUsize = AtomicUsize::new(0);

fn temp_dir() -> PathBuf {
    let n = DIR_COUNTER.fetch_add(1, Ordering::SeqCst);
    std::env::temp_dir().join(format!(
        "phira-rec-test-{}-{}",
        std::process::id(),
        n
    ))
}

fn sample_record() -> PhiraRecord {
    PhiraRecord {
        id: 1001,
        time_ms: 1_700_000_000_000,
        chart_id: 42,
        chart_name: "Test Chart".into(),
        user_id: 7,
        user_name: "Tester".into(),
        touch_frames: vec![
            TouchFrame {
                time: 1.0,
                points: vec![
                    TouchPoint { id: 0, pos: CompactPos::from_f32(0.25, 0.75) },
                    TouchPoint { id: 1, pos: CompactPos::from_f32(0.5, 0.5) },
                ],
            },
            TouchFrame { time: 2.5, points: vec![] },
        ],
        judge_events: vec![
            JudgeEvent { time: 1.1, line_id: 0, note_id: 3, judgement: Judgement::Perfect },
            JudgeEvent { time: 1.2, line_id: 1, note_id: 9, judgement: Judgement::HoldGood },
        ],
    }
}

#[test]
fn write_read_roundtrip_uncompressed() {
    let dir = temp_dir();
    let rec = sample_record();
    let path = rec.write_to(&dir, CompressionType::None).unwrap();
    assert!(path.extension().unwrap() == "phirarec");
    let loaded = PhiraRecord::read_from(&path).unwrap();
    assert_eq!(loaded, rec);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn write_read_roundtrip_zstd() {
    let dir = temp_dir();
    let rec = sample_record();
    let path = rec.write_to(&dir, CompressionType::Zstd).unwrap();
    let loaded = PhiraRecord::read_from(&path).unwrap();
    assert_eq!(loaded, rec);
    // 压缩后文件应小于原始载荷（文本数据高度可压缩）
    let compressed = std::fs::read(&path).unwrap();
    assert!(compressed.len() < 300, "zstd 压缩应显著减小体积: {}", compressed.len());
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn roundtrip_empty_lists() {
    let dir = temp_dir();
    let rec = PhiraRecord {
        touch_frames: vec![],
        judge_events: vec![],
        ..sample_record()
    };
    for ct in [CompressionType::None, CompressionType::Zstd] {
        let path = rec.write_to(&dir, ct).unwrap();
        assert_eq!(PhiraRecord::read_from(&path).unwrap(), rec);
    }
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn parse_file_too_short() {
    assert!(PhiraRecord::parse(&[]).is_err());
    assert!(PhiraRecord::parse(&[0; 12]).is_err(), "12 字节 < 13 字节头部");
    assert!(PhiraRecord::parse(&[0; 12]).unwrap_err().contains("too short"));
}

#[test]
fn parse_bad_magic() {
    let mut data = vec![0u8; 13];
    data[..4].copy_from_slice(b"NOPE");
    let err = PhiraRecord::parse(&data).unwrap_err();
    assert!(err.contains("bad magic"), "{err}");
}

#[test]
fn parse_bad_version() {
    let mut data = vec![0u8; 13];
    data[..8].copy_from_slice(MAGIC);
    data[8] = 99; // version = 99 (LE)
    let err = PhiraRecord::parse(&data).unwrap_err();
    assert!(err.contains("unsupported version"), "{err}");
}

#[test]
fn parse_unknown_compression() {
    let mut data = vec![0u8; 13];
    data[..8].copy_from_slice(MAGIC);
    data[8..12].copy_from_slice(&FORMAT_VERSION.to_le_bytes());
    data[12] = 0x7F; // 未知压缩类型
    let err = PhiraRecord::parse(&data).unwrap_err();
    assert!(err.contains("unknown compression"), "{err}");
}

#[test]
fn parse_corrupt_zstd_payload() {
    let mut data = vec![0u8; 13];
    data[..8].copy_from_slice(MAGIC);
    data[8..12].copy_from_slice(&FORMAT_VERSION.to_le_bytes());
    data[12] = CompressionType::Zstd as u8;
    data.extend_from_slice(b"this is not zstd data at all........");
    let err = PhiraRecord::parse(&data).unwrap_err();
    assert!(err.contains("zstd"), "{err}");
}

#[test]
fn parse_truncated_payload_after_header() {
    // 头部合法但载荷截断 → 解码错误
    let dir = temp_dir();
    let rec = sample_record();
    let path = rec.write_to(&dir, CompressionType::None).unwrap();
    let data = std::fs::read(&path).unwrap();
    let truncated = &data[..data.len().saturating_sub(5)];
    let err = PhiraRecord::parse(truncated).unwrap_err();
    assert!(err.contains("decode payload"), "{err}");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn read_missing_file() {
    let err = PhiraRecord::read_from(&temp_dir().join("nope.phirarec")).unwrap_err();
    assert!(err.contains("read file"), "{err}");
}

#[test]
fn maybe_build_record_empty_data_is_none() {
    let r = maybe_build_record(
        1,
        2,
        "U",
        Some(42),
        Some("C".into()),
        vec![],
        vec![],
    );
    assert!(r.is_none());
}

#[test]
fn maybe_build_record_with_touches() {
    let rec = maybe_build_record(
        1,
        2,
        "U",
        Some(42),
        Some("C".into()),
        vec![TouchFrame { time: 1.0, points: vec![] }],
        vec![],
    )
    .unwrap();
    assert_eq!(rec.id, 1);
    assert_eq!(rec.user_id, 2);
    assert_eq!(rec.user_name, "U");
    assert_eq!(rec.chart_id, 42);
    assert_eq!(rec.chart_name, "C");
    assert_eq!(rec.touch_frames.len(), 1);
}

#[test]
fn maybe_build_record_with_judges_only() {
    let rec = maybe_build_record(
        9,
        1,
        "X",
        None,
        None,
        vec![],
        vec![JudgeEvent { time: 1.0, line_id: 0, note_id: 1, judgement: Judgement::Perfect }],
    )
    .unwrap();
    assert_eq!(rec.chart_id, 0, "chart_id None → 0");
    assert_eq!(rec.chart_name, "", "chart_name None → 空串");
    assert_eq!(rec.judge_events.len(), 1);
}
