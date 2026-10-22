//! 全服与房间封禁命令。

use std::sync::Arc;

use crate::log::{log_info, log_raw, log_warn};
use crate::server::ServerContext;

use super::render_line;

pub(super) fn ban(ctx: &Arc<ServerContext>, args: &[&str]) {
    let i18n = &ctx.i18n;
    let Some(user_id) = args.first().and_then(|s| s.parse::<i32>().ok()) else {
        log_warn!(i18n, "LOG_CMD_USAGE_BAN");
        return;
    };
    if !ctx.bans.ban(user_id, None) {
        log_warn!(i18n, "LOG_CMD_ALREADY_BANNED", ("id", user_id));
    }
    // 已在线 → 立即踢出（断线清理流程负责移除注册/退房）
    if let Some(p) = ctx.players.get(user_id) {
        let name = p.name();
        p.kick();
        if let Some(lp) = crate::player::local_of(&p) {
            let conn = lp.connection();
            futures::executor::block_on(conn.close());
        }
        log_info!(
            i18n,
            "LOG_CMD_BANNED_NAMED",
            ("id", user_id),
            ("name", name)
        );
    } else {
        log_info!(i18n, "LOG_CMD_BANNED", ("id", user_id));
    }
}

pub(super) fn unban(ctx: &Arc<ServerContext>, args: &[&str]) {
    let i18n = &ctx.i18n;
    let Some(user_id) = args.first().and_then(|s| s.parse::<i32>().ok()) else {
        log_warn!(i18n, "LOG_CMD_USAGE_UNBAN");
        return;
    };
    if ctx.bans.unban(user_id) {
        log_info!(i18n, "LOG_CMD_UNBANNED", ("id", user_id));
    } else {
        log_warn!(i18n, "LOG_CMD_NOT_BANNED", ("id", user_id));
    }
}

pub(super) fn banlist(ctx: &Arc<ServerContext>, args: &[&str]) {
    let i18n = &ctx.i18n;
    if !args.is_empty() {
        log_warn!(i18n, "LOG_CMD_USAGE_BANLIST");
        return;
    }
    let list = ctx.bans.ban_list();
    if list.is_empty() {
        log_info!(i18n, "LOG_CMD_NO_BANNED");
        return;
    }
    let mut out = render_line(
        i18n,
        "LOG_CMD_BANLIST_TITLE",
        &[("count", &list.len().to_string())],
    );
    for (id, reason) in list {
        match reason {
            Some(r) => out.push_str(&render_line(
                i18n,
                "LOG_CMD_BANLIST_ITEM",
                &[("id", &id.to_string()), ("reason", &r)],
            )),
            None => out.push_str(&render_line(
                i18n,
                "LOG_CMD_BANLIST_ITEM_PLAIN",
                &[("id", &id.to_string())],
            )),
        }
    }
    log_raw(tracing::Level::INFO, &out);
}

pub(super) fn banroom(ctx: &Arc<ServerContext>, args: &[&str]) {
    let i18n = &ctx.i18n;
    let (Some(user_id), Some(room_id)) = (
        args.first().and_then(|s| s.parse::<i32>().ok()),
        args.get(1),
    ) else {
        log_warn!(i18n, "LOG_CMD_USAGE_BANROOM");
        return;
    };
    ctx.bans.ban_room(room_id, user_id);
    // 玩家若在该房间 → 移出
    if let Some(room) = ctx.rooms.find_room(room_id)
        && room.contains_member(user_id)
    {
        let (_l, plan, _d) = room.leave(user_id);
        futures::executor::block_on(crate::room::send_broadcasts(plan));
        log_info!(
            i18n,
            "LOG_CMD_REMOVED_FROM_ROOM",
            ("id", user_id),
            ("room", room_id)
        );
    }
    log_info!(
        i18n,
        "LOG_CMD_BANNED_FROM_ROOM",
        ("id", user_id),
        ("room", room_id)
    );
}

pub(super) fn unbanroom(ctx: &Arc<ServerContext>, args: &[&str]) {
    let i18n = &ctx.i18n;
    let (Some(user_id), Some(room_id)) = (
        args.first().and_then(|s| s.parse::<i32>().ok()),
        args.get(1),
    ) else {
        log_warn!(i18n, "LOG_CMD_USAGE_UNBANROOM");
        return;
    };
    if ctx.bans.unban_room(room_id, user_id) {
        log_info!(
            i18n,
            "LOG_CMD_UNBANNED_FROM_ROOM",
            ("id", user_id),
            ("room", room_id)
        );
    } else {
        log_warn!(
            i18n,
            "LOG_CMD_NOT_BANNED_FROM_ROOM",
            ("id", user_id),
            ("room", room_id)
        );
    }
}
