//! 认证 Handler（5.1 节）。
//!
//! - 仅接受 Authenticate 包；收到任何其他包（含 Ping）→ 立即关闭。
//! - 流程：token → Phira `/me` → PlayerManager.resolve_player →
//!   Create: 切 PlayHandler / Resume: 会话恢复（复用旧 handler）。
//! - 失败 → ClientBoundAuthenticatePacket.failed + 关闭。

use crate::network::connection::ConnectionHandle;
use crate::network::handler::{HandleOutcome, HandlerContext, PacketHandler};
use crate::packet::clientbound::{AuthenticateData, ClientBoundPacket};
use crate::packet::data::FullUserProfile;
use crate::packet::serverbound::ServerBoundPacket;
use crate::packet::PacketResult;
use crate::player::ResolveResult;
use futures::future::BoxFuture;
use tracing::{debug, warn};

pub struct AuthenticateHandler;

impl AuthenticateHandler {
    pub fn new() -> Self {
        AuthenticateHandler
    }
}

impl PacketHandler for AuthenticateHandler {
    fn handle<'a>(
        &'a mut self,
        ctx: &'a HandlerContext,
        packet: ServerBoundPacket,
    ) -> BoxFuture<'a, HandleOutcome> {
        Box::pin(async move {
            match packet {
                ServerBoundPacket::Authenticate { token, .. } => authenticate(ctx, token).await,
                // 未认证收到任何其他包（含 Ping）→ 关闭（易错点 7）
                _ => {
                    debug!("unauthenticated connection sent non-auth packet, closing");
                    HandleOutcome::Close
                }
            }
        })
    }
}

async fn authenticate(ctx: &HandlerContext, token: String) -> HandleOutcome {
    let server = ctx.server.clone();

    // 1. Phira `/me` 获取用户信息
    let user_info = match server.phira.get_user_info(&token).await {
        Ok(info) => info,
        Err(e) => {
            warn!("auth failed (phira /me): {e}");
            return fail(ctx, server.i18n.default_message("error.authentication_failed")).await;
        }
    };

    // 2. resolve_player（全局唯一注册 + 换绑/会话恢复）
    let (resolve, old_conn): (ResolveResult, Option<ConnectionHandle>) =
        match server.players.resolve_or_resume(user_info.clone(), &ctx.conn).await {
            Ok(v) => v,
            Err(e) => {
                // ResumeFailed / 其他
                return fail(ctx, server.i18n.default_message(&format!("{e}"))).await;
            }
        };

    let player = resolve.player.clone();

    // 3. 踢旧连新（双开/重连场景）
    if let Some(old) = old_conn {
        if old.id() != ctx.conn.id() && !old.is_closed() {
            // 通知旧连接「账号在其他地方登录」（Java sendChat 用 user=-1）
            let msg = server.i18n.message_for(&player, "error.logged_in_elsewhere");
            old.send(ClientBoundPacket::message(crate::packet::message::Message::Chat {
                user: -1,
                content: msg,
            }))
            .await;
            old.mark_duplicate();
            old.close().await;
        }
    }

    // 4. 构造认证响应（带房间快照，客户端据此恢复房间 UI）
    let room_info = player.get_room_info(&player);
    let resp = ClientBoundPacket::Authenticate {
        result: PacketResult::Success(AuthenticateData {
            user_profile: FullUserProfile {
                user_id: player.id(),
                user_name: player.name(),
                monitor: false,
            },
            room_info,
        }),
        trailer: None,
    };
    ctx.send(resp).await;

    // 5. 阶段推进
    match resolve.next_handler {
        Some(h) => HandleOutcome::Switch(h),
        None => {
            HandleOutcome::Switch(Box::new(crate::network::play::PlayHandler::new(player)))
        }
    }
}

async fn fail(ctx: &HandlerContext, message: String) -> HandleOutcome {
    ctx.send(ClientBoundPacket::Authenticate {
        result: PacketResult::Failed(message),
        trailer: None,
    })
    .await;
    HandleOutcome::Close
}
