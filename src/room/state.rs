//! 房间状态机（6.1 节）：密封三态 + 切换语义。
//!
//! 注意：WaitForReady 与 Playing 都必须保留 chart_id/chart_name，
//! 否则多人开局谱面丢失。

use crate::packet::data::{JudgeEvent, TouchFrame};
use crate::packet::state::GameState;
use std::collections::{HashMap, HashSet};

#[derive(Debug)]
pub enum RoomState {
    SelectChart {
        chart_id: Option<i32>,
        chart_name: Option<String>,
    },
    WaitForReady {
        chart_id: Option<i32>,
        chart_name: Option<String>,
        ready: HashSet<i32>,
    },
    Playing {
        chart_id: Option<i32>,
        chart_name: Option<String>,
        /// 已完成（played/abort/中途加入视为完成）。
        done: HashSet<i32>,
        touch_frames: HashMap<i32, Vec<TouchFrame>>,
        judge_events: HashMap<i32, Vec<JudgeEvent>>,
    },
}

impl RoomState {
    pub fn to_protocol(&self) -> GameState {
        match self {
            RoomState::SelectChart { chart_id, .. } => GameState::SelectChart {
                chart_id: *chart_id,
            },
            RoomState::WaitForReady { .. } => GameState::WaitForReady,
            RoomState::Playing { .. } => GameState::Playing,
        }
    }

    /// 对局中加入的玩家直接标记 done（6.5 节 handleJoin）。
    pub fn handle_join(&mut self, user_id: i32) {
        if let RoomState::Playing { done, .. } = self {
            done.insert(user_id);
        }
    }

    pub fn handle_leave(&mut self, user_id: i32) {
        match self {
            RoomState::WaitForReady { ready, .. } => {
                ready.remove(&user_id);
            }
            RoomState::Playing { done, .. } => {
                // 离开即视为完成（避免对局卡死等待离开者）
                done.insert(user_id);
            }
            _ => {}
        }
    }

    /// 当前对局的成绩表（GameEndEvent 载荷用）。
    pub fn records(&self) -> &[(i32, i32, f32, bool)] {
        &[]
    }
}
