//! 全服和房间系统消息。

use std::sync::Arc;

use crate::log::{log_info, log_warn};
use crate::packet::clientbound::{ClientBoundPacket, encode_shared};
use crate::packet::message::Message;
use crate::server::ServerContext;

use super::find_room_or_warn;
use super::parsing::tail_after;

pub(super) fn broadcast(ctx: &Arc<ServerContext>, cmd: &str, args: &[&str]) {
    let i18n = &ctx.i18n;
    if args.is_empty() {
        log_warn!(i18n, "LOG_CMD_USAGE_BROADCAST");
        return;
    }
    let Some(content) = tail_after(cmd, 1) else {
        log_warn!(i18n, "LOG_CMD_USAGE_BROADCAST");
        return;
    };
    // 全服广播：user=0 表示系统消息
    let frame = encode_shared(&ClientBoundPacket::message(Message::Chat {
        user: 0,
        content: content.clone(),
    }));
    let targets = ctx.players.online_players();
    let count = targets.len();
    futures::executor::block_on(async move {
        for p in targets {
            p.send_frame(frame.clone()).await;
        }
    });
    log_info!(
        i18n,
        "LOG_CMD_BROADCAST_SENT",
        ("count", count),
        ("content", content),
    );
}

pub(super) fn roomsay(ctx: &Arc<ServerContext>, cmd: &str, args: &[&str]) {
    let i18n = &ctx.i18n;
    if args.len() < 2 {
        log_warn!(i18n, "LOG_CMD_USAGE_ROOMSAY");
        return;
    }
    let room_id = args[0];
    let Some(content) = tail_after(cmd, 2) else {
        log_warn!(i18n, "LOG_CMD_USAGE_ROOMSAY");
        return;
    };
    let Some(room) = find_room_or_warn(ctx, room_id) else {
        return;
    };
    match room.admin_chat(content) {
        Ok(plan) => {
            futures::executor::block_on(crate::room::send_broadcasts(plan));
            log_info!(i18n, "LOG_CMD_ROOMSAY_SENT", ("room", room_id));
        }
        Err(e) => log_warn!(i18n, "LOG_CMD_ROOMSAY_FAILED", ("err", e.0)),
    }
}
