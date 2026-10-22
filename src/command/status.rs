//! 在线玩家、房间列表和命令帮助。

use std::sync::Arc;

use crate::log::log_raw;
use crate::server::ServerContext;

use super::render_line;

pub(super) fn online(ctx: &Arc<ServerContext>) {
    let i18n = &ctx.i18n;
    let players = ctx.players.online_players();
    let mut out = render_line(
        i18n,
        "LOG_CMD_ONLINE_TITLE",
        &[("count", &players.len().to_string())],
    );
    for p in players {
        out.push_str(&render_line(
            i18n,
            "LOG_CMD_ONLINE_ITEM",
            &[("id", &p.id().to_string()), ("name", &p.name())],
        ));
    }
    log_raw(tracing::Level::INFO, &out);
}

pub(super) fn rooms(ctx: &Arc<ServerContext>) {
    let i18n = &ctx.i18n;
    let rooms = ctx.rooms.all_rooms();
    let mut out = render_line(
        i18n,
        "LOG_CMD_ROOMS_TITLE",
        &[("count", &rooms.len().to_string())],
    );
    for r in rooms {
        let snap = r.snapshot();
        out.push_str(&render_line(
            i18n,
            "LOG_CMD_ROOMS_ITEM",
            &[
                ("id", &snap.room_id),
                ("state", snap.state_kind()),
                ("players", &snap.players.len().to_string()),
                ("monitors", &snap.monitors.len().to_string()),
                ("locked", &snap.locked.to_string()),
            ],
        ));
    }
    log_raw(tracing::Level::INFO, &out);
}

pub(super) fn help(ctx: &Arc<ServerContext>) {
    let i18n = &ctx.i18n;
    let mut out = render_line(i18n, "LOG_CMD_HELP_TITLE", &[]);
    for key in [
        "LOG_CMD_HELP_STOP",
        "LOG_CMD_HELP_ONLINE",
        "LOG_CMD_HELP_ROOMS",
        "LOG_CMD_HELP_BAN",
        "LOG_CMD_HELP_UNBAN",
        "LOG_CMD_HELP_BANLIST",
        "LOG_CMD_HELP_BANROOM",
        "LOG_CMD_HELP_UNBANROOM",
        "LOG_CMD_HELP_BROADCAST",
        "LOG_CMD_HELP_ROOMSAY",
        "LOG_CMD_HELP_MAXUSERS",
        "LOG_CMD_HELP_NEXTHOST",
        "LOG_CMD_HELP_LOCK",
        "LOG_CMD_HELP_CYCLE",
        "LOG_CMD_HELP_SETHOST",
        "LOG_CMD_HELP_ROOMINFO",
    ] {
        out.push('\n');
        out.push_str(&render_line(i18n, key, &[]));
    }
    log_raw(tracing::Level::INFO, &out);
}
