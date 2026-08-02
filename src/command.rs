//! 控制台命令（对应 Java `CommandService` + `SimpleTerminalConsole`）。
//!
//! 职责与 Java 对齐：
//! - 内建命令 `stop`/`online`/`rooms`/`help` 直接处理，不经事件总线
//!   （对应 Java runCommand 的 stop 特判 + 内建命令集）。
//! - 管理命令（封禁/广播/房间管理）直接处理 [`crate::ban::BanManager`] 与房间领域接口。
//! - 其余命令 → 抛 `CommandProcessEvent`（扩展点），任一订阅者 cancel（=已处理）；
//!   否则输出 `Unknown command`。
//!
//! 输入/回显走 [`terminal_console`]（对标 TCA 交互式终端 + 日志不打断输入行）。

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use terminal_console::{self as console, ConsoleHandler};

use crate::i18n::I18nService;
use crate::log::{log_info, log_raw, log_warn, render_message};
use crate::packet::clientbound::{encode_shared, ClientBoundPacket};
use crate::packet::message::Message;
use crate::server::ServerContext;

/// 渲染一条服务器默认语言的 i18n 行（命令多行输出拼接用）。
fn line(i18n: &I18nService, key: &str, args: &[(&str, &str)]) -> String {
    render_message(Some(i18n), None, key, args)
}

/// 启动控制台命令线程（独立线程，不占用 tokio worker）。
pub fn start_command_thread(ctx: Arc<crate::server::ServerContext>) {
    let handler = CommandConsole { running: AtomicBool::new(true), ctx };
    std::thread::spawn(move || console::Console::builder().build().run(handler));
}

/// 控制台命令处理器（对应 TCA `SimpleTerminalConsole` 的 isRunning/runCommand/shutdown）。
struct CommandConsole {
    running: AtomicBool,
    ctx: Arc<crate::server::ServerContext>,
}

// ---------------------------------------------------------------------------
// 内建命令实现（自由函数，供 ConsoleHandler 与测试共用）
// ---------------------------------------------------------------------------

/// 处理一条内建/管理命令。返回 `true` 表示已处理（不再走扩展命令事件）。
/// `stop` 由 [`CommandConsole::on_command`] 特判（需修改 running 状态），不在此处理。
pub fn process_command(ctx: &Arc<ServerContext>, command: &str) -> bool {
    let cmd = command.trim();
    if cmd.is_empty() {
        return true; // 空行不算 unknown
    }
    let mut parts = cmd.split_whitespace();
    let name = parts.next().unwrap_or("").to_ascii_lowercase();
    let args: Vec<&str> = parts.collect();

    match name.as_str() {
        "online" => cmd_online(ctx),
        "rooms" => cmd_rooms(ctx),
        "help" => cmd_help(ctx),
        "ban" => cmd_ban(ctx, &args),
        "unban" | "pardon" => cmd_unban(ctx, &args),
        "banlist" => cmd_banlist(ctx, &args),
        "banroom" => cmd_banroom(ctx, &args),
        "unbanroom" => cmd_unbanroom(ctx, &args),
        "broadcast" | "say" => cmd_broadcast(ctx, cmd, &args),
        "roomsay" => cmd_roomsay(ctx, cmd, &args),
        "maxusers" => cmd_maxusers(ctx, &args),
        "nexthost" => cmd_nexthost(ctx, &args),
        "lock" => cmd_lock(ctx, &args),
        "cycle" => cmd_cycle(ctx, &args),
        "sethost" => cmd_sethost(ctx, &args),
        "roominfo" => cmd_roominfo(ctx, &args),
        _ => return false,
    }
    true
}

// ---- 参数解析辅助 ----

/// 取命令中跳过前 `skip` 个空白分隔 token 后的剩余文本（保留空格）。
/// `skip = 1` 跳过命令名；`skip = 2` 再跳过一个参数（如 roomsay 的 roomId）。
fn tail_after(cmd: &str, skip: usize) -> Option<String> {
    let mut rest = cmd.trim();
    for _ in 0..skip {
        let i = rest.find(|c: char| c.is_whitespace())?;
        rest = rest[i..].trim_start();
    }
    (!rest.is_empty()).then(|| rest.to_string())
}

/// 解析布尔参数（true/false/1/0/yes/no/on/off，大小写不敏感）。
fn parse_bool(s: &str) -> Option<bool> {
    match s.to_ascii_lowercase().as_str() {
        "true" | "1" | "yes" | "on" => Some(true),
        "false" | "0" | "no" | "off" => Some(false),
        _ => None,
    }
}

// ---- online / rooms / help ----

fn cmd_online(ctx: &Arc<ServerContext>) {
    let i18n = &ctx.i18n;
    let players = ctx.players.online_players();
    let mut out = line(i18n, "LOG_CMD_ONLINE_TITLE", &[("count", &players.len().to_string())]);
    for p in players {
        out.push_str(&line(
            i18n,
            "LOG_CMD_ONLINE_ITEM",
            &[("id", &p.id().to_string()), ("name", &p.name())],
        ));
    }
    log_raw(tracing::Level::INFO, &out);
}

fn cmd_rooms(ctx: &Arc<ServerContext>) {
    let i18n = &ctx.i18n;
    let rooms = ctx.rooms.all_rooms();
    let mut out = line(i18n, "LOG_CMD_ROOMS_TITLE", &[("count", &rooms.len().to_string())]);
    for r in rooms {
        let snap = r.snapshot();
        out.push_str(&line(
            i18n,
            "LOG_CMD_ROOMS_ITEM",
            &[
                ("id", &snap.room_id),
                ("state", &snap.state_kind().to_string()),
                ("players", &snap.players.len().to_string()),
                ("monitors", &snap.monitors.len().to_string()),
                ("locked", &snap.locked.to_string()),
            ],
        ));
    }
    log_raw(tracing::Level::INFO, &out);
}

fn cmd_help(ctx: &Arc<ServerContext>) {
    let i18n = &ctx.i18n;
    let mut out = line(i18n, "LOG_CMD_HELP_TITLE", &[]);
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
        out.push_str(&line(i18n, key, &[]));
    }
    log_raw(tracing::Level::INFO, &out);
}

// ---- 封禁 ----

fn cmd_ban(ctx: &Arc<ServerContext>, args: &[&str]) {
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
        log_info!(i18n, "LOG_CMD_BANNED_NAMED", ("id", user_id), ("name", name));
    } else {
        log_info!(i18n, "LOG_CMD_BANNED", ("id", user_id));
    }
}

fn cmd_unban(ctx: &Arc<ServerContext>, args: &[&str]) {
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

fn cmd_banlist(ctx: &Arc<ServerContext>, args: &[&str]) {
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
    let mut out = line(i18n, "LOG_CMD_BANLIST_TITLE", &[("count", &list.len().to_string())]);
    for (id, reason) in list {
        match reason {
            Some(r) => out.push_str(&line(
                i18n,
                "LOG_CMD_BANLIST_ITEM",
                &[("id", &id.to_string()), ("reason", &r)],
            )),
            None => out.push_str(&line(
                i18n,
                "LOG_CMD_BANLIST_ITEM_PLAIN",
                &[("id", &id.to_string())],
            )),
        }
    }
    log_raw(tracing::Level::INFO, &out);
}

fn cmd_banroom(ctx: &Arc<ServerContext>, args: &[&str]) {
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
    if let Some(room) = ctx.rooms.find_room(room_id) {
        if room.contains_member(user_id) {
            let (_l, plan, _d) = room.leave(user_id);
            futures::executor::block_on(crate::room::send_broadcasts(plan));
            log_info!(i18n, "LOG_CMD_REMOVED_FROM_ROOM", ("id", user_id), ("room", room_id));
        }
    }
    log_info!(i18n, "LOG_CMD_BANNED_FROM_ROOM", ("id", user_id), ("room", room_id));
}

fn cmd_unbanroom(ctx: &Arc<ServerContext>, args: &[&str]) {
    let i18n = &ctx.i18n;
    let (Some(user_id), Some(room_id)) = (
        args.first().and_then(|s| s.parse::<i32>().ok()),
        args.get(1),
    ) else {
        log_warn!(i18n, "LOG_CMD_USAGE_UNBANROOM");
        return;
    };
    if ctx.bans.unban_room(room_id, user_id) {
        log_info!(i18n, "LOG_CMD_UNBANNED_FROM_ROOM", ("id", user_id), ("room", room_id));
    } else {
        log_warn!(i18n, "LOG_CMD_NOT_BANNED_FROM_ROOM", ("id", user_id), ("room", room_id));
    }
}

// ---- 广播 ----

fn cmd_broadcast(ctx: &Arc<ServerContext>, cmd: &str, args: &[&str]) {
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

fn cmd_roomsay(ctx: &Arc<ServerContext>, cmd: &str, args: &[&str]) {
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
    let Some(room) = ctx.rooms.find_room(room_id) else {
        log_warn!(i18n, "LOG_CMD_ROOM_NOT_FOUND", ("room", room_id));
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

// ---- 房间管理 ----

fn cmd_maxusers(ctx: &Arc<ServerContext>, args: &[&str]) {
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
    let Some(room) = ctx.rooms.find_room(room_id) else {
        log_warn!(i18n, "LOG_CMD_ROOM_NOT_FOUND", ("room", room_id));
        return;
    };
    match room.admin_set_max_player(count) {
        Ok(()) => log_info!(i18n, "LOG_CMD_MAXUSERS_SET", ("room", room_id), ("count", count)),
        Err(e) => log_warn!(i18n, "LOG_CMD_FAILED", ("err", e.0)),
    }
}

fn cmd_nexthost(ctx: &Arc<ServerContext>, args: &[&str]) {
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
    let Some(room) = ctx.rooms.find_room(room_id) else {
        log_warn!(i18n, "LOG_CMD_ROOM_NOT_FOUND", ("room", room_id));
        return;
    };
    match room.admin_set_next_host(user_id) {
        Ok(()) => {
            if !room.setting().cycle {
                log_warn!(i18n, "LOG_CMD_NEXTHOST_NOT_CYCLE", ("room", room_id));
            }
            log_info!(i18n, "LOG_CMD_NEXTHOST_SET", ("room", room_id), ("id", user_id));
        }
        Err(e) => log_warn!(i18n, "LOG_CMD_FAILED", ("err", e.0)),
    }
}

fn cmd_lock(ctx: &Arc<ServerContext>, args: &[&str]) {
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
    let Some(room) = ctx.rooms.find_room(room_id) else {
        log_warn!(i18n, "LOG_CMD_ROOM_NOT_FOUND", ("room", room_id));
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

fn cmd_cycle(ctx: &Arc<ServerContext>, args: &[&str]) {
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
    let Some(room) = ctx.rooms.find_room(room_id) else {
        log_warn!(i18n, "LOG_CMD_ROOM_NOT_FOUND", ("room", room_id));
        return;
    };
    match room.admin_set_cycle(cycle) {
        Ok(plan) => {
            futures::executor::block_on(crate::room::send_broadcasts(plan));
            log_info!(i18n, "LOG_CMD_CYCLE_SET", ("room", room_id), ("value", cycle));
        }
        Err(e) => log_warn!(i18n, "LOG_CMD_FAILED", ("err", e.0)),
    }
}

fn cmd_sethost(ctx: &Arc<ServerContext>, args: &[&str]) {
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
    let Some(room) = ctx.rooms.find_room(room_id) else {
        log_warn!(i18n, "LOG_CMD_ROOM_NOT_FOUND", ("room", room_id));
        return;
    };
    match room.admin_transfer_host(user_id) {
        Ok(plan) => {
            futures::executor::block_on(crate::room::send_broadcasts(plan));
            log_info!(i18n, "LOG_CMD_HOST_TRANSFERRED", ("room", room_id), ("id", user_id));
        }
        Err(e) => log_warn!(i18n, "LOG_CMD_FAILED", ("err", e.0)),
    }
}

fn cmd_roominfo(ctx: &Arc<ServerContext>, args: &[&str]) {
    let i18n = &ctx.i18n;
    let Some(room_id) = args.first() else {
        log_warn!(i18n, "LOG_CMD_USAGE_ROOMINFO");
        return;
    };
    let Some(room) = ctx.rooms.find_room(room_id) else {
        log_warn!(i18n, "LOG_CMD_ROOM_NOT_FOUND", ("room", room_id));
        return;
    };
    let snap = room.snapshot();
    let setting = room.setting();
    let none = line(i18n, "LOG_CMD_NONE", &[]);
    let name_of = |id: i32| {
        ctx.players
            .get(id)
            .map(|p| p.name())
            .unwrap_or_else(|| none.clone())
    };
    let mut out = line(i18n, "LOG_CMD_ROOMINFO_TITLE", &[("room", &snap.room_id)]);
    out.push_str(&line(
        i18n,
        "LOG_CMD_ROOMINFO_STATE",
        &[("state", &snap.state_kind().to_string())],
    ));
    out.push_str(&line(
        i18n,
        "LOG_CMD_ROOMINFO_LOCKED",
        &[("locked", &snap.locked.to_string())],
    ));
    out.push_str(&line(
        i18n,
        "LOG_CMD_ROOMINFO_CYCLE",
        &[("cycle", &setting.cycle.to_string())],
    ));
    out.push_str(&line(
        i18n,
        "LOG_CMD_ROOMINFO_CHAT",
        &[("chat", &setting.chat.to_string())],
    ));
    out.push_str(&line(
        i18n,
        "LOG_CMD_ROOMINFO_MAX",
        &[("count", &setting.max_player.to_string())],
    ));
    let host = snap
        .host
        .map(|h| format!("{h} ({})", name_of(h)))
        .unwrap_or_else(|| none.clone());
    out.push_str(&line(i18n, "LOG_CMD_ROOMINFO_HOST", &[("host", &host)]));
    let chart = match snap.chart_name.as_deref() {
        Some(c) => c.to_string(),
        None => none.clone(),
    };
    out.push_str(&line(i18n, "LOG_CMD_ROOMINFO_CHART", &[("chart", &chart)]));
    out.push_str(&line(
        i18n,
        "LOG_CMD_ROOMINFO_PLAYERS",
        &[("count", &snap.players.len().to_string())],
    ));
    for id in &snap.players {
        out.push_str(&line(
            i18n,
            "LOG_CMD_ROOMINFO_PLAYER_ITEM",
            &[("id", &id.to_string()), ("name", &name_of(*id))],
        ));
    }
    out.push_str(&line(
        i18n,
        "LOG_CMD_ROOMINFO_MONITORS",
        &[("count", &snap.monitors.len().to_string())],
    ));
    for id in &snap.monitors {
        out.push_str(&line(
            i18n,
            "LOG_CMD_ROOMINFO_PLAYER_ITEM",
            &[("id", &id.to_string()), ("name", &name_of(*id))],
        ));
    }
    log_raw(tracing::Level::INFO, &out);
}

// ---------------------------------------------------------------------------
// ConsoleHandler
// ---------------------------------------------------------------------------

impl ConsoleHandler for CommandConsole {
    fn is_running(&self) -> bool {
        self.running.load(Ordering::SeqCst) && !self.ctx.is_shutdown_requested()
    }

    fn on_command(&mut self, command: &str) {
        let cmd = command.trim();
        if cmd.is_empty() {
            return;
        }

        // stop：直接处理（需置 running=false，对应 Java 的 stop 特判）
        let name = cmd.split_whitespace().next().unwrap_or("").to_ascii_lowercase();
        if name == "stop" {
            self.running.store(false, Ordering::SeqCst);
            self.ctx.request_shutdown();
            return;
        }

        // 内建/管理命令：直接处理，不经事件总线
        if process_command(&self.ctx, cmd) {
            return;
        }

        // 扩展命令：CommandProcessEvent，任一订阅者 cancel 即视为已处理
        let bus = self.ctx.events.clone();
        let ev = crate::events::CommandProcessEvent {
            command: cmd.to_string(),
            cancel_reason: None,
        };
        let handled = futures::executor::block_on(async move {
            let ev = bus.post_mut(crate::events::COMMAND_PROCESS, ev).await;
            ev.is_cancelled()
        });
        if !handled {
            log_warn!(&self.ctx.i18n, "LOG_CMD_UNKNOWN", ("cmd", cmd));
        }
    }

    fn on_shutdown(&mut self) {
        // Ctrl+C（对应 TCA UserInterruptException → shutdown）
        self.running.store(false, Ordering::SeqCst);
        self.ctx.request_shutdown();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::network::connection::{ConnectionHandle, WriterMsg};
    use crate::phira::UserInfo;
    use crate::server::ServerArgs;
    use tokio::sync::mpsc;

    /// 独立 ServerContext（不启动网络；GLOBAL_CTX 被覆盖不影响本测试，均不依赖它）。
    fn ctx() -> Arc<ServerContext> {
        ServerContext::new(ServerArgs {
            port: 0,
            host: "127.0.0.1".into(),
            proxy_protocol: false,
            http_port: 0,
            language: "zh-CN".into(),
            session_timeout: 300,
            phira_api: "http://127.0.0.1/".into(),
            record_dir: None,
        })
    }

    /// 注册一个在线 LocalPlayer，返回 (玩家, writer 通道接收端)。
    /// rx 保持存活 → is_online() == true；可从 rx 读取发出的帧。
    fn register(
        ctx: &Arc<ServerContext>,
        id: i32,
        name: &str,
    ) -> (Arc<dyn crate::player::Player>, mpsc::UnboundedReceiver<WriterMsg>) {
        let (tx, rx) = mpsc::unbounded_channel::<WriterMsg>();
        let conn = ConnectionHandle::new_for_test(tx);
        let info = Arc::new(UserInfo { id, name: name.into(), ..Default::default() });
        let (res, _old) = ctx
            .players
            .resolve_player(
                info.clone(),
                Some(&conn),
                |i, c| crate::player::LocalPlayer::new(i, c.expect("conn")),
                |_p, _c| Ok(None),
            )
            .unwrap();
        (res.player, rx)
    }

    /// 剥掉 VarInt 帧头后解码 ClientBoundPacket。
    fn decode_frame_payload(frame: &crate::packet::clientbound::SharedFrame) -> Option<ClientBoundPacket> {
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

    #[test]
    fn ban_and_unban_commands() {
        let ctx = ctx();
        assert!(process_command(&ctx, "ban 42"));
        assert!(ctx.bans.is_banned(42));
        assert!(process_command(&ctx, "banlist"));
        // unban / pardon 均为解封
        assert!(process_command(&ctx, "pardon 42"));
        assert!(!ctx.bans.is_banned(42));
        assert!(process_command(&ctx, "ban 42"));
        assert!(ctx.bans.is_banned(42));
        assert!(process_command(&ctx, "unban 42"));
        assert!(!ctx.bans.is_banned(42));
    }

    #[test]
    fn ban_kicks_online_player() {
        let ctx = ctx();
        let (p, _rx) = register(&ctx, 1, "A");
        assert!(process_command(&ctx, "ban 1"));
        let lp = crate::player::local_of(&p).expect("local player");
        assert!(lp.is_kicked(), "在线玩家应被踢出");
    }

    #[test]
    fn banroom_unbanroom_commands() {
        let ctx = ctx();
        assert!(process_command(&ctx, "banroom 42 R1"));
        assert!(ctx.bans.is_room_banned("R1", 42));
        assert!(!ctx.bans.is_room_banned("R2", 42));
        assert!(process_command(&ctx, "unbanroom 42 R1"));
        assert!(!ctx.bans.is_room_banned("R1", 42));
        assert!(process_command(&ctx, "unbanroom 42 R1")); // 已解封
    }

    #[test]
    fn broadcast_sends_system_message_to_all_online() {
        let ctx = ctx();
        let (_p1, mut rx1) = register(&ctx, 1, "A");
        let (_p2, mut rx2) = register(&ctx, 2, "B");
        assert!(process_command(&ctx, "broadcast hello world"));
        for rx in [&mut rx1, &mut rx2] {
            let msg = rx.try_recv().expect("should receive frame");
            match msg {
                WriterMsg::Shared(frame) => {
                    let pkt = decode_frame_payload(&frame).expect("decode");
                    match pkt {
                        ClientBoundPacket::Message {
                            message: Message::Chat { user, content },
                            ..
                        } => {
                            assert_eq!(user, 0, "系统广播 user=0");
                            assert_eq!(content, "hello world");
                        }
                        other => panic!("expected Chat, got {other:?}"),
                    }
                }
                other => panic!("expected Shared frame, got {other:?}"),
            }
        }
    }

    #[test]
    fn maxusers_roominfo_commands() {
        let ctx = ctx();
        // 保持房间强引用，避免弱引用失效
        let _room = ctx.rooms.create_room("R1").unwrap();
        assert!(process_command(&ctx, "maxusers R1 4"));
        assert_eq!(ctx.rooms.find_room("R1").unwrap().setting().max_player, 4);
        assert!(process_command(&ctx, "roominfo R1"));
        assert!(process_command(&ctx, "roominfo")); // 缺参数不 panic
    }

    #[test]
    fn room_management_commands_with_players() {
        let ctx = ctx();
        let room = ctx.rooms.create_room("R1").unwrap();
        let (p1, _rx1) = register(&ctx, 1, "A");
        let (p2, _rx2) = register(&ctx, 2, "B");
        room.join(p1, false).unwrap();
        room.join(p2, false).unwrap();
        assert!(room.is_host(1));

        assert!(process_command(&ctx, "lock R1 true"));
        assert!(room.setting().locked);
        assert!(process_command(&ctx, "lock R1 false"));
        assert!(!room.setting().locked);

        assert!(process_command(&ctx, "cycle R1 true"));
        assert!(room.setting().cycle);

        assert!(process_command(&ctx, "sethost R1 2"));
        assert!(room.is_host(2));
        assert!(!room.is_host(1));

        assert!(process_command(&ctx, "nexthost R1 1"));
        // 循环模式对局结束 → 房主转移到指定玩家
        room.commit_select_chart(2, 1, "C".into()).unwrap();
        room.require_start(2).unwrap();
        room.ready(1).unwrap();
        room.commit_played(2, 1, 1.0, true).unwrap();
        let out = room.commit_played(1, 1, 1.0, true).unwrap();
        assert!(out.game_ended);
        assert!(room.is_host(1), "nexthost 应在循环对局结束后生效");
    }

    #[test]
    fn unknown_command_falls_through() {
        let ctx = ctx();
        assert!(!process_command(&ctx, "frobnicate 1 2 3"));
        assert!(process_command(&ctx, "")); // 空行不算 unknown
    }
}
