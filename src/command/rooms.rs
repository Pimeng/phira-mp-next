//! 房间设置、房主调整和房间详情。

use std::sync::Arc;

use crate::log::{log_info, log_raw, log_warn};
use crate::server::ServerContext;

use super::parsing::parse_bool;
use super::{find_room_or_warn, render_line};

pub(super) fn maxusers(ctx: &Arc<ServerContext>, args: &[&str]) {
    let i18n = &ctx.i18n;
    if args.len() < 2 {
        log_warn!(i18n, "LOG_CMD_USAGE_MAXUSERS");
        return;
    }
    let (room_id, count) = (args[0], args[1]);
    let Some(count) = count.parse::<usize>().ok() else {
        log_warn!(i18n, "LOG_CMD_INVALID_COUNT", ("count", count));
        return;
    };
    let Some(room) = find_room_or_warn(ctx, room_id) else {
        return;
    };
    match room.admin_set_max_player(count) {
        Ok(()) => log_info!(
            i18n,
            "LOG_CMD_MAXUSERS_SET",
            ("room", room_id),
            ("count", count)
        ),
        Err(e) => log_warn!(i18n, "LOG_CMD_FAILED", ("err", e.0)),
    }
}

pub(super) fn nexthost(ctx: &Arc<ServerContext>, args: &[&str]) {
    let i18n = &ctx.i18n;
    if args.len() < 2 {
        log_warn!(i18n, "LOG_CMD_USAGE_NEXTHOST");
        return;
    }
    let (room_id, user_id) = (args[0], args[1]);
    let Some(user_id) = user_id.parse::<i32>().ok() else {
        log_warn!(i18n, "LOG_CMD_INVALID_USER_ID", ("id", user_id));
        return;
    };
    let Some(room) = find_room_or_warn(ctx, room_id) else {
        return;
    };
    match room.admin_set_next_host(user_id) {
        Ok(()) => {
            if !room.setting().cycle {
                log_warn!(i18n, "LOG_CMD_NEXTHOST_NOT_CYCLE", ("room", room_id));
            }
            log_info!(
                i18n,
                "LOG_CMD_NEXTHOST_SET",
                ("room", room_id),
                ("id", user_id)
            );
        }
        Err(e) => log_warn!(i18n, "LOG_CMD_FAILED", ("err", e.0)),
    }
}

pub(super) fn lock(ctx: &Arc<ServerContext>, args: &[&str]) {
    let i18n = &ctx.i18n;
    if args.len() < 2 {
        log_warn!(i18n, "LOG_CMD_USAGE_LOCK");
        return;
    }
    let (room_id, lock) = (args[0], args[1]);
    let Some(lock) = parse_bool(lock) else {
        log_warn!(i18n, "LOG_CMD_INVALID_BOOL", ("value", lock));
        return;
    };
    let Some(room) = find_room_or_warn(ctx, room_id) else {
        return;
    };
    match room.admin_set_locked(lock) {
        Ok(plan) => {
            futures::executor::block_on(crate::room::send_broadcasts(plan));
            log_info!(i18n, "LOG_CMD_LOCK_SET", ("room", room_id), ("value", lock));
        }
        Err(e) => log_warn!(i18n, "LOG_CMD_FAILED", ("err", e.0)),
    }
}

pub(super) fn cycle(ctx: &Arc<ServerContext>, args: &[&str]) {
    let i18n = &ctx.i18n;
    if args.len() < 2 {
        log_warn!(i18n, "LOG_CMD_USAGE_CYCLE");
        return;
    }
    let (room_id, cycle) = (args[0], args[1]);
    let Some(cycle) = parse_bool(cycle) else {
        log_warn!(i18n, "LOG_CMD_INVALID_BOOL", ("value", cycle));
        return;
    };
    let Some(room) = find_room_or_warn(ctx, room_id) else {
        return;
    };
    match room.admin_set_cycle(cycle) {
        Ok(plan) => {
            futures::executor::block_on(crate::room::send_broadcasts(plan));
            log_info!(
                i18n,
                "LOG_CMD_CYCLE_SET",
                ("room", room_id),
                ("value", cycle)
            );
        }
        Err(e) => log_warn!(i18n, "LOG_CMD_FAILED", ("err", e.0)),
    }
}

pub(super) fn sethost(ctx: &Arc<ServerContext>, args: &[&str]) {
    let i18n = &ctx.i18n;
    if args.len() < 2 {
        log_warn!(i18n, "LOG_CMD_USAGE_SETHOST");
        return;
    }
    let (room_id, user_id) = (args[0], args[1]);
    let Some(user_id) = user_id.parse::<i32>().ok() else {
        log_warn!(i18n, "LOG_CMD_INVALID_USER_ID", ("id", user_id));
        return;
    };
    let Some(room) = find_room_or_warn(ctx, room_id) else {
        return;
    };
    match room.admin_transfer_host(user_id) {
        Ok(plan) => {
            futures::executor::block_on(crate::room::send_broadcasts(plan));
            log_info!(
                i18n,
                "LOG_CMD_HOST_TRANSFERRED",
                ("room", room_id),
                ("id", user_id)
            );
        }
        Err(e) => log_warn!(i18n, "LOG_CMD_FAILED", ("err", e.0)),
    }
}

pub(super) fn roominfo(ctx: &Arc<ServerContext>, args: &[&str]) {
    let i18n = &ctx.i18n;
    let Some(room_id) = args.first() else {
        log_warn!(i18n, "LOG_CMD_USAGE_ROOMINFO");
        return;
    };
    let Some(room) = find_room_or_warn(ctx, room_id) else {
        return;
    };
    let snap = room.snapshot();
    let setting = room.setting();
    let none = render_line(i18n, "LOG_CMD_NONE", &[]);
    let name_of = |id: i32| {
        ctx.players
            .get(id)
            .map(|p| p.name())
            .unwrap_or_else(|| none.clone())
    };
    let mut out = render_line(i18n, "LOG_CMD_ROOMINFO_TITLE", &[("room", &snap.room_id)]);
    out.push_str(&render_line(
        i18n,
        "LOG_CMD_ROOMINFO_STATE",
        &[("state", snap.state_kind())],
    ));
    out.push_str(&render_line(
        i18n,
        "LOG_CMD_ROOMINFO_LOCKED",
        &[("locked", &snap.locked.to_string())],
    ));
    out.push_str(&render_line(
        i18n,
        "LOG_CMD_ROOMINFO_CYCLE",
        &[("cycle", &setting.cycle.to_string())],
    ));
    out.push_str(&render_line(
        i18n,
        "LOG_CMD_ROOMINFO_CHAT",
        &[("chat", &setting.chat.to_string())],
    ));
    out.push_str(&render_line(
        i18n,
        "LOG_CMD_ROOMINFO_MAX",
        &[("count", &setting.max_player.to_string())],
    ));
    let host = snap
        .host
        .map(|h| format!("{h} ({})", name_of(h)))
        .unwrap_or_else(|| none.clone());
    out.push_str(&render_line(
        i18n,
        "LOG_CMD_ROOMINFO_HOST",
        &[("host", &host)],
    ));
    let chart = match snap.chart_name.as_deref() {
        Some(c) => c.to_string(),
        None => none.clone(),
    };
    out.push_str(&render_line(
        i18n,
        "LOG_CMD_ROOMINFO_CHART",
        &[("chart", &chart)],
    ));
    out.push_str(&render_line(
        i18n,
        "LOG_CMD_ROOMINFO_PLAYERS",
        &[("count", &snap.players.len().to_string())],
    ));
    for id in &snap.players {
        out.push_str(&render_line(
            i18n,
            "LOG_CMD_ROOMINFO_PLAYER_ITEM",
            &[("id", &id.to_string()), ("name", &name_of(*id))],
        ));
    }
    out.push_str(&render_line(
        i18n,
        "LOG_CMD_ROOMINFO_MONITORS",
        &[("count", &snap.monitors.len().to_string())],
    ));
    for id in &snap.monitors {
        out.push_str(&render_line(
            i18n,
            "LOG_CMD_ROOMINFO_PLAYER_ITEM",
            &[("id", &id.to_string()), ("name", &name_of(*id))],
        ));
    }
    log_raw(tracing::Level::INFO, &out);
}
