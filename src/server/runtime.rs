//! TCP/HTTP 服务启动、连接接收与优雅关闭。

use super::{ServerArgs, ServerContext};
use crate::log::{log_info, log_warn};
use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::time::{Duration, Instant};
use tokio::net::TcpListener;

/// 启动服务端（1.1 节；对应 Java `Server.main` + 生命周期事件）。
pub async fn run(args: ServerArgs) -> std::io::Result<()> {
    let boot_start = Instant::now();
    let ctx = ServerContext::new(args);

    let listener = start_services(&ctx, boot_start).await?;
    listen_for_shutdown_signal(&ctx);
    accept_connections(&ctx, &listener).await;
    shutdown(&ctx, listener, boot_start).await;
    Ok(())
}

/// 按启动顺序初始化控制台、TCP、HTTP，再通知扩展服务已经就绪。
async fn start_services(
    ctx: &Arc<ServerContext>,
    boot_start: Instant,
) -> std::io::Result<TcpListener> {
    // 第一条日志：当前服务器语言（多语言日志系统；按服务器默认语言渲染）。
    // 回退链：玩家语言 → 服务器默认 → zh-CN → key。
    log_info!(
        &ctx.i18n,
        &ctx.args.language,
        "LOG_LANGUAGE",
        ("lang", &ctx.args.language),
    );
    log_info!(&ctx.i18n, None, "LOG_BOOTING");

    // 控制台命令线程（对应 Java CommandService）
    crate::command::start_command_thread(ctx.clone());

    log_info!(&ctx.i18n, None, "LOG_INIT_NETWORK");
    let listener = TcpListener::bind(format!("{}:{}", ctx.args.host, ctx.args.port)).await?;
    *ctx.listen_addr.write().unwrap() = Some(listener.local_addr()?.to_string());
    log_info!(
        &ctx.i18n,
        None,
        "LOG_LISTENING",
        ("host", &ctx.args.host),
        ("port", ctx.args.port),
    );
    // HTTP 查询 API（GET /api/rooms）
    if ctx.args.http_port > 0 {
        log_info!(&ctx.i18n, None, "LOG_INIT_HTTP");
        crate::http::start(ctx.clone(), ctx.args.host.clone(), ctx.args.http_port).await?;
    }
    ctx.events
        .post(
            crate::events::SERVER_LIFECYCLE,
            crate::events::ServerLifecycleEvent {
                phase: crate::events::LifecyclePhase::Started,
            },
        )
        .await;
    log_info!(
        &ctx.i18n,
        None,
        "LOG_DONE",
        ("secs", format!("{:.3}", boot_start.elapsed().as_secs_f64())),
    );
    log_info!(&ctx.i18n, None, "LOG_SERVER_RUNNING");

    Ok(listener)
}

fn listen_for_shutdown_signal(ctx: &Arc<ServerContext>) {
    let ctx = ctx.clone();
    tokio::spawn(async move {
        if tokio::signal::ctrl_c().await.is_ok() {
            ctx.request_shutdown();
        }
    });
}

async fn accept_connections(ctx: &Arc<ServerContext>, listener: &TcpListener) {
    loop {
        tokio::select! {
            accept = listener.accept() => {
                match accept {
                    Ok((stream, _addr)) => {
                        let ctx = ctx.clone();
                        let proxy = ctx.args.proxy_protocol;
                        tokio::spawn(async move {
                            let initial = (ctx.extensions.initial_handler.read().unwrap())();
                            crate::network::connection::spawn_connection(stream, ctx, proxy, initial).await;
                        });
                    }
                    Err(e) => {
                        log_warn!(&ctx.i18n, "LOG_SERVER_ACCEPT_ERROR", ("err", e.to_string()));
                    }
                }
            }
            _ = ctx.wait_shutdown() => {
                break;
            }
        }
    }
}

async fn shutdown(ctx: &Arc<ServerContext>, listener: TcpListener, boot_start: Instant) {
    // 关闭流程（1.3 节；对齐 Java Server.shutdown 日志序列）：
    // Shutting down... → Kicking {n} player(s)... → Closing {m} channel(s)...
    // → Channels closed. → Uptime → Shutdown completed in {ms}ms. Goodbye!
    ctx.events
        .post(
            crate::events::SERVER_LIFECYCLE,
            crate::events::ServerLifecycleEvent {
                phase: crate::events::LifecyclePhase::Stopping,
            },
        )
        .await;
    let shutdown_start = Instant::now();
    log_info!(&ctx.i18n, None, "LOG_SHUTTING_DOWN");
    let kick_count = ctx.players.online_players().len();
    // 对应 Java allChannels：server channel + 活跃连接
    let channel_count = ctx.active_connections.load(Ordering::SeqCst) + 1;
    if kick_count > 0 {
        log_info!(
            &ctx.i18n,
            None,
            "LOG_KICKING_PLAYERS",
            ("count", kick_count)
        );
    }
    if channel_count > 0 {
        log_info!(
            &ctx.i18n,
            None,
            "LOG_CLOSING_CHANNELS",
            ("count", channel_count),
        );
    }
    for player in ctx.players.online_players() {
        player.kick();
        if let Some(lp) = crate::player::local_of(&player) {
            lp.connection().close().await;
        }
    }
    drop(listener);
    // 等待连接任务退出（最多 10 秒，对应 Java close().awaitUninterruptibly(10s)）
    if channel_count > 0 {
        let _ = tokio::time::timeout(Duration::from_secs(10), async {
            while ctx.active_connections.load(Ordering::SeqCst) > 0 {
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await;
        log_info!(&ctx.i18n, None, "LOG_CHANNELS_CLOSED");
    }
    let uptime = boot_start.elapsed();
    log_info!(
        &ctx.i18n,
        None,
        "LOG_UPTIME",
        ("min", uptime.as_secs() / 60),
        ("sec", uptime.as_secs() % 60),
    );
    log_info!(
        &ctx.i18n,
        None,
        "LOG_SHUTDOWN_COMPLETED",
        ("ms", shutdown_start.elapsed().as_millis()),
    );
    ctx.events
        .post(
            crate::events::SERVER_LIFECYCLE,
            crate::events::ServerLifecycleEvent {
                phase: crate::events::LifecyclePhase::Stopped,
            },
        )
        .await;
    ctx.notify_stopped();
}
