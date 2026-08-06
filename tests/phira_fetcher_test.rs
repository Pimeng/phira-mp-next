//! PhiraFetcher 测试（本地 mock HTTP API）。
//!
//! 覆盖：/me /chart /record /user 成功路径、TTL 缓存命中（不发第二次请求）、
//! 404 快速失败、5xx 重试（一次失败后成功 / 持续失败）、坏 JSON。

use phira_mp::phira::{PhiraError, PhiraFetcher, PhiraFetcherConfig};
use serde_json::json;
use std::collections::HashMap;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;

/// 可编程响应脚本。
#[derive(Default)]
struct Script {
    /// path → 剩余 500 次数（之后走常规响应）
    fail_then_ok: Mutex<HashMap<String, usize>>,
    /// 总是 500
    always_500: Vec<String>,
    /// 200 但返回坏 JSON
    bad_json: Vec<String>,
    /// 404
    not_found: Vec<String>,
}

struct MockApi {
    addr: String,
    shutdown: tokio::sync::oneshot::Sender<()>,
    requests: Arc<AtomicUsize>,
}

async fn start_mock(script: Arc<Script>) -> MockApi {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap().to_string();
    let (tx, mut rx) = tokio::sync::oneshot::channel::<()>();
    let requests = Arc::new(AtomicUsize::new(0));
    let requests_task = requests.clone();

    tokio::spawn(async move {
        loop {
            tokio::select! {
                accept = listener.accept() => {
                    let Ok((mut stream, _)) = accept else { continue };
                    let script = script.clone();
                    let requests = requests_task.clone();
                    tokio::spawn(async move {
                        let mut buf = vec![0u8; 8192];
                        let n = stream.read(&mut buf).await.unwrap_or(0);
                        let req = String::from_utf8_lossy(&buf[..n]).to_string();
                        let path = req.split_whitespace().nth(1).unwrap_or("/").to_string();
                        let token = req.lines().find_map(|l| {
                            let lower = l.to_ascii_lowercase();
                            lower.strip_prefix("authorization: bearer ")
                                .map(|_| l["Authorization: Bearer ".len()..].trim().to_string())
                        });
                        requests.fetch_add(1, Ordering::SeqCst);
                        let (status, body) = respond(&path, token.as_deref(), &script);
                        let resp = format!(
                            "HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                            body.len()
                        );
                        let _ = stream.write_all(resp.as_bytes()).await;
                        let _ = stream.shutdown().await;
                    });
                }
                _ = &mut rx => break,
            }
        }
    });

    MockApi {
        addr,
        shutdown: tx,
        requests,
    }
}

fn respond(path: &str, token: Option<&str>, script: &Script) -> (&'static str, String) {
    if script.not_found.iter().any(|p| p == path) {
        return ("404 Not Found", "{}".to_string());
    }
    if script.bad_json.iter().any(|p| p == path) {
        return ("200 OK", "this is {not json".to_string());
    }
    if script.always_500.iter().any(|p| p == path) {
        return ("500 Internal Server Error", "{}".to_string());
    }
    {
        let mut m = script.fail_then_ok.lock().unwrap();
        if let Some(n) = m.get_mut(path)
            && *n > 0
        {
            *n -= 1;
            return ("500 Internal Server Error", "{}".to_string());
        }
    }
    let seg = path.trim_start_matches('/');
    if seg == "me" {
        let id: i32 = token
            .and_then(|t| t.strip_prefix("tok-"))
            .and_then(|s| s.parse().ok())
            .unwrap_or(1);
        return (
            "200 OK",
            json!({
                "id": id, "name": format!("User{id}"), "language": "en-US",
                "rks": 16.0, "banned": false, "loginBanned": false, "roles": 2, "exp": 100
            })
            .to_string(),
        );
    }
    if let Some(rest) = seg.strip_prefix("chart/") {
        let id: i32 = rest.parse().unwrap_or(0);
        return (
            "200 OK",
            json!({
                "id": id, "name": format!("Chart{id}"), "level": "IN",
                "difficulty": 15.5, "charter": "c", "composer": "m",
                "ranked": false, "uploader": 3
            })
            .to_string(),
        );
    }
    if let Some(rest) = seg.strip_prefix("record/") {
        let id: i32 = rest.parse().unwrap_or(0);
        return (
            "200 OK",
            json!({
                "id": id, "player": 7, "chart": 42, "score": 1000000, "accuracy": 100.0,
                "perfect": 100, "good": 0, "bad": 0, "miss": 0,
                "fullCombo": true, "maxCombo": 100, "speed": 1.1, "time": "2026-01-01T00:00:00Z"
            })
            .to_string(),
        );
    }
    if let Some(rest) = seg.strip_prefix("user/") {
        let id: i32 = rest.parse().unwrap_or(0);
        return (
            "200 OK",
            json!({
                "id": id, "name": format!("User{id}"), "language": "zh-CN",
                "rks": 15.0, "banned": false, "loginBanned": false
            })
            .to_string(),
        );
    }
    ("404 Not Found", "{}".to_string())
}

async fn fetcher_for(api: &MockApi) -> PhiraFetcher {
    PhiraFetcher::new(PhiraFetcherConfig {
        api_url: format!("http://{}/", api.addr),
        ..Default::default()
    })
}

#[tokio::test]
async fn get_user_info_success_and_cached() {
    let script = Arc::new(Script::default());
    let api = start_mock(script).await;
    let f = fetcher_for(&api).await;

    let u = f.get_user_info("tok-5").await.unwrap();
    assert_eq!(u.id, 5);
    assert_eq!(u.name, "User5");
    assert_eq!(u.language.as_deref(), Some("en-US"));
    assert_eq!(u.rks, 16.0);
    assert!(!u.banned);

    // 缓存命中：第二次不发请求
    let n1 = api.requests.load(Ordering::SeqCst);
    let u2 = f.get_user_info("tok-5").await.unwrap();
    assert_eq!(u2.id, 5);
    assert_eq!(api.requests.load(Ordering::SeqCst), n1, "缓存应命中");

    // 不同 token → 新请求
    let u3 = f.get_user_info("tok-9").await.unwrap();
    assert_eq!(u3.id, 9);
    assert_eq!(api.requests.load(Ordering::SeqCst), n1 + 1);

    let _ = api.shutdown.send(());
}

#[tokio::test]
async fn get_chart_and_record_info() {
    let script = Arc::new(Script::default());
    let api = start_mock(script).await;
    let f = fetcher_for(&api).await;

    let c = f.get_chart_info(42).await.unwrap();
    assert_eq!(c.id, 42);
    assert_eq!(c.name, "Chart42");
    assert_eq!(c.level, "IN");
    assert!(!c.ranked);

    let r = f.get_record_info(100).await.unwrap();
    assert_eq!(r.id, 100);
    assert_eq!(r.score, 1_000_000);
    assert!(r.full_combo);

    // 缓存
    let n = api.requests.load(Ordering::SeqCst);
    f.get_chart_info(42).await.unwrap();
    f.get_record_info(100).await.unwrap();
    assert_eq!(api.requests.load(Ordering::SeqCst), n);

    let _ = api.shutdown.send(());
}

#[tokio::test]
async fn get_user_by_id() {
    let script = Arc::new(Script::default());
    let api = start_mock(script).await;
    let f = fetcher_for(&api).await;
    let u = f.get_user(3).await.unwrap();
    assert_eq!(u.id, 3);
    let _ = api.shutdown.send(());
}

#[tokio::test]
async fn not_found_returns_immediately() {
    let mut script = Script::default();
    script.not_found.push("/user/999".to_string());
    script.not_found.push("/chart/404".to_string());
    script.not_found.push("/record/404".to_string());
    let script = Arc::new(script);
    let api = start_mock(script).await;
    let f = fetcher_for(&api).await;

    assert!(matches!(
        f.get_user(999).await,
        Err(PhiraError::NotFound(_))
    ));
    assert!(matches!(
        f.get_chart_info(404).await,
        Err(PhiraError::NotFound(_))
    ));
    assert!(matches!(
        f.get_record_info(404).await,
        Err(PhiraError::NotFound(_))
    ));
    // 404 不重试：请求数应保持 3
    assert_eq!(api.requests.load(Ordering::SeqCst), 3);
    let _ = api.shutdown.send(());
}

#[tokio::test]
async fn bad_json_is_http_error() {
    let mut script = Script::default();
    script.bad_json.push("/me".to_string());
    let script = Arc::new(script);
    let api = start_mock(script).await;
    let f = fetcher_for(&api).await;
    let err = f.get_user_info("tok-1").await.unwrap_err();
    assert!(matches!(err, PhiraError::Http(_)));
    let _ = api.shutdown.send(());
}

#[tokio::test]
async fn retry_after_transient_failure() {
    let script = Script {
        fail_then_ok: Mutex::new(HashMap::from([("/me".to_string(), 1)])),
        ..Default::default()
    };
    let script = Arc::new(script);
    let api = start_mock(script).await;
    let f = fetcher_for(&api).await;

    // 第一次 500 → 重试 → 第二次成功
    let u = f.get_user_info("tok-1").await.unwrap();
    assert_eq!(u.id, 1);
    assert!(api.requests.load(Ordering::SeqCst) >= 2, "应至少请求两次");
    let _ = api.shutdown.send(());
}

#[tokio::test]
async fn persistent_5xx_fails_after_retries() {
    let mut script = Script::default();
    script.always_500.push("/chart/7".to_string());
    let script = Arc::new(script);
    let api = start_mock(script).await;
    let f = fetcher_for(&api).await;

    let err = f.get_chart_info(7).await.unwrap_err();
    assert!(matches!(err, PhiraError::Http(_)));
    assert_eq!(api.requests.load(Ordering::SeqCst), 5, "最多重试 5 次");
    let _ = api.shutdown.send(());
}

#[tokio::test]
async fn network_error_fails() {
    // 指向一个不存在的端口 → 连接失败 → 重试后 Http 错误
    let f = PhiraFetcher::new(PhiraFetcherConfig {
        api_url: "http://127.0.0.1:1/".into(),
        ..Default::default()
    });
    let err = f.get_user_info("tok-1").await.unwrap_err();
    assert!(matches!(err, PhiraError::Http(_)));
}
