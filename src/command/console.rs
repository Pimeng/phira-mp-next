//! 终端线程生命周期，以及未被内建命令处理的扩展事件。

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use terminal_console::{self as console, ConsoleHandler};

use crate::log::log_warn;
use crate::server::ServerContext;

use super::dispatch_command;
use super::parsing::ParsedCommand;

/// 启动控制台命令线程（独立线程，不占用 tokio worker）。
pub fn start_command_thread(ctx: Arc<ServerContext>) {
    let handler = CommandConsole {
        running: AtomicBool::new(true),
        ctx,
    };
    std::thread::spawn(move || console::Console::builder().build().run(handler));
}

/// 控制台命令处理器（对应 TCA `SimpleTerminalConsole` 的 isRunning/runCommand/shutdown）。
struct CommandConsole {
    running: AtomicBool,
    ctx: Arc<ServerContext>,
}

impl ConsoleHandler for CommandConsole {
    fn is_running(&self) -> bool {
        self.running.load(Ordering::SeqCst) && !self.ctx.is_shutdown_requested()
    }

    fn on_command(&mut self, command: &str) {
        let Some(command) = ParsedCommand::parse(command) else {
            return;
        };

        // stop 由终端处理，确保输入循环与服务器同时退出。
        if command.name == "stop" {
            self.running.store(false, Ordering::SeqCst);
            self.ctx.request_shutdown();
            return;
        }

        // 内建/管理命令：直接处理，不经事件总线
        if dispatch_command(&self.ctx, &command) {
            return;
        }

        // 扩展命令：CommandProcessEvent，任一订阅者 cancel 即视为已处理
        let bus = self.ctx.events.clone();
        let ev = crate::events::CommandProcessEvent {
            command: command.raw.to_string(),
            cancel_reason: None,
        };
        let handled = futures::executor::block_on(async move {
            let ev = bus.post_mut(crate::events::COMMAND_PROCESS, ev).await;
            ev.is_cancelled()
        });
        if !handled {
            log_warn!(&self.ctx.i18n, "LOG_CMD_UNKNOWN", ("cmd", command.raw));
        }
    }

    fn on_shutdown(&mut self) {
        // Ctrl+C（对应 TCA UserInterruptException → shutdown）
        self.running.store(false, Ordering::SeqCst);
        self.ctx.request_shutdown();
    }
}
