//! HTTP 服务（查询 API）。
//!
//! 基于 axum 提供轻量只读接口，与游戏 TCP 协议完全独立：
//! - `GET /api/rooms` —— 当前服务端房间列表（JSON）。

use crate::log::{log_error, log_info};
use crate::server::ServerContext;
use axum::extract::State;
use axum::routing::get;
use axum::{Json, Router};
use serde::Serialize;
use std::sync::Arc;

/// 启动 HTTP 服务，挂载于 `host:port`（绑定失败返回 Err 由调用方处理）。
pub async fn start(ctx: Arc<ServerContext>, host: String, port: u16) -> std::io::Result<()> {
    let app = Router::new()
        .route("/api/rooms", get(rooms))
        .with_state(ctx.clone());
    let listener = tokio::net::TcpListener::bind(format!("{host}:{port}")).await?;
    let addr = listener.local_addr()?.to_string();
    *ctx.http_addr.write().unwrap() = Some(addr.clone());
    log_info!(&ctx.i18n, "LOG_HTTP_LISTENING", ("addr", addr));
    tokio::spawn(async move {
        if let Err(e) = axum::serve(listener, app).await {
            log_error!(&ctx.i18n, "LOG_HTTP_ERROR", ("err", e.to_string()));
        }
    });
    Ok(())
}

/// `GET /api/rooms` 响应。
#[derive(Serialize)]
struct RoomsResponse {
    rooms: Vec<RoomView>,
    total: usize,
}

/// 单个房间的 HTTP 视图。
#[derive(Serialize)]
struct RoomView {
    roomid: String,
    cycle: bool,
    lock: bool,
    host: Option<HostView>,
    /// `select_chart` / `wait_for_ready` / `playing`
    state: String,
    chart: Option<ChartView>,
    players: Vec<PlayerView>,
}

#[derive(Serialize)]
struct HostView {
    name: String,
    id: String,
}

#[derive(Serialize)]
struct ChartView {
    name: String,
    id: String,
}

#[derive(Serialize)]
struct PlayerView {
    name: String,
    id: i32,
}

fn state_snake(kind: &str) -> &'static str {
    match kind {
        "SelectChart" => "select_chart",
        "WaitForReady" => "wait_for_ready",
        "Playing" => "playing",
        _ => "unknown",
    }
}

/// `GET /api/rooms`：列出全部存活房间及其成员概览。
/// 房间 id 以下划线 `_` 开头的（内部/隐藏房间）不显示。
async fn rooms(State(ctx): State<Arc<ServerContext>>) -> Json<RoomsResponse> {
    let list = ctx
        .rooms
        .all_rooms()
        .into_iter()
        .filter(|room| !room.id().starts_with('_'))
        .map(|room| {
            let snap = room.snapshot();
            let setting = room.setting();
            let player_view = |id: i32| PlayerView {
                name: ctx.players.get(id).map(|p| p.name()).unwrap_or_default(),
                id,
            };
            let host = snap.host.map(|id| HostView {
                name: ctx.players.get(id).map(|p| p.name()).unwrap_or_default(),
                id: id.to_string(),
            });
            let chart = match (snap.chart_id, &snap.chart_name) {
                (Some(id), Some(name)) => Some(ChartView {
                    name: name.clone(),
                    id: id.to_string(),
                }),
                _ => None,
            };
            RoomView {
                roomid: snap.room_id.clone(),
                cycle: setting.cycle,
                lock: snap.locked,
                host,
                state: state_snake(&snap.state_kind_name).to_string(),
                chart,
                players: snap.players.iter().map(|id| player_view(*id)).collect(),
            }
        })
        .collect::<Vec<_>>();
    Json(RoomsResponse {
        total: list.len(),
        rooms: list,
    })
}
