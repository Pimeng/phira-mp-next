//! AuthenticateHandler（5.1 节）。
//!
//! 与 Java 版一致的扁平化结构：单方法主流程，早返回处理失败分支，
//! 认证数据源经 `resolve_user_info` 收敛（事件注入 → provider → 远程）。

use super::handler::{HandleOutcome, HandlerContext, PacketHandler};
use crate::events;
use crate::packet::clientbound::{AuthenticateData, ClientBoundPacket};
use crate::packet::data::FullUserProfile;
use crate::packet::serverbound::ServerBoundPacket;
use crate::packet::PacketResult;
use crate::phira::UserInfo;
use futures::future::BoxFuture;
use std::sync::Arc;
use tracing::info;

const MAX_TOKEN_BYTES: usize = 256;

pub struct AuthenticateHandler;

impl AuthenticateHandler {
    /// 回复 failed 并关闭（对应 Java 的 `send(failed); close(); return;`）。
    async fn fail_and_close(ctx: &HandlerContext, reason: impl Into<String>) -> HandleOutcome {
        ctx.send(crate::packet::clientbound::authenticate_failed(reason))
            .await;
        ctx.close().await;
        HandleOutcome::Close
    }

    /// 认证数据源收敛（对应 Java `eventUserInfo != null ? ... : PhiraFetcher.GET_USER_INFO`）。
    ///
    /// 返回 `Err(cancel_reason)` 表示 PlayerPreAuthenticateEvent 取消了本次登录。
    async fn resolve_user_info(
        ctx: &HandlerContext,
        token: &str,
    ) -> Result<Arc<UserInfo>, String> {
        // 1. PlayerPreAuthenticateEvent：可取消；可注入 user_info 跳过远程验证
        let ev = events::PlayerPreAuthenticateEvent {
            token: token.to_string(),
            user_info: None,
            cancel_reason: None,
        };
        let ev = ctx
            .server
            .events
            .post_mut(events::PLAYER_PRE_AUTHENTICATE, ev)
            .await;
        if let Some(reason) = ev.cancel_reason {
            return Err(reason);
        }
        if let Some(ui) = ev.user_info {
            return Ok(ui);
        }

        // 2. 全局 provider（自建账号体系）
        if let Some(p) = crate::player::auth_provider() {
            return p(token.to_string()).await.map_err(|e| e.to_string());
        }

        // 3. 远程 PhiraFetcher（对应 Java `PhiraFetcher.GET_USER_INFO`）
        ctx.server
            .phira
            .get_user_info(token)
            .await
            .map_err(|e| e.to_string())
    }
}

impl PacketHandler for AuthenticateHandler {
    fn handle<'a>(
        &'a mut self,
        ctx: &'a HandlerContext,
        packet: ServerBoundPacket,
    ) -> BoxFuture<'a, HandleOutcome> {
        Box::pin(async move {
            let ServerBoundPacket::Authenticate { token, .. } = packet else {
                // 未认证阶段收到其他包 → 直接关闭（对应 Java onUnhandledPacket → close，不发包）
                ctx.close().await;
                return HandleOutcome::Close;
            };
            if token.len() > MAX_TOKEN_BYTES {
                ctx.close().await;
                return HandleOutcome::Close;
            }
            let peer = ctx.conn.peer_addr().await;
            info!(addr = %peer, "received token");

            let info = match Self::resolve_user_info(ctx, &token).await {
                Ok(i) => i,
                Err(reason) => {
                    return Self::fail_and_close(ctx, reason).await;
                }
            };

            // PlayerPreLoginEvent：可取消（白名单/封禁）
            let ev = events::PlayerPreLoginEvent {
                user_info: info.clone(),
                cancel_reason: None,
            };
            let ev = ctx.server.events.post_mut(events::PLAYER_PRE_LOGIN, ev).await;
            if let Some(reason) = ev.cancel_reason {
                return Self::fail_and_close(ctx, reason).await;
            }

            // 注册/恢复（对应 Java resolvePlayer 的默认 LocalPlayer 路径）
            let (result, old_conn) = match ctx
                .server
                .players
                .resolve_or_resume(info.clone(), &ctx.conn)
                .await
            {
                Ok(r) => r,
                Err(key) => {
                    return Self::fail_and_close(ctx, ctx.server.i18n.message(info.language.as_deref(), &key)).await;
                }
            };
            let player = result.player;

            // 顶号：旧连接发「他处登录」并标记 duplicate 后关闭
            if let Some(old) = old_conn.filter(|c| !c.is_closed()) {
                old.send(ClientBoundPacket::message(crate::packet::message::Message::Chat {
                    user: 0,
                    content: ctx.server.i18n.message(player.language().as_deref(), "error.logged_in_elsewhere"),
                }))
                .await;
                old.mark_duplicate();
                old.close().await;
            }

            // 事件：新建 / 换绑
            let (key, ev_created): (&str, bool) = if result.created {
                (events::PLAYER_CREATE, true)
            } else {
                (events::PLAYER_CONNECTION_BIND, false)
            };
            if ev_created {
                ctx.server.events.post(key, events::PlayerCreateEvent { player: player.clone() }).await;
            } else {
                ctx.server.events.post(key, events::PlayerConnectionBindEvent { player: player.clone() }).await;
            }

            // 回复 Authenticate 包（恢复时附带 RoomInfo）
            let is_monitor = result
                .suspended
                .as_ref()
                .map(|s| s.room.is_monitor_user(player.id()))
                .unwrap_or(false);
            let room_info = result
                .suspended
                .as_ref()
                .map(|s| s.room.build_room_info(player.as_ref()));
            ctx.send(ClientBoundPacket::Authenticate {
                result: PacketResult::Success(AuthenticateData {
                    user_profile: FullUserProfile {
                        user_id: player.id(),
                        user_name: player.name(),
                        monitor: is_monitor,
                    },
                    room_info,
                }),
                trailer: None,
            })
            .await;

            info!(user_id = info.id, name = %info.name, "logged in");
            ctx.server.events.post(events::PLAYER_POST_LOGIN, events::PlayerPostLoginEvent { player: player.clone() }).await;

            // Switch：恢复挂起 → RoomHandler；否则 → PlayHandler
            let fallback: Box<dyn PacketHandler> = Box::new(super::play_handler::PlayHandler::new(player.clone()));
            let next: Box<dyn PacketHandler> = match result.suspended {
                Some(s) => Box::new(super::room_handler::RoomHandler::new(player, s.room, fallback)),
                None => fallback,
            };
            HandleOutcome::Switch(next)
        })
    }
}
