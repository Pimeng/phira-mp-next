//! LocalRoom 的成员、设置、对局与管理操作回归测试。

mod administration;
mod gameplay;
mod membership;
mod settings;

use super::*;
use crate::network::connection::ConnectionHandle;
use crate::packet::clientbound::ClientBoundPacket;
use crate::packet::message::Message;
use crate::phira::UserInfo;
use crate::player::LocalPlayer;

/// 构造测试用玩家（在线）。广播内容经 `broadcast plan` 的 SharedFrame 直接解码断言，
/// 无需经 socket/连接通道，故连接的 rx 仅 forget 保活。
fn player(id: i32, name: &str) -> Arc<dyn Player> {
    let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
    std::mem::forget(rx); // 保持 tx 配对端存活 → is_online() == true
    let conn = ConnectionHandle::new_for_test(tx);
    LocalPlayer::new(
        Arc::new(UserInfo {
            id,
            name: name.to_string(),
            ..Default::default()
        }),
        conn,
    )
}

fn make_room() -> Arc<LocalRoom> {
    LocalRoom::new("R1", RoomSetting::default(), || {})
}

/// 解码广播计划里发给指定玩家的第一个 message 包。
fn message_for(plan: &crate::room::behavior::Broadcast, user_id: i32) -> Option<Message> {
    plan.iter()
        .filter(|(p, _)| p.id() == user_id)
        .filter_map(|(_, frame)| {
            // frame 是「带 VarInt 帧头的完整字节」；剥掉帧头后解码
            decode_frame_payload(frame)
        })
        .find_map(|p| match p {
            ClientBoundPacket::Message { message, .. } => Some(message),
            _ => None,
        })
}

fn packet_for(plan: &crate::room::behavior::Broadcast, user_id: i32) -> Vec<ClientBoundPacket> {
    plan.iter()
        .filter(|(p, _)| p.id() == user_id)
        .filter_map(|(_, frame)| decode_frame_payload(frame))
        .collect()
}

/// 剥掉 VarInt 帧头后解码 ClientBoundPacket。
fn decode_frame_payload(
    frame: &crate::packet::clientbound::SharedFrame,
) -> Option<ClientBoundPacket> {
    // 跳过一个 VarInt（最多 5 字节）
    let bytes = frame.as_ref().as_ref();
    let mut i = 0;
    for _ in 0..5 {
        if i >= bytes.len() {
            return None;
        }
        let b = bytes[i];
        i += 1;
        if b & 0x80 == 0 {
            break;
        }
    }
    ClientBoundPacket::decode_frame(&bytes[i..]).ok()
}
