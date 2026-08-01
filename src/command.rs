//! 控制台命令线程（文档 1.1 节：支持控制台输入 "stop" 等）。

use std::io::BufRead;
use std::sync::Arc;

/// 启动控制台命令线程（阻塞读 stdin，独立线程不占用 tokio worker）。
pub fn start_command_thread(ctx: Arc<crate::server::ServerContext>) {
    std::thread::spawn(move || {
        let stdin = std::io::stdin();
        for line in stdin.lock().lines() {
            let Ok(line) = line else { break };
            let cmd = line.trim();
            if cmd.is_empty() {
                continue;
            }
            match cmd {
                "stop" => {
                    tracing::info!("stop command received");
                    ctx.request_shutdown();
                    break;
                }
                "online" => {
                    let players = ctx.players.online_players();
                    println!("online players ({}):", players.len());
                    for p in players {
                        println!("  {} ({})", p.name(), p.id());
                    }
                }
                "rooms" => {
                    let rooms = ctx.rooms.all_rooms();
                    println!("rooms ({}):", rooms.len());
                    for r in rooms {
                        let snap = r.snapshot();
                        println!(
                            "  {} state={:?} players={} monitors={} locked={}",
                            snap.room_id,
                            snap.state_kind(),
                            snap.players.len(),
                            snap.monitors.len(),
                            snap.locked
                        );
                    }
                }
                "help" => {
                    println!("commands: stop, online, rooms, help");
                }
                _ => println!("unknown command: {cmd} (try 'help')"),
            }
        }
    });
}
