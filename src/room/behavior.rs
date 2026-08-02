//! 房间操作层（对应 Java `Room.Operation` 接口的默认实现 `LocalOperation`）。
//!
//! 全部操作遵循同一纪律：**锁内只做状态决策并产出「广播计划」，锁外由
//! [`deliver`] 发送**。广播载荷是预编码的共享帧（`SharedFrame = Arc<Bytes>`），
//! 同一份字节零拷贝地发给所有目标连接。

use super::local::Inner;
use super::state::RoomState;
use super::{CommitGameOutcome, GameError, GameResult, RecordingData};
use crate::packet::clientbound::{ClientBoundPacket, SharedFrame, encode_shared};
use crate::packet::data::{JudgeEvent, TouchFrame};
use crate::packet::message::Message;
use crate::packet::state::GameState;
use crate::player::Player;
use std::collections::HashSet;
use std::sync::{Arc, Mutex};

/// 广播计划：(目标玩家, 预编码共享帧)。
/// 目标为 `Arc<dyn Player>`（不限于 LocalPlayer）——自定义玩家经
/// [`Player::send_frame`] 覆写即可接收广播。
pub type Broadcast = Vec<(Arc<dyn Player>, SharedFrame)>;

/// 发送广播计划（锁外执行；共享帧零拷贝，跳过已离线目标）。
pub async fn deliver(plan: Broadcast) {
    for (target, frame) in plan {
        target.send_frame(frame).await;
    }
}

/// 给全体成员（players + monitors）编码同一份包，产出广播计划。
pub(crate) fn broadcast_all(g: &Inner, packet: ClientBoundPacket) -> Broadcast {
    let frame = encode_shared(&packet);
    g.players
        .iter()
        .chain(g.monitors.iter())
        .map(|p| (p.clone(), frame.clone()))
        .collect()
}

/// lockRoom：忽略客户端值，按切换处理（易错点 1）。返回广播计划。
pub(crate) fn toggle_lock(inner: &Mutex<Inner>) -> GameResult<Broadcast> {
    let mut g = inner.lock().unwrap();
    g.setting.locked = !g.setting.locked;
    let locked = g.setting.locked;
    Ok(broadcast_all(
        &g,
        ClientBoundPacket::message(Message::LockRoom { lock: locked }),
    ))
}

pub(crate) fn toggle_cycle(inner: &Mutex<Inner>) -> GameResult<Broadcast> {
    let mut g = inner.lock().unwrap();
    g.setting.cycle = !g.setting.cycle;
    let cycle = g.setting.cycle;
    Ok(broadcast_all(
        &g,
        ClientBoundPacket::message(Message::CycleRoom { cycle }),
    ))
}

/// chat：返回广播计划。
pub(crate) fn chat(inner: &Mutex<Inner>, user_id: i32, content: String) -> GameResult<Broadcast> {
    let g = inner.lock().unwrap();
    if !g.setting.chat {
        return Err(GameError("ERROR_CHAT_NOT_ENABLED"));
    }
    Ok(broadcast_all(
        &g,
        ClientBoundPacket::message(Message::Chat {
            user: user_id,
            content,
        }),
    ))
}

/// selectChart 校验（锁内，await HTTP 前调用）。
pub(crate) fn validate_select_chart(inner: &Mutex<Inner>) -> GameResult<()> {
    let g = inner.lock().unwrap();
    if !matches!(g.state, RoomState::SelectChart { .. }) {
        return Err(GameError("ERROR_INVALID_STATE"));
    }
    Ok(())
}

/// selectChart 提交（HTTP 完成后锁内）。
pub(crate) fn commit_select_chart(
    inner: &Mutex<Inner>,
    user_id: i32,
    chart_id: i32,
    chart_name: String,
) -> GameResult<Broadcast> {
    let mut g = inner.lock().unwrap();
    let RoomState::SelectChart {
        chart_id: cid,
        chart_name: cn,
    } = &mut g.state
    else {
        return Err(GameError("ERROR_INVALID_STATE"));
    };
    *cid = Some(chart_id);
    *cn = Some(chart_name.clone());

    let mut plan = broadcast_all(
        &g,
        ClientBoundPacket::message(Message::SelectChart {
            user: user_id,
            name: chart_name,
            id: chart_id,
        }),
    );
    plan.extend(broadcast_all(
        &g,
        ClientBoundPacket::change_state(GameState::SelectChart {
            chart_id: Some(chart_id),
        }),
    ));
    Ok(plan)
}

/// requireStart（6.3 节）：单人直接开局；多人进 WaitForReady（发起者自动 ready）。
pub(crate) fn require_start(inner: &Mutex<Inner>, user_id: i32) -> GameResult<Broadcast> {
    let mut g = inner.lock().unwrap();
    let total = g.players.len() + g.monitors.len();
    let (chart_id, chart_name) = match &g.state {
        RoomState::SelectChart {
            chart_id,
            chart_name,
        } => (*chart_id, chart_name.clone()),
        _ => return Err(GameError("ERROR_INVALID_STATE")),
    };

    let mut plan = Broadcast::new();
    if total == 1 {
        // 单人直接开局。Java 仅 enterState(Playing) 不发 StartPlaying；
        // 但客户端以 StartPlaying 作为「开始演奏」的明确信号，单人也补发以统一体验。
        g.state = RoomState::Playing {
            chart_id,
            chart_name,
            done: HashSet::new(),
            touch_frames: Default::default(),
            judge_events: Default::default(),
        };
        plan.extend(broadcast_all(
            &g,
            ClientBoundPacket::change_state(GameState::Playing),
        ));
        plan.extend(broadcast_all(
            &g,
            ClientBoundPacket::message(Message::StartPlaying),
        ));
    } else {
        // 多人：updateGameState(WaitForReady) → enterState(WaitForReady) + gameRequireStart(=GameStart)。
        g.state = RoomState::WaitForReady {
            chart_id,
            chart_name,
            ready: {
                let mut s = HashSet::new();
                s.insert(user_id); // 发起者自动 ready
                s
            },
        };
        plan.extend(broadcast_all(
            &g,
            ClientBoundPacket::change_state(GameState::WaitForReady),
        ));
        plan.extend(broadcast_all(
            &g,
            ClientBoundPacket::message(Message::GameStart { user: user_id }),
        ));
    }
    Ok(plan)
}

/// ready（6.4 节）。返回 (广播计划, 是否全员就绪已开局)。
pub(crate) fn ready(inner: &Mutex<Inner>, user_id: i32) -> GameResult<(Broadcast, bool)> {
    let mut g = inner.lock().unwrap();
    let online_ids: HashSet<i32> = g
        .players
        .iter()
        .chain(g.monitors.iter())
        .filter(|p| p.is_online())
        .map(|p| p.id())
        .collect();

    let (all_ready, cid, cn) = {
        let RoomState::WaitForReady {
            ready,
            chart_id,
            chart_name,
        } = &mut g.state
        else {
            return Err(GameError("ERROR_INVALID_STATE"));
        };
        ready.insert(user_id);
        let all_ready = online_ids.iter().all(|id| ready.contains(id)) && !online_ids.is_empty();
        (all_ready, *chart_id, chart_name.clone())
    };

    let mut plan = broadcast_all(
        &g,
        ClientBoundPacket::message(Message::Ready { user: user_id }),
    );
    if all_ready {
        g.state = RoomState::Playing {
            chart_id: cid,
            chart_name: cn,
            done: HashSet::new(),
            touch_frames: Default::default(),
            judge_events: Default::default(),
        };
        plan.extend(broadcast_all(
            &g,
            ClientBoundPacket::change_state(GameState::Playing),
        ));
        plan.extend(broadcast_all(
            &g,
            ClientBoundPacket::message(Message::StartPlaying),
        ));
        return Ok((plan, true));
    }
    Ok((plan, false))
}

/// cancelReady（6.4 节）。
pub(crate) fn cancel_ready(inner: &Mutex<Inner>, user_id: i32) -> GameResult<Broadcast> {
    let mut g = inner.lock().unwrap();
    let RoomState::WaitForReady { ready, .. } = &mut g.state else {
        return Err(GameError("ERROR_INVALID_STATE"));
    };
    ready.remove(&user_id);
    Ok(broadcast_all(
        &g,
        ClientBoundPacket::message(Message::CancelReady { user: user_id }),
    ))
}

/// played 提交（HTTP 完成后锁内；幂等）。
pub(crate) fn commit_played(
    inner: &Mutex<Inner>,
    user_id: i32,
    score: i32,
    accuracy: f32,
    full_combo: bool,
) -> GameResult<CommitGameOutcome> {
    let mut g = inner.lock().unwrap();
    let RoomState::Playing { done, .. } = &mut g.state else {
        return Err(GameError("ERROR_INVALID_STATE"));
    };
    if !done.insert(user_id) {
        return Ok(CommitGameOutcome {
            broadcasts: vec![],
            game_ended: false,
            recording: None,
        });
    }

    // 取录制数据（若该玩家有触摸/判定数据）
    let recording = {
        let RoomState::Playing {
            touch_frames,
            judge_events,
            chart_id,
            chart_name,
            ..
        } = &mut g.state
        else {
            unreachable!()
        };
        let tf = touch_frames.remove(&user_id).unwrap_or_default();
        let je = judge_events.remove(&user_id).unwrap_or_default();
        if !tf.is_empty() || !je.is_empty() {
            Some(RecordingData {
                chart_id: *chart_id,
                chart_name: chart_name.clone(),
                touch_frames: tf,
                judge_events: je,
            })
        } else {
            None
        }
    };

    let mut plan = broadcast_all(
        &g,
        ClientBoundPacket::message(Message::Played {
            user: user_id,
            score,
            accuracy,
            full_combo,
        }),
    );
    let ended = check_game_end_inner(&mut g, &mut plan);
    Ok(CommitGameOutcome {
        broadcasts: plan,
        game_ended: ended,
        recording,
    })
}

/// abort（幂等）。
pub(crate) fn commit_abort(inner: &Mutex<Inner>, user_id: i32) -> GameResult<CommitGameOutcome> {
    let mut g = inner.lock().unwrap();
    let RoomState::Playing { done, .. } = &mut g.state else {
        return Err(GameError("ERROR_INVALID_STATE"));
    };
    if !done.insert(user_id) {
        return Ok(CommitGameOutcome {
            broadcasts: vec![],
            game_ended: false,
            recording: None,
        });
    }
    let mut plan = broadcast_all(
        &g,
        ClientBoundPacket::message(Message::Abort { user: user_id }),
    );
    let ended = check_game_end_inner(&mut g, &mut plan);
    Ok(CommitGameOutcome {
        broadcasts: plan,
        game_ended: ended,
        recording: None,
    })
}

/// 对局结束判定（6.5 节）：所有在线 players（不含 monitor）完成；全员掉线也算结束。
fn check_game_end_inner(g: &mut Inner, plan: &mut Broadcast) -> bool {
    let RoomState::Playing {
        done,
        chart_id,
        chart_name,
        ..
    } = &g.state
    else {
        return false;
    };
    let online_players: Vec<i32> = g
        .players
        .iter()
        .filter(|p| p.is_online())
        .map(|p| p.id())
        .collect();
    let all_done = !online_players.is_empty() && online_players.iter().all(|id| done.contains(id));
    let no_online = online_players.is_empty();
    if !all_done && !no_online {
        return false;
    }

    let (cid, cn) = (*chart_id, chart_name.clone());
    g.state = RoomState::SelectChart {
        chart_id: cid,
        chart_name: cn,
    };
    plan.extend(broadcast_all(
        g,
        ClientBoundPacket::change_state(GameState::SelectChart { chart_id: cid }),
    ));
    plan.extend(broadcast_all(
        g,
        ClientBoundPacket::message(Message::GameEnd),
    ));

    if g.setting.cycle {
        plan.extend(transfer_host_plan(g));
    }
    true
}

/// touch/judge 收集 + 转发 Monitor（6.5 节）。
///
/// 对齐 Java `LocalOperation.touchSend`：
/// - 收集仅在 Playing 状态（`stateRef.get().touchSend`）。
/// - **转发给 monitor 是无条件的**（`broadcastToMonitors`），任何状态都转。
pub(crate) fn touch_send(inner: &Mutex<Inner>, user_id: i32, frames: Vec<TouchFrame>) -> Broadcast {
    let mut g = inner.lock().unwrap();
    // 1. 收集（仅 Playing）
    if let RoomState::Playing { touch_frames, .. } = &mut g.state {
        touch_frames
            .entry(user_id)
            .or_default()
            .extend(frames.iter().cloned());
    }
    // 2. 无条件转发 monitor
    let frame = encode_shared(&ClientBoundPacket::Touches {
        from_player_id: user_id,
        frames,
        trailer: None,
    });
    g.monitors
        .iter()
        .map(|m| (m.clone(), frame.clone()))
        .collect()
}

pub(crate) fn judge_send(inner: &Mutex<Inner>, user_id: i32, judges: Vec<JudgeEvent>) -> Broadcast {
    let mut g = inner.lock().unwrap();
    // 1. 收集（仅 Playing）
    if let RoomState::Playing { judge_events, .. } = &mut g.state {
        judge_events
            .entry(user_id)
            .or_default()
            .extend(judges.iter().copied());
    }
    // 2. 无条件转发 monitor
    let frame = encode_shared(&ClientBoundPacket::Judges {
        from_player_id: user_id,
        judges,
        trailer: None,
    });
    g.monitors
        .iter()
        .map(|m| (m.clone(), frame.clone()))
        .collect()
}

/// 挂起前清理现场：WaitForReady → cancelReady；Playing → abort。
pub(crate) fn cleanup_for_suspend(inner: &Mutex<Inner>, user_id: i32) -> Broadcast {
    let mut g = inner.lock().unwrap();
    let mut plan = Broadcast::new();
    match &mut g.state {
        RoomState::WaitForReady { ready, .. } => {
            if ready.remove(&user_id) {
                plan.extend(broadcast_all(
                    &g,
                    ClientBoundPacket::message(Message::CancelReady { user: user_id }),
                ));
            }
        }
        RoomState::Playing { done, .. } => {
            if done.insert(user_id) {
                plan.extend(broadcast_all(
                    &g,
                    ClientBoundPacket::message(Message::Abort { user: user_id }),
                ));
                check_game_end_inner(&mut g, &mut plan);
            }
        }
        _ => {}
    }
    plan
}

/// 房主转移（6.6 节）：优先 `next_host`（控制台 nexthost 指定）；
/// 否则按 userId 升序取下一个；无则最小。返回广播计划。
pub(crate) fn transfer_host_plan(g: &mut Inner) -> Broadcast {
    let Some(old_host) = g.host.clone() else {
        return vec![];
    };
    if g.players.is_empty() {
        g.host = None;
        return vec![];
    }
    // nexthost 指定：仅当目标在房间且不是当前房主时生效；
    // 指定当前房主视为「本轮不转移」（next_host 已 take 失效）。
    match g.next_host.take() {
        Some(id) if id == old_host.id() => return vec![],
        Some(id) => {
            if let Some(p) = g.players.iter().find(|p| p.id() == id).cloned() {
                g.host = Some(p.clone());
                return build_host_change_plan(g, Some(old_host), p);
            }
            // 目标已不在房间 → 回退默认轮换
        }
        None => {}
    }
    let mut sorted: Vec<Arc<dyn Player>> = g.players.clone();
    sorted.sort_by_key(|p| p.id());
    let new_host = sorted
        .iter()
        .find(|p| p.id() > old_host.id())
        .cloned()
        .unwrap_or_else(|| sorted[0].clone());
    g.host = Some(new_host.clone());
    build_host_change_plan(g, Some(old_host), new_host)
}

/// 房主变更广播：旧房主 ChangeHost(false)、新房主 ChangeHost(true)，
/// 其余成员收 NewHost。
fn build_host_change_plan(
    g: &Inner,
    old_host: Option<Arc<dyn Player>>,
    new_host: Arc<dyn Player>,
) -> Broadcast {
    let mut plan = Broadcast::new();
    if let Some(old) = old_host {
        plan.push((
            old.clone(),
            encode_shared(&ClientBoundPacket::change_host(false)),
        ));
    }
    plan.push((
        new_host.clone(),
        encode_shared(&ClientBoundPacket::change_host(true)),
    ));
    let new_id = new_host.id();
    let msg = encode_shared(&ClientBoundPacket::message(Message::NewHost {
        user: new_id,
    }));
    for target in g.players.iter().chain(g.monitors.iter()) {
        if target.id() != new_id {
            plan.push((target.clone(), msg.clone()));
        }
    }
    plan
}

// ---------------- 管理员操作（控制台命令；不要求房主） ----------------

/// 设置房间最大人数（控制台 `maxusers`）。count 至少为 1。
pub(crate) fn set_max_player(inner: &Mutex<Inner>, count: usize) -> GameResult<()> {
    if count == 0 {
        return Err(GameError("ERROR_INVALID_ARGUMENT"));
    }
    inner.lock().unwrap().setting.max_player = count;
    Ok(())
}

/// 强制锁定/解锁（控制台 `lock`）。
pub(crate) fn set_locked(inner: &Mutex<Inner>, locked: bool) -> GameResult<Broadcast> {
    let mut g = inner.lock().unwrap();
    g.setting.locked = locked;
    Ok(broadcast_all(
        &g,
        ClientBoundPacket::message(Message::LockRoom { lock: locked }),
    ))
}

/// 开关循环模式（控制台 `cycle`）。
pub(crate) fn set_cycle(inner: &Mutex<Inner>, cycle: bool) -> GameResult<Broadcast> {
    let mut g = inner.lock().unwrap();
    g.setting.cycle = cycle;
    Ok(broadcast_all(
        &g,
        ClientBoundPacket::message(Message::CycleRoom { cycle }),
    ))
}

/// 指定下一轮房主（控制台 `nexthost`）。目标玩家必须在房间内；
/// 仅循环模式对局结束时（transfer_host_plan）生效。
pub(crate) fn set_next_host(inner: &Mutex<Inner>, user_id: i32) -> GameResult<()> {
    let mut g = inner.lock().unwrap();
    if !g.players.iter().any(|p| p.id() == user_id) {
        return Err(GameError("ERROR_PLAYER_NOT_IN_ROOM"));
    }
    g.next_host = Some(user_id);
    Ok(())
}

/// 立即转移房主（控制台 `sethost`）。
pub(crate) fn transfer_host_to(inner: &Mutex<Inner>, user_id: i32) -> GameResult<Broadcast> {
    let mut g = inner.lock().unwrap();
    if !g.setting.host {
        return Err(GameError("ERROR_HOST_DISABLED"));
    }
    let Some(target) = g.players.iter().find(|p| p.id() == user_id).cloned() else {
        return Err(GameError("ERROR_PLAYER_NOT_IN_ROOM"));
    };
    if g.host.as_ref().map(|h| h.id()) == Some(user_id) {
        return Ok(vec![]);
    }
    let old_host = g.host.replace(target.clone());
    Ok(build_host_change_plan(&g, old_host, target))
}

/// 管理员向房间广播消息（控制台 `roomsay`；绕过 chat 开关，user=0 表示系统）。
pub(crate) fn admin_chat(inner: &Mutex<Inner>, content: String) -> GameResult<Broadcast> {
    let g = inner.lock().unwrap();
    Ok(broadcast_all(
        &g,
        ClientBoundPacket::message(Message::Chat { user: 0, content }),
    ))
}
