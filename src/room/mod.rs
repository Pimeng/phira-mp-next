//! 房间与状态机（第 6、7 节）。

pub mod state;

use crate::packet::clientbound::{ClientBoundPacket, JoinRoomData};
use crate::packet::data::{FullUserProfile, JudgeEvent, RoomInfo, TouchFrame};
use crate::packet::message::Message;
use crate::packet::state::GameState;
use crate::packet::PacketResult;
use crate::player::Player;
use state::RoomState;
use std::collections::HashSet;
use std::sync::{Arc, Mutex, Weak};

/// 业务操作错误（对应 GameOperationException，message 为 i18n key）。
#[derive(Debug)]
pub struct GameError(pub &'static str);

impl std::fmt::Display for GameError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

pub type GameResult<T> = Result<T, GameError>;

pub const DEFAULT_MAX_PLAYER: usize = 8;

#[derive(Debug, Clone)]
pub struct RoomSetting {
    pub auto_destroy: bool,
    pub host: bool,
    pub max_player: usize,
    pub locked: bool,
    pub cycle: bool,
    pub live: bool,
    pub chat: bool,
}

impl Default for RoomSetting {
    fn default() -> Self {
        Self {
            auto_destroy: true,
            host: true,
            max_player: DEFAULT_MAX_PLAYER,
            locked: false,
            cycle: false,
            live: false,
            chat: true,
        }
    }
}

/// 锁内状态（锁内只做状态修改，不做网络 I/O / await）。
struct Inner {
    host: Option<Arc<Player>>,
    players: Vec<Arc<Player>>,
    monitors: Vec<Arc<Player>>,
    state: RoomState,
    setting: RoomSetting,
    destroyed: bool,
}

/// 加入结果（锁外广播用）。
#[derive(Debug)]
pub enum JoinOutcome {
    AlreadyIn,
    FirstPlayer,
    Joined { is_monitor: bool },
}

/// played 校验结果（避免 None 二义性）。
pub enum PlayedCheck {
    CanPlay { chart_id: Option<i32>, chart_name: Option<String> },
    AlreadyDone,
}

pub struct Room {
    pub id: String,
    inner: Mutex<Inner>,
    /// 销毁回调（从全局注册表移除）。
    on_destroy: Mutex<Option<Box<dyn Fn() + Send + Sync>>>,
}

impl Room {
    pub fn new(
        id: impl Into<String>,
        setting: RoomSetting,
        on_destroy: impl Fn() + Send + Sync + 'static,
    ) -> Arc<Self> {
        Arc::new(Self {
            id: id.into(),
            inner: Mutex::new(Inner {
                host: None,
                players: Vec::new(),
                monitors: Vec::new(),
                state: RoomState::SelectChart {
                    chart_id: None,
                    chart_name: None,
                },
                setting,
                destroyed: false,
            }),
            on_destroy: Mutex::new(Some(Box::new(on_destroy))),
        })
    }

    // ---------- 查询（锁内，快速返回） ----------

    pub fn contains_member(&self, user_id: i32) -> bool {
        let g = self.inner.lock().unwrap();
        g.players.iter().any(|p| p.id() == user_id) || g.monitors.iter().any(|p| p.id() == user_id)
    }

    pub fn contains_monitor(&self, user_id: i32) -> bool {
        self.inner.lock().unwrap().monitors.iter().any(|p| p.id() == user_id)
    }

    pub fn is_host(&self, user_id: i32) -> bool {
        let g = self.inner.lock().unwrap();
        g.setting.host && g.host.as_ref().map(|h| h.id()) == Some(user_id)
    }

    pub fn is_destroyed(&self) -> bool {
        self.inner.lock().unwrap().destroyed
    }

    pub fn game_state_protocol(&self) -> GameState {
        self.inner.lock().unwrap().state.to_protocol()
    }

    /// viewer 视角 RoomInfo 快照（3.5 / 7.2 节）。
    pub fn build_room_info(&self, viewer: &Player) -> RoomInfo {
        let g = self.inner.lock().unwrap();
        let users: Vec<FullUserProfile> = g
            .players
            .iter()
            .map(|p| FullUserProfile {
                user_id: p.id(),
                user_name: p.name(),
                monitor: false,
            })
            .chain(g.monitors.iter().map(|p| FullUserProfile {
                user_id: p.id(),
                user_name: p.name(),
                monitor: true,
            }))
            .collect();
        RoomInfo {
            room_id: self.id.clone(),
            state: g.state.to_protocol(),
            live: g.setting.live,
            locked: g.setting.locked,
            cycle: g.setting.cycle,
            is_host: g.setting.host && g.host.as_ref().map(|h| h.id()) == Some(viewer.id()),
            // WaitForReady 状态下对 viewer 恒 true（易错点 3）
            is_ready: matches!(g.state, RoomState::WaitForReady { .. }),
            users,
        }
    }

    /// JoinRoom 快照（3.3 节 0x09 载荷）。
    pub fn join_room_data(&self) -> JoinRoomData {
        let g = self.inner.lock().unwrap();
        let users: Vec<FullUserProfile> = g
            .players
            .iter()
            .map(|p| FullUserProfile {
                user_id: p.id(),
                user_name: p.name(),
                monitor: false,
            })
            .chain(g.monitors.iter().map(|p| FullUserProfile {
                user_id: p.id(),
                user_name: p.name(),
                monitor: true,
            }))
            .collect();
        JoinRoomData {
            game_state: g.state.to_protocol(),
            users,
            live: g.setting.live,
        }
    }

    // ---------- 成员管理（锁内改状态，返回广播计划） ----------

    /// join：返回 (outcome, 广播包列表)。
    pub fn join(&self, player: Arc<Player>, is_monitor: bool) -> GameResult<(JoinOutcome, Vec<(Arc<Player>, ClientBoundPacket)>)> {
        let mut g = self.inner.lock().unwrap();
        if g.destroyed {
            return Err(GameError("error.room_not_found"));
        }
        // 重复加入 → 直接返回
        if g.players.iter().any(|p| p.id() == player.id()) || g.monitors.iter().any(|p| p.id() == player.id()) {
            return Ok((JoinOutcome::AlreadyIn, vec![]));
        }
        if !is_monitor {
            if g.players.len() >= g.setting.max_player {
                return Err(GameError("error.room_full"));
            }
            if g.setting.locked && !g.players.is_empty() {
                return Err(GameError("error.room_locked"));
            }
        }

        let mut broadcasts: Vec<(Arc<Player>, ClientBoundPacket)> = Vec::new();
        let outcome = if is_monitor {
            // Monitor：广播加入（OnJoinRoom + JoinRoom 消息），排除自己
            let profile = FullUserProfile {
                user_id: player.id(),
                user_name: player.name(),
                monitor: true,
            };
            for target in g.players.iter().chain(g.monitors.iter()) {
                broadcasts.push((target.clone(), ClientBoundPacket::on_join_room(profile.clone())));
                broadcasts.push((
                    target.clone(),
                    ClientBoundPacket::message(Message::JoinRoom {
                        user: player.id(),
                        name: player.name(),
                    }),
                ));
            }
            g.monitors.push(player.clone());
            g.state.handle_join(player.id());
            JoinOutcome::Joined { is_monitor: true }
        } else if g.players.is_empty() && g.setting.host {
            // 首个玩家自动成为房主（不广播加入）
            g.host = Some(player.clone());
            g.players.push(player.clone());
            g.state.handle_join(player.id());
            JoinOutcome::FirstPlayer
        } else {
            let profile = FullUserProfile {
                user_id: player.id(),
                user_name: player.name(),
                monitor: false,
            };
            for target in g.players.iter().chain(g.monitors.iter()) {
                broadcasts.push((target.clone(), ClientBoundPacket::on_join_room(profile.clone())));
                broadcasts.push((
                    target.clone(),
                    ClientBoundPacket::message(Message::JoinRoom {
                        user: player.id(),
                        name: player.name(),
                    }),
                ));
            }
            g.players.push(player.clone());
            g.state.handle_join(player.id());
            JoinOutcome::Joined { is_monitor: false }
        };
        Ok((outcome, broadcasts))
    }

    /// leave：返回 (是否离开成功, 需要广播的包, 需要 ChangeHost 通知的玩家, 是否销毁)。
    pub fn leave(
        &self,
        player_id: i32,
    ) -> (bool, Vec<(Arc<Player>, ClientBoundPacket)>, bool) {
        let mut g = self.inner.lock().unwrap();
        let player_name = g
            .players
            .iter()
            .chain(g.monitors.iter())
            .find(|p| p.id() == player_id)
            .map(|p| p.name());
        let Some(name) = player_name else {
            return (false, vec![], false);
        };

        let was_host = g.host.as_ref().map(|h| h.id()) == Some(player_id);
        g.players.retain(|p| p.id() != player_id);
        g.monitors.retain(|p| p.id() != player_id);

        let mut broadcasts: Vec<(Arc<Player>, ClientBoundPacket)> = Vec::new();

        // 空房间且 autoDestroy → 销毁
        if g.players.is_empty() && g.monitors.is_empty() && g.setting.auto_destroy {
            g.destroyed = true;
            if let Some(cb) = self.on_destroy.lock().unwrap().take() {
                cb();
            }
            return (true, broadcasts, true);
        }

        // 房主离开 → 转移
        if was_host {
            let new_host = transfer_host_inner(&mut g);
            match new_host {
                Some((old, new)) => {
                    broadcasts.push((old, ClientBoundPacket::change_host(false)));
                    broadcasts.push((new.clone(), ClientBoundPacket::change_host(true)));
                    // NewHost 消息广播给其他人
                    for target in g.players.iter().chain(g.monitors.iter()) {
                        if target.id() != new.id() {
                            broadcasts.push((
                                target.clone(),
                                ClientBoundPacket::message(Message::NewHost { user: new.id() }),
                            ));
                        }
                    }
                }
                None => {
                    g.host = None;
                }
            }
        }

        // LeaveRoom 消息广播（离开者已不在集合，天然排除）
        for target in g.players.iter().chain(g.monitors.iter()) {
            broadcasts.push((
                target.clone(),
                ClientBoundPacket::message(Message::LeaveRoom {
                    user: player_id,
                    name: name.clone(),
                }),
            ));
        }
        g.state.handle_leave(player_id);
        (true, broadcasts, false)
    }

    // ---------- 操作鉴权 + 状态校验（锁内） / 提交（锁内） ----------

    pub fn validate_host(&self, user_id: i32) -> GameResult<()> {
        if self.is_host(user_id) {
            Ok(())
        } else {
            Err(GameError("error.permission_denied"))
        }
    }

    /// lockRoom：忽略客户端值，按切换处理（易错点 1）。返回新状态 + 广播。
    pub fn toggle_lock(&self, user_id: i32) -> GameResult<Vec<(Arc<Player>, ClientBoundPacket)>> {
        self.validate_host(user_id)?;
        let mut g = self.inner.lock().unwrap();
        g.setting.locked = !g.setting.locked;
        let locked = g.setting.locked;
        Ok(broadcast_all(&g, ClientBoundPacket::message(Message::LockRoom { lock: locked })))
    }

    pub fn toggle_cycle(&self, user_id: i32) -> GameResult<Vec<(Arc<Player>, ClientBoundPacket)>> {
        self.validate_host(user_id)?;
        let mut g = self.inner.lock().unwrap();
        g.setting.cycle = !g.setting.cycle;
        let cycle = g.setting.cycle;
        Ok(broadcast_all(&g, ClientBoundPacket::message(Message::CycleRoom { cycle })))
    }

    /// chat：返回广播计划。
    pub fn chat(&self, user_id: i32, content: String) -> GameResult<Vec<(Arc<Player>, ClientBoundPacket)>> {
        let g = self.inner.lock().unwrap();
        if !g.setting.chat {
            return Err(GameError("error.chat_not_enabled"));
        }
        Ok(broadcast_all(
            &g,
            ClientBoundPacket::message(Message::Chat { user: user_id, content }),
        ))
    }

    /// selectChart 校验（锁内，await HTTP 前调用）。
    pub fn validate_select_chart(&self, user_id: i32) -> GameResult<()> {
        self.validate_host(user_id)?;
        let g = self.inner.lock().unwrap();
        if !matches!(g.state, RoomState::SelectChart { .. }) {
            return Err(GameError("error.invalid_state"));
        }
        Ok(())
    }

    /// selectChart 提交（HTTP 完成后锁内）。
    pub fn commit_select_chart(
        &self,
        user_id: i32,
        chart_id: i32,
        chart_name: String,
        user_name: String,
    ) -> GameResult<Vec<(Arc<Player>, ClientBoundPacket)>> {
        self.validate_host(user_id)?;
        let mut g = self.inner.lock().unwrap();
        let RoomState::SelectChart { chart_id: cid, chart_name: cn } = &mut g.state else {
            return Err(GameError("error.invalid_state"));
        };
        *cid = Some(chart_id);
        *cn = Some(chart_name.clone());
        let mut out = broadcast_all(
            &g,
            ClientBoundPacket::message(Message::SelectChart {
                user: user_id,
                name: chart_name,
                id: chart_id,
            }),
        );
        out.extend(broadcast_all(
            &g,
            ClientBoundPacket::change_state(GameState::SelectChart {
                chart_id: Some(chart_id),
            }),
        ));
        let _ = user_name;
        Ok(out)
    }

    /// requireStart（6.3 节）。
    pub fn require_start(&self, user_id: i32) -> GameResult<Vec<(Arc<Player>, ClientBoundPacket)>> {
        self.validate_host(user_id)?;
        let mut g = self.inner.lock().unwrap();
        let total = g.players.len() + g.monitors.len();
        let (chart_id, chart_name) = match &g.state {
            RoomState::SelectChart { chart_id, chart_name } => (*chart_id, chart_name.clone()),
            _ => return Err(GameError("error.invalid_state")),
        };

        let mut out = Vec::new();
        if total == 1 {
            // 单人直接开局
            g.state = RoomState::Playing {
                chart_id,
                chart_name,
                done: HashSet::new(),
                touch_frames: Default::default(),
                judge_events: Default::default(),
            };
            out.extend(broadcast_all(&g, ClientBoundPacket::change_state(GameState::Playing)));
            out.extend(broadcast_all(&g, ClientBoundPacket::message(Message::StartPlaying)));
        } else {
            g.state = RoomState::WaitForReady {
                chart_id,
                chart_name,
                ready: {
                    let mut s = HashSet::new();
                    s.insert(user_id); // 发起者自动 ready
                    s
                },
            };
            out.extend(broadcast_all(
                &g,
                ClientBoundPacket::change_state(GameState::WaitForReady),
            ));
            out.extend(broadcast_all(
                &g,
                ClientBoundPacket::message(Message::GameStart { user: user_id }),
            ));
        }
        Ok(out)
    }

    /// ready（6.4 节）。返回 (广播包, 是否全员就绪已开局)。
    pub fn ready(&self, user_id: i32) -> GameResult<(Vec<(Arc<Player>, ClientBoundPacket)>, bool)> {
        let mut g = self.inner.lock().unwrap();
        let online_ids: HashSet<i32> = g
            .players
            .iter()
            .chain(g.monitors.iter())
            .filter(|p| p.is_online())
            .map(|p| p.id())
            .collect();

        // 记录 ready 并判定全员就绪（在窄作用域内完成，随后释放对 state 的可变借用）
        let (all_ready, cid, cn) = {
            let RoomState::WaitForReady { ready, chart_id, chart_name } = &mut g.state else {
                return Err(GameError("error.invalid_state"));
            };
            ready.insert(user_id);
            let all_ready =
                online_ids.iter().all(|id| ready.contains(id)) && !online_ids.is_empty();
            (all_ready, *chart_id, chart_name.clone())
        };

        let mut out = broadcast_all(&g, ClientBoundPacket::message(Message::Ready { user: user_id }));

        // 所有在线成员（含 Monitor）已 ready → 开局
        if all_ready {
            g.state = RoomState::Playing {
                chart_id: cid,
                chart_name: cn,
                done: HashSet::new(),
                touch_frames: Default::default(),
                judge_events: Default::default(),
            };
            out.extend(broadcast_all(&g, ClientBoundPacket::change_state(GameState::Playing)));
            out.extend(broadcast_all(&g, ClientBoundPacket::message(Message::StartPlaying)));
            return Ok((out, true));
        }
        Ok((out, false))
    }

    /// cancelReady（6.4 节）。
    pub fn cancel_ready(&self, user_id: i32) -> GameResult<Vec<(Arc<Player>, ClientBoundPacket)>> {
        let mut g = self.inner.lock().unwrap();
        let RoomState::WaitForReady { ready, .. } = &mut g.state else {
            return Err(GameError("error.invalid_state"));
        };
        ready.remove(&user_id);
        Ok(broadcast_all(
            &g,
            ClientBoundPacket::message(Message::CancelReady { user: user_id }),
        ))
    }

    /// played 校验（幂等；避免 None 二义性）。
    pub fn check_played(&self, user_id: i32) -> GameResult<PlayedCheck> {
        let g = self.inner.lock().unwrap();
        match &g.state {
            RoomState::Playing { done, chart_id, chart_name, .. } => {
                if done.contains(&user_id) {
                    Ok(PlayedCheck::AlreadyDone)
                } else {
                    Ok(PlayedCheck::CanPlay {
                        chart_id: *chart_id,
                        chart_name: chart_name.clone(),
                    })
                }
            }
            _ => Err(GameError("error.invalid_state")),
        }
    }

    /// played 提交（HTTP 完成后锁内）。
    /// 返回 (广播包, 是否对局结束, 若结束且 cycle 需要的新房主通知)。
    pub fn commit_played(
        &self,
        user_id: i32,
        score: i32,
        accuracy: f32,
        full_combo: bool,
    ) -> GameResult<CommitGameOutcome> {
        let mut g = self.inner.lock().unwrap();
        let RoomState::Playing { done, .. } = &mut g.state else {
            return Err(GameError("error.invalid_state"));
        };
        if !done.insert(user_id) {
            // 幂等
            return Ok(CommitGameOutcome {
                broadcasts: vec![],
                game_ended: false,
                recording: None,
            });
        }

        // 取录制数据（若该玩家有触摸/判定数据）
        let recording = {
            let RoomState::Playing { touch_frames, judge_events, chart_id, chart_name, .. } = &mut g.state
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

        let mut out = broadcast_all(
            &g,
            ClientBoundPacket::message(Message::Played {
                user: user_id,
                score,
                accuracy,
                full_combo,
            }),
        );

        let ended = self.check_game_end_inner(&mut g, &mut out);
        Ok(CommitGameOutcome {
            broadcasts: out,
            game_ended: ended,
            recording,
        })
    }

    /// abort（幂等）。
    pub fn commit_abort(&self, user_id: i32) -> GameResult<CommitGameOutcome> {
        let mut g = self.inner.lock().unwrap();
        let RoomState::Playing { done, .. } = &mut g.state else {
            return Err(GameError("error.invalid_state"));
        };
        if !done.insert(user_id) {
            return Ok(CommitGameOutcome {
                broadcasts: vec![],
                game_ended: false,
                recording: None,
            });
        }
        let mut out = broadcast_all(
            &g,
            ClientBoundPacket::message(Message::Abort { user: user_id }),
        );
        let ended = self.check_game_end_inner(&mut g, &mut out);
        Ok(CommitGameOutcome {
            broadcasts: out,
            game_ended: ended,
            recording: None,
        })
    }

    /// 对局结束判定（6.5 节）：所有在线 players（不含 monitor）完成。
    fn check_game_end_inner(
        &self,
        g: &mut Inner,
        out: &mut Vec<(Arc<Player>, ClientBoundPacket)>,
    ) -> bool {
        let RoomState::Playing { done, chart_id, chart_name, .. } = &g.state else {
            return false;
        };
        let online_players: Vec<i32> = g
            .players
            .iter()
            .filter(|p| p.is_online())
            .map(|p| p.id())
            .collect();
        let all_done = !online_players.is_empty()
            && online_players.iter().all(|id| done.contains(id));
        // 全员掉线场景：没有在线玩家也算结束
        let no_online = online_players.is_empty();
        if !all_done && !no_online {
            return false;
        }

        let (cid, cn) = (*chart_id, chart_name.clone());
        g.state = RoomState::SelectChart {
            chart_id: cid,
            chart_name: cn,
        };
        out.extend(broadcast_all(
            g,
            ClientBoundPacket::change_state(GameState::SelectChart { chart_id: cid }),
        ));
        out.extend(broadcast_all(g, ClientBoundPacket::message(Message::GameEnd)));

        // cycle → 转移房主
        if g.setting.cycle {
            if let Some((old, new)) = transfer_host_inner(g) {
                out.push((old, ClientBoundPacket::change_host(false)));
                out.push((new.clone(), ClientBoundPacket::change_host(true)));
                for target in g.players.iter().chain(g.monitors.iter()) {
                    if target.id() != new.id() {
                        out.push((
                            target.clone(),
                            ClientBoundPacket::message(Message::NewHost { user: new.id() }),
                        ));
                    }
                }
            }
        }
        true
    }

    /// touch/judge 收集 + 转发 Monitor（6.5 节）。
    /// 返回需要转发给 monitor 的包（锁外发送）。
    pub fn touch_send(
        &self,
        user_id: i32,
        frames: Vec<TouchFrame>,
    ) -> Vec<(Arc<Player>, ClientBoundPacket)> {
        let mut g = self.inner.lock().unwrap();
        if let RoomState::Playing { touch_frames, .. } = &mut g.state {
            touch_frames.entry(user_id).or_default().extend(frames.iter().cloned());
            g.monitors
                .iter()
                .map(|m| {
                    (
                        m.clone(),
                        ClientBoundPacket::Touches {
                            from_player_id: user_id,
                            frames: frames.clone(),
                            trailer: None,
                        },
                    )
                })
                .collect()
        } else {
            vec![] // 非 Playing 忽略
        }
    }

    pub fn judge_send(
        &self,
        user_id: i32,
        judges: Vec<JudgeEvent>,
    ) -> Vec<(Arc<Player>, ClientBoundPacket)> {
        let mut g = self.inner.lock().unwrap();
        if let RoomState::Playing { judge_events, .. } = &mut g.state {
            judge_events.entry(user_id).or_default().extend(judges.iter().copied());
            g.monitors
                .iter()
                .map(|m| {
                    (
                        m.clone(),
                        ClientBoundPacket::Judges {
                            from_player_id: user_id,
                            judges: judges.clone(),
                            trailer: None,
                        },
                    )
                })
                .collect()
        } else {
            vec![]
        }
    }

    // ---------- 会话挂起支持（5.3 节） ----------

    /// 挂起前清理现场：WaitForReady → cancelReady；Playing → abort。
    /// 返回需要广播的包。
    pub fn cleanup_for_suspend(&self, user_id: i32) -> Vec<(Arc<Player>, ClientBoundPacket)> {
        let mut g = self.inner.lock().unwrap();
        let mut out = Vec::new();
        match &mut g.state {
            RoomState::WaitForReady { ready, .. } => {
                if ready.remove(&user_id) {
                    out.extend(broadcast_all(
                        &g,
                        ClientBoundPacket::message(Message::CancelReady { user: user_id }),
                    ));
                }
            }
            RoomState::Playing { done, .. } => {
                if done.insert(user_id) {
                    out.extend(broadcast_all(
                        &g,
                        ClientBoundPacket::message(Message::Abort { user: user_id }),
                    ));
                    self.check_game_end_inner(&mut g, &mut out);
                }
            }
            _ => {}
        }
        out
    }

    /// 快照（控制台命令用）。
    pub fn snapshot(&self) -> RoomSnapshot {
        let g = self.inner.lock().unwrap();
        RoomSnapshot {
            room_id: self.id.clone(),
            state_kind_name: match &g.state {
                RoomState::SelectChart { .. } => "SelectChart",
                RoomState::WaitForReady { .. } => "WaitForReady",
                RoomState::Playing { .. } => "Playing",
            }
            .to_string(),
            locked: g.setting.locked,
            players: g.players.iter().map(|p| p.id()).collect(),
            monitors: g.monitors.iter().map(|p| p.id()).collect(),
        }
    }

    pub fn all_members(&self) -> Vec<Arc<Player>> {
        let g = self.inner.lock().unwrap();
        g.players.iter().chain(g.monitors.iter()).cloned().collect()
    }

    pub fn is_monitor_user(&self, user_id: i32) -> bool {
        self.inner.lock().unwrap().monitors.iter().any(|p| p.id() == user_id)
    }
}

/// played 时提取的录制数据（第 10 节）。
pub struct RecordingData {
    pub chart_id: Option<i32>,
    pub chart_name: Option<String>,
    pub touch_frames: Vec<TouchFrame>,
    pub judge_events: Vec<JudgeEvent>,
}

pub struct CommitGameOutcome {
    pub broadcasts: Vec<(Arc<Player>, ClientBoundPacket)>,
    pub game_ended: bool,
    pub recording: Option<RecordingData>,
}

pub struct RoomSnapshot {
    pub room_id: String,
    state_kind_name: String,
    pub locked: bool,
    pub players: Vec<i32>,
    pub monitors: Vec<i32>,
}

impl RoomSnapshot {
    pub fn state_kind(&self) -> &str {
        &self.state_kind_name
    }
}

/// 房主转移算法（6.6 节）：按 userId 升序，取第一个大于当前房主的；无则取最小。
/// 返回 (旧房主, 新房主)。
fn transfer_host_inner(g: &mut Inner) -> Option<(Arc<Player>, Arc<Player>)> {
    let old_host = g.host.clone()?;
    if g.players.is_empty() {
        g.host = None;
        return None;
    }
    let mut sorted: Vec<Arc<Player>> = g.players.clone();
    sorted.sort_by_key(|p| p.id());
    let new_host = sorted
        .iter()
        .find(|p| p.id() > old_host.id())
        .cloned()
        .unwrap_or_else(|| sorted[0].clone());
    g.host = Some(new_host.clone());
    Some((old_host, new_host))
}

fn broadcast_all(g: &Inner, packet: ClientBoundPacket) -> Vec<(Arc<Player>, ClientBoundPacket)> {
    g.players
        .iter()
        .chain(g.monitors.iter())
        .map(|p| (p.clone(), packet.clone()))
        .collect()
}

/// 房间注册表（7.3 节）。
pub struct RoomRegistry {
    rooms: Mutex<std::collections::HashMap<String, Weak<Room>>>,
}

impl Default for RoomRegistry {
    fn default() -> Self {
        Self::new()
    }
}

impl RoomRegistry {
    pub fn new() -> Self {
        Self {
            rooms: Mutex::new(std::collections::HashMap::new()),
        }
    }

    /// 创建房间；已存在返回 error.room_already_exists。
    pub fn create_room(&self, room_id: &str) -> GameResult<Arc<Room>> {
        let mut map = self.rooms.lock().unwrap();
        if let Some(existing) = map.get(room_id).and_then(|w| w.upgrade()) {
            if !existing.is_destroyed() {
                return Err(GameError("error.room_already_exists"));
            }
        }
        let rid = room_id.to_string();
        let room = Room::new(rid.clone(), RoomSetting::default(), {
            let weak_registry_remove = {
                // 通过全局 ctx 移除（on_destroy 回调）
                move || {
                    crate::server::with_server_ctx(|ctx| ctx.rooms.remove(&rid));
                }
            };
            weak_registry_remove
        });
        map.insert(room_id.to_string(), Arc::downgrade(&room));
        Ok(room)
    }

    pub fn find_room(&self, room_id: &str) -> Option<Arc<Room>> {
        let mut map = self.rooms.lock().unwrap();
        let room = map.get(room_id).and_then(|w| w.upgrade());
        if room.is_none() {
            map.remove(room_id); // 清理失效弱引用
        }
        room.filter(|r| !r.is_destroyed())
    }

    pub fn remove(&self, room_id: &str) {
        self.rooms.lock().unwrap().remove(room_id);
    }

    pub fn all_rooms(&self) -> Vec<Arc<Room>> {
        let mut map = self.rooms.lock().unwrap();
        let mut out = Vec::new();
        let mut dead = Vec::new();
        for (k, w) in map.iter() {
            match w.upgrade() {
                Some(r) if !r.is_destroyed() => out.push(r),
                _ => dead.push(k.clone()),
            }
        }
        for k in dead {
            map.remove(&k);
        }
        out
    }
}

impl PacketResult<()> {
    pub fn from_game_result(r: GameResult<()>, i18n: &crate::i18n::I18nService, lang: Option<&str>) -> Self {
        match r {
            Ok(()) => PacketResult::ok(),
            Err(e) => PacketResult::failed(i18n.message(lang, e.0)),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::network::connection::ConnectionHandle;
    use crate::phira::UserInfo;

    /// 构造测试用玩家（连接句柄指向一个永不使用的 dummy writer）。
    fn make_player(id: i32, name: &str) -> Arc<Player> {
        let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
        // 通过 ConnectionHandle 的测试构造器不可得，这里用真实结构
        let conn = ConnectionHandle::new_for_test(tx);
        Player::new(
            Arc::new(UserInfo {
                id,
                name: name.to_string(),
                ..Default::default()
            }),
            conn,
        )
    }

    fn make_room() -> Arc<Room> {
        Room::new("R1", RoomSetting::default(), || {})
    }

    #[test]
    fn join_first_player_becomes_host() {
        let room = make_room();
        let p1 = make_player(1, "A");
        let (outcome, _b) = room.join(p1.clone(), false).unwrap();
        assert!(matches!(outcome, JoinOutcome::FirstPlayer));
        assert!(room.is_host(1));
        assert!(room.contains_member(1));
    }

    #[test]
    fn join_room_full_and_locked() {
        let setting = RoomSetting {
            max_player: 1,
            ..Default::default()
        };
        let room = Room::new("R2", setting, || {});
        room.join(make_player(1, "A"), false).unwrap();
        let err = room.join(make_player(2, "B"), false).unwrap_err();
        assert_eq!(err.0, "error.room_full");
        // monitor 不占名额
        room.join(make_player(3, "M"), true).unwrap();
        assert!(room.is_monitor_user(3));
    }

    #[test]
    fn lock_toggle_ignores_client_value() {
        let room = make_room();
        room.join(make_player(1, "A"), false).unwrap();
        // 第一次切换 → locked=true
        room.toggle_lock(1).unwrap();
        // 锁房后非首玩家无法加入
        let err = room.join(make_player(2, "B"), false).unwrap_err();
        assert_eq!(err.0, "error.room_locked");
        // monitor 不受限
        room.join(make_player(3, "M"), true).unwrap();
        // 再切回
        room.toggle_lock(1).unwrap();
        room.join(make_player(2, "B"), false).unwrap();
        // 非房主无权操作
        assert_eq!(room.toggle_lock(2).unwrap_err().0, "error.permission_denied");
    }

    #[test]
    fn host_transfer_by_user_id_order() {
        let room = make_room();
        room.join(make_player(5, "E"), false).unwrap(); // host=5
        room.join(make_player(2, "B"), false).unwrap();
        room.join(make_player(9, "I"), false).unwrap();
        // host 5 离开 → 下一个大于 5 的 → 9
        let (_left, _b, _d) = room.leave(5);
        assert!(room.is_host(9));
        // host 9 离开 → 无更大 id → 最小 id 2
        room.leave(9);
        assert!(room.is_host(2));
    }

    #[test]
    fn single_player_start_goes_playing_directly() {
        let room = make_room();
        room.join(make_player(1, "A"), false).unwrap();
        room.commit_select_chart(1, 42, "Chart".into(), "A".into()).unwrap();
        room.require_start(1).unwrap();
        assert!(matches!(room.game_state_protocol(), GameState::Playing));
    }

    #[test]
    fn multi_player_ready_flow() {
        let room = make_room();
        room.join(make_player(1, "A"), false).unwrap();
        room.join(make_player(2, "B"), false).unwrap();
        room.commit_select_chart(1, 42, "Chart".into(), "A".into()).unwrap();
        room.require_start(1).unwrap();
        // 发起者自动 ready；另一人 ready 后 → Playing（谱面保留）
        assert!(matches!(room.game_state_protocol(), GameState::WaitForReady));
        let (_b, started) = room.ready(2).unwrap();
        assert!(started);
        assert!(matches!(room.game_state_protocol(), GameState::Playing));
        // cancelReady 在 Playing 状态非法
        assert_eq!(room.cancel_ready(1).unwrap_err().0, "error.invalid_state");
    }

    #[test]
    fn playing_played_idempotent_and_game_end_cycle() {
        let setting = RoomSetting {
            cycle: true,
            ..Default::default()
        };
        let room = Room::new("RC", setting, || {});
        room.join(make_player(1, "A"), false).unwrap();
        room.join(make_player(2, "B"), false).unwrap();
        room.commit_select_chart(1, 42, "Chart".into(), "A".into()).unwrap();
        room.require_start(1).unwrap();
        room.ready(2).unwrap();

        // played 幂等
        match room.check_played(1).unwrap() {
            PlayedCheck::CanPlay { chart_id, .. } => assert_eq!(chart_id, Some(42)),
            _ => panic!(),
        }
        let out1 = room.commit_played(1, 990000, 0.995, true).unwrap();
        assert!(!out1.game_ended);
        match room.check_played(1).unwrap() {
            PlayedCheck::AlreadyDone => {}
            _ => panic!("played should be idempotent"),
        }

        // 第二人 abort → 对局结束 → 回 SelectChart（保留谱面）→ cycle 换房主
        let out2 = room.commit_abort(2).unwrap();
        assert!(out2.game_ended);
        match room.game_state_protocol() {
            GameState::SelectChart { chart_id } => assert_eq!(chart_id, Some(42)),
            _ => panic!(),
        }
        assert!(room.is_host(2), "cycle should transfer host to next player");
    }

    #[test]
    fn suspend_cleanup_cancel_ready() {
        let room = make_room();
        room.join(make_player(1, "A"), false).unwrap();
        room.join(make_player(2, "B"), false).unwrap();
        room.require_start(1).unwrap();
        // B 掉线挂起 → 自动 cancelReady（B 还没 ready，无广播但状态不卡）
        let _ = room.cleanup_for_suspend(2);
        // B 重新 ready（恢复后）→ 正常开局
        let (_b, started) = room.ready(2).unwrap();
        assert!(started);
    }

    #[test]
    fn touch_forward_only_in_playing() {
        let room = make_room();
        room.join(make_player(1, "A"), false).unwrap();
        room.join(make_player(9, "M"), true).unwrap();
        // 非 Playing → 忽略
        let f = room.touch_send(1, vec![]);
        assert!(f.is_empty());
        room.commit_select_chart(1, 1, "C".into(), "A".into()).unwrap();
        room.require_start(1).unwrap();
        room.ready(9).unwrap();
        let f = room.touch_send(1, vec![crate::packet::data::TouchFrame {
            time: 0.5,
            points: vec![],
        }]);
        assert_eq!(f.len(), 1); // 仅转发给 monitor
    }
}
