//! 控制台命令（对应 Java `CommandService` + `SimpleTerminalConsole`）。
//!
//! 职责与 Java 对齐：
//! - 内建命令 `stop`/`online`/`rooms`/`help` 直接处理，不经事件总线
//!   （对应 Java runCommand 的 stop 特判 + 内建命令集）。
//! - 其余命令 → 抛 `CommandProcessEvent`（扩展点），任一订阅者 cancel（=已处理）；
//!   否则输出 `Unknown command`。
//!
//! 输入/回显走 [`terminal_console`]（对标 TCA 交互式终端 + 日志不打断输入行）。

use crate::player::Player as _;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use terminal_console::{self as console, ConsoleHandler};

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

impl CommandConsole {
    fn cmd_online(&self) {
        let players = self.ctx.players.online_players();
        let mut out = format!("Online players ({}):", players.len());
        for p in players {
            out.push_str(&format!("\n  {} ({})", p.name(), p.id()));
        }
        tracing::info!("{out}");
    }

    fn cmd_rooms(&self) {
        let rooms = self.ctx.rooms.all_rooms();
        let mut out = format!("Rooms ({}):", rooms.len());
        for r in rooms {
            let snap = r.snapshot();
            out.push_str(&format!(
                "\n  {} state={} players={} monitors={} locked={}",
                snap.room_id,
                snap.state_kind(),
                snap.players.len(),
                snap.monitors.len(),
                snap.locked
            ));
        }
        tracing::info!("{out}");
    }
}

impl ConsoleHandler for CommandConsole {
    fn is_running(&self) -> bool {
        self.running.load(Ordering::SeqCst) && !self.ctx.is_shutdown_requested()
    }

    fn on_command(&mut self, command: &str) {
        let cmd = command.trim();
        if cmd.is_empty() {
            return;
        }

        // 内建命令：直接处理，不经事件总线（大小写不敏感，对应 Java equalsIgnoreCase）
        match cmd.to_ascii_lowercase().as_str() {
            "stop" => {
                self.running.store(false, Ordering::SeqCst);
                self.ctx.request_shutdown();
                return;
            }
            "online" => {
                self.cmd_online();
                return;
            }
            "rooms" => {
                self.cmd_rooms();
                return;
            }
            "help" => {
                tracing::info!("Commands: stop, online, rooms, help (+ extension commands)");
                return;
            }
            _ => {}
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
            tracing::warn!("Unknown command: {cmd}");
        }
    }

    fn on_shutdown(&mut self) {
        // Ctrl+C（对应 TCA UserInterruptException → shutdown）
        self.running.store(false, Ordering::SeqCst);
        self.ctx.request_shutdown();
    }
}
