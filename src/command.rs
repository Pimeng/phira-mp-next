//! 控制台命令（对应 Java `CommandService`）。
//!
//! 职责与 Java 完全对齐：
//! - `stop` → 优雅关闭（内置，不经过事件）。
//! - 其余命令 → 抛 `CommandProcessEvent`，任一订阅者 cancel（=已处理）；
//!   否则输出 `Unknown command`。
//! - 内置 `online`/`rooms`/`help` 以默认订阅者身份实现（可被扩展方先行处理）。

use crate::player::Player as _;
use std::io::BufRead;
use std::sync::Arc;

/// 启动控制台命令线程（阻塞读 stdin，独立线程不占用 tokio worker）。
pub fn start_command_thread(ctx: Arc<crate::server::ServerContext>) {
    // 内置命令：以订阅者实现（对应 Java 插件式命令注册）。
    let bus = ctx.events.clone();
    let ctx_cmds = ctx.clone();
    let sub = bus.subscribe_mut(
        crate::events::COMMAND_PROCESS,
        move |ev: &mut crate::events::CommandProcessEvent| {
            let ctx = ctx_cmds.clone();
            let cmd = ev.command.trim().to_string();
            let mut handled = false;
            match cmd.as_str() {
                "online" => {
                    let players = ctx.players.online_players();
                    println!("online players ({}):", players.len());
                    for p in players {
                        println!("  {} ({})", p.name(), p.id());
                    }
                    handled = true;
                }
                "rooms" => {
                    let rooms = ctx.rooms.all_rooms();
                    println!("rooms ({}):", rooms.len());
                    for r in rooms {
                        let snap = r.snapshot();
                        println!(
                            "  {} state={} players={} monitors={} locked={}",
                            snap.room_id,
                            snap.state_kind(),
                            snap.players.len(),
                            snap.monitors.len(),
                            snap.locked
                        );
                    }
                    handled = true;
                }
                "help" => {
                    println!("commands: stop, online, rooms, help (+ extension commands)");
                    handled = true;
                }
                _ => {}
            }
            if handled {
                ev.cancel("handled");
            }
            async {}
        },
    );
    std::mem::forget(sub); // 常驻订阅（与 server 同生命周期）

    std::thread::spawn(move || {
        let stdin = std::io::stdin();
        for line in stdin.lock().lines() {
            let Ok(line) = line else { break };
            let cmd = line.trim().to_string();
            if cmd.is_empty() {
                continue;
            }

            // stop：内置直接关闭（对应 Java runCommand 的 stop 特判）
            if cmd.eq_ignore_ascii_case("stop") {
                tracing::info!("stop command received");
                ctx.request_shutdown();
                break;
            }

            // CommandProcessEvent：任一订阅者 cancel 即视为已处理
            let bus = ctx.events.clone();
            let ev = crate::events::CommandProcessEvent {
                command: cmd.clone(),
                cancel_reason: None,
            };
            let handled = futures::executor::block_on(async move {
                let ev = bus.post_mut(crate::events::COMMAND_PROCESS, ev).await;
                ev.is_cancelled()
            });
            if !handled {
                println!("Unknown command: {cmd}");
            }
        }
    });
}
