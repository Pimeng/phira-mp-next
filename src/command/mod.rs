//! 控制台命令分发。
//!
//! 内建命令直接调用领域接口；未知命令由终端发送 `CommandProcessEvent`。
//! `process_command` 可独立于终端调用，返回是否识别了命令（参数错误也算已处理）。
//! `stop` 与扩展事件由 `console` 负责，其余实现按查询、封禁、消息、房间管理拆分。

mod console;
mod messaging;
mod moderation;
mod parsing;
mod rooms;
mod status;

#[cfg(test)]
mod tests;

pub use console::start_command_thread;

use std::sync::Arc;

use crate::i18n::I18nService;
use crate::log::{log_warn, render_message};
use crate::room::Room;
use crate::server::ServerContext;

use parsing::ParsedCommand;

/// 处理内建/管理命令。空行和参数错误返回 `true`，未知命令返回 `false`。
/// `stop` 由终端处理，不在此处触发关闭。
pub fn process_command(ctx: &Arc<ServerContext>, input: &str) -> bool {
    let Some(command) = ParsedCommand::parse(input) else {
        return true;
    };
    dispatch_command(ctx, &command)
}

fn dispatch_command(ctx: &Arc<ServerContext>, command: &ParsedCommand<'_>) -> bool {
    match command.name.as_str() {
        "online" => status::online(ctx),
        "rooms" => status::rooms(ctx),
        "help" => status::help(ctx),
        "ban" => moderation::ban(ctx, &command.args),
        "unban" | "pardon" => moderation::unban(ctx, &command.args),
        "banlist" => moderation::banlist(ctx, &command.args),
        "banroom" => moderation::banroom(ctx, &command.args),
        "unbanroom" => moderation::unbanroom(ctx, &command.args),
        "broadcast" | "say" => messaging::broadcast(ctx, command.raw, &command.args),
        "roomsay" => messaging::roomsay(ctx, command.raw, &command.args),
        "maxusers" => rooms::maxusers(ctx, &command.args),
        "nexthost" => rooms::nexthost(ctx, &command.args),
        "lock" => rooms::lock(ctx, &command.args),
        "cycle" => rooms::cycle(ctx, &command.args),
        "sethost" => rooms::sethost(ctx, &command.args),
        "roominfo" => rooms::roominfo(ctx, &command.args),
        _ => return false,
    }
    true
}

/// 按服务器默认语言渲染一行，供命令拼接多行输出。
fn render_line(i18n: &I18nService, key: &str, args: &[(&str, &str)]) -> String {
    render_message(Some(i18n), None, key, args)
}

/// 房间命令在参数校验后统一执行查找，保持错误提示顺序一致。
fn find_room_or_warn(ctx: &ServerContext, room_id: &str) -> Option<Arc<dyn Room>> {
    let room = ctx.rooms.find_room(room_id);
    if room.is_none() {
        log_warn!(&ctx.i18n, "LOG_CMD_ROOM_NOT_FOUND", ("room", room_id));
    }
    room
}
