//! 会话挂起/恢复测试（SessionManager）。
//!
//! 覆盖：suspend/resume round-trip、恢复失败语义（无会话/已离房）、
//! 超时 force-leave + remover、重复挂起代次（旧超时任务不误杀）、
//! 挂起清理广播（WaitForReady→CancelReady、Playing→Abort）。

mod common;

use common::TestPlayer;
use phira_mp::packet::message::Message;
use phira_mp::player::Player;
use phira_mp::room::{LocalRoom, Room, RoomSetting};
use phira_mp::session::{ResumeFailed, SessionManager};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

fn make_player(id: i32) -> Arc<TestPlayer> {
    TestPlayer::new(id, &format!("P{id}"))
}

/// Arc<TestPlayer> → Arc<dyn Player>（SessionManager/Room 接口需要）。
fn as_dyn(p: Arc<TestPlayer>) -> Arc<dyn Player> {
    p as Arc<dyn Player>
}

fn make_room() -> Arc<LocalRoom> {
    LocalRoom::new("RS", RoomSetting::default(), || {})
}

/// 断言 resume 失败并返回错误码。
fn resume_err(sm: &Arc<SessionManager>, p: &Arc<TestPlayer>) -> &'static str {
    match sm.resume(&as_dyn(p.clone())) {
        Err(ResumeFailed(msg)) => msg,
        Ok(_) => panic!("expected resume failure"),
    }
}

#[tokio::test]
async fn suspend_and_resume_roundtrip() {
    let sm = Arc::new(SessionManager::new());
    let player = make_player(1);
    let room = make_room();
    room.join(as_dyn(player.clone()), false).unwrap();

    sm.suspend(as_dyn(player.clone()), room.clone(), || {})
        .await
        .unwrap();
    assert!(sm.has_suspended(1));
    assert!(room.contains_member(1), "挂起后仍在房间");

    let s = sm.resume(&as_dyn(player.clone())).unwrap();
    assert_eq!(s.user_id, 1);
    assert!(s.room.contains_member(1));
    assert!(!sm.has_suspended(1), "resume 是 take 语义");
}

#[tokio::test]
async fn resume_without_session_fails() {
    let sm = Arc::new(SessionManager::new());
    let player = make_player(1);
    assert_eq!(resume_err(&sm, &player), "ERROR_SESSION_NOT_FOUND");
    // take_suspended 空
    assert!(sm.take_suspended(1).is_none());
}

#[tokio::test]
async fn resume_after_leave_fails_expired() {
    let sm = Arc::new(SessionManager::new());
    let player = make_player(1);
    let room = make_room();
    room.join(as_dyn(player.clone()), false).unwrap();

    sm.suspend(as_dyn(player.clone()), room.clone(), || {})
        .await
        .unwrap();
    // 挂起期间玩家离开房间（如被房主踢出）
    room.leave(1);

    assert_eq!(resume_err(&sm, &player), "ERROR_SESSION_EXPIRED");
}

#[tokio::test]
async fn suspend_timeout_forces_leave_and_remover() {
    let sm = Arc::new(SessionManager::new());
    sm.set_timeout(Duration::from_millis(80));
    let player = make_player(1);
    let room = make_room();
    room.join(as_dyn(player.clone()), false).unwrap();

    let removed = Arc::new(AtomicBool::new(false));
    let flag = removed.clone();
    sm.suspend(as_dyn(player.clone()), room.clone(), move || {
        flag.store(true, Ordering::SeqCst);
    })
    .await
    .unwrap();
    assert!(sm.has_suspended(1));

    // 等待超时任务执行
    for _ in 0..100 {
        if removed.load(Ordering::SeqCst) {
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    assert!(removed.load(Ordering::SeqCst), "remover 应在超时后被调用");
    assert!(!sm.has_suspended(1), "会话应被取出");
    assert!(!room.contains_member(1), "超时后应 force-leave");
}

#[tokio::test]
async fn suspend_timeout_only_once() {
    let sm = Arc::new(SessionManager::new());
    sm.set_timeout(Duration::from_millis(60));
    let player = make_player(1);
    let room = make_room();
    room.join(as_dyn(player.clone()), false).unwrap();

    let calls = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let c = calls.clone();
    sm.suspend(as_dyn(player.clone()), room.clone(), move || {
        c.fetch_add(1, Ordering::SeqCst);
    })
    .await
    .unwrap();
    // 等超时完成
    for _ in 0..100 {
        if calls.load(Ordering::SeqCst) > 0 {
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    assert_eq!(calls.load(Ordering::SeqCst), 1, "remover 只应被调用一次");
}

#[tokio::test]
async fn repeated_suspend_old_timeout_does_not_fire() {
    let sm = Arc::new(SessionManager::new());
    sm.set_timeout(Duration::from_millis(80));
    let player = make_player(1);
    let room = make_room();
    room.join(as_dyn(player.clone()), false).unwrap();

    let old_fired = Arc::new(AtomicBool::new(false));
    let old_flag = old_fired.clone();
    sm.suspend(as_dyn(player.clone()), room.clone(), move || {
        old_flag.store(true, Ordering::SeqCst);
    })
    .await
    .unwrap();

    // 立即再次挂起（代次 +1）
    let new_fired = Arc::new(AtomicBool::new(false));
    let new_flag = new_fired.clone();
    sm.suspend(as_dyn(player.clone()), room.clone(), move || {
        new_flag.store(true, Ordering::SeqCst);
    })
    .await
    .unwrap();

    // 等待超过 timeout：旧代次任务应失效，只有新任务触发
    for _ in 0..100 {
        if new_fired.load(Ordering::SeqCst) {
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    assert!(new_fired.load(Ordering::SeqCst), "新挂起超时应触发");
    assert!(
        !old_fired.load(Ordering::SeqCst),
        "旧挂起的超时任务不得误杀"
    );
    assert!(!sm.has_suspended(1));
}

#[tokio::test]
async fn resume_before_timeout_suppresses_timeout() {
    let sm = Arc::new(SessionManager::new());
    sm.set_timeout(Duration::from_millis(120));
    let player = make_player(1);
    let room = make_room();
    room.join(as_dyn(player.clone()), false).unwrap();

    let removed = Arc::new(AtomicBool::new(false));
    let flag = removed.clone();
    sm.suspend(as_dyn(player.clone()), room.clone(), move || {
        flag.store(true, Ordering::SeqCst);
    })
    .await
    .unwrap();

    // 超时前恢复
    let s = sm.resume(&as_dyn(player.clone())).unwrap();
    assert_eq!(s.room.id(), "RS");
    tokio::time::sleep(Duration::from_millis(200)).await;
    assert!(
        !removed.load(Ordering::SeqCst),
        "resume 后超时任务不得触发 remover"
    );
    assert!(room.contains_member(1));
}

#[tokio::test]
async fn suspend_cleanup_wait_ready_broadcasts_cancel_ready() {
    let sm = Arc::new(SessionManager::new());
    let p1 = make_player(1);
    let p2 = make_player(2);
    let room = make_room();
    room.join(as_dyn(p1.clone()), false).unwrap();
    room.join(as_dyn(p2.clone()), false).unwrap();
    room.commit_select_chart(1, 42, "C42".into()).unwrap();
    room.require_start(1).unwrap(); // WaitForReady，p1 ready

    sm.suspend(as_dyn(p1.clone()), room.clone(), || {})
        .await
        .unwrap();
    // p2 应收到 CancelReady(1) 广播
    assert!(
        p2.messages()
            .iter()
            .any(|m| matches!(m, Message::CancelReady { user: 1 }))
    );
}

#[tokio::test]
async fn suspend_cleanup_playing_broadcasts_abort() {
    let sm = Arc::new(SessionManager::new());
    let p1 = make_player(1);
    let p2 = make_player(2);
    let room = make_room();
    room.join(as_dyn(p1.clone()), false).unwrap();
    room.join(as_dyn(p2.clone()), false).unwrap();
    room.commit_select_chart(1, 42, "C42".into()).unwrap();
    room.require_start(1).unwrap();
    room.ready(2).unwrap(); // → Playing

    sm.suspend(as_dyn(p1.clone()), room.clone(), || {})
        .await
        .unwrap();
    assert!(
        p2.messages()
            .iter()
            .any(|m| matches!(m, Message::Abort { user: 1 }))
    );
    // p2 未完成，对局不结束
    assert!(matches!(
        room.game_state_protocol(),
        phira_mp::packet::state::GameState::Playing
    ));
}

#[tokio::test]
async fn monitor_suspend_roundtrip() {
    // SessionManager 不区分 monitor/player（挂起语义由 handler 层决定），
    // 这里验证 monitor 挂起/恢复也能正常工作。
    let sm = Arc::new(SessionManager::new());
    let p1 = make_player(1);
    let m = make_player(50);
    let room = make_room();
    room.join(as_dyn(p1.clone()), false).unwrap();
    room.join(as_dyn(m.clone()), true).unwrap();

    sm.suspend(as_dyn(m.clone()), room.clone(), || {})
        .await
        .unwrap();
    assert!(sm.has_suspended(50));
    let s = sm.resume(&as_dyn(m.clone())).unwrap();
    assert_eq!(s.user_id, 50);
}

#[tokio::test]
async fn default_timeout_is_300_seconds() {
    let sm = Arc::new(SessionManager::new());
    assert_eq!(sm.timeout(), Duration::from_secs(300));
    sm.set_timeout(Duration::from_secs(1));
    assert_eq!(sm.timeout(), Duration::from_secs(1));
}
