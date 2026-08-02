//! 事件总线测试（EventBus）：订阅/发布/退订、post_mut 可变事件、
//! 多订阅者顺序、handler panic 隔离、无订阅者安全路径。

use phira_mp::eventbus::EventBus;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

const KEY: &str = "test.event";
const KEY2: &str = "test.event2";

#[derive(Debug, Default, Clone, PartialEq)]
struct CounterEvent {
    value: usize,
    cancelled: bool,
}

#[tokio::test]
async fn subscribe_and_post() {
    let bus = EventBus::new();
    let got = Arc::new(AtomicUsize::new(0));
    let g = got.clone();
    let _sub = bus.subscribe(KEY, move |_: &CounterEvent| {
        let g = g.clone();
        async move {
            g.fetch_add(1, Ordering::SeqCst);
        }
    });

    bus.post(KEY, CounterEvent { value: 1, cancelled: false }).await;
    assert_eq!(got.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn multiple_subscribers_run_in_order() {
    let bus = EventBus::new();
    let order = Arc::new(std::sync::Mutex::new(Vec::new()));
    let o1 = order.clone();
    let _s1 = bus.subscribe(KEY, move |_: &i32| {
        let o1 = o1.clone();
        async move { o1.lock().unwrap().push(1); }
    });
    let o2 = order.clone();
    let _s2 = bus.subscribe(KEY, move |_: &i32| {
        let o2 = o2.clone();
        async move { o2.lock().unwrap().push(2); }
    });

    bus.post(KEY, 42).await;
    assert_eq!(*order.lock().unwrap(), vec![1, 2], "按订阅顺序执行");
}

#[tokio::test]
async fn unsubscribe_on_drop() {
    let bus = EventBus::new();
    let got = Arc::new(AtomicUsize::new(0));
    let g = got.clone();
    {
        let _sub = bus.subscribe(KEY, move |_: &i32| {
            let g = g.clone();
            async move { g.fetch_add(1, Ordering::SeqCst); }
        });
        bus.post(KEY, 1).await;
        assert_eq!(got.load(Ordering::SeqCst), 1);
    } // Subscription drop → 退订

    bus.post(KEY, 2).await;
    assert_eq!(got.load(Ordering::SeqCst), 1, "drop 后不再收到");
}

#[tokio::test]
async fn post_mut_mutates_event() {
    let bus = EventBus::new();
    let _sub = bus.subscribe_mut(KEY, |e: &mut CounterEvent| {
        e.value += 1;
        async {}
    });
    let _sub2 = bus.subscribe_mut(KEY, |e: &mut CounterEvent| {
        e.value *= 10;
        async {}
    });

    let out = bus.post_mut(KEY, CounterEvent { value: 1, cancelled: false }).await;
    assert_eq!(out.value, 20, "两个 handler 按序改写");
}

#[tokio::test]
async fn post_mut_cancellation_pattern() {
    let bus = EventBus::new();
    let _allow = bus.subscribe_mut(KEY, |e: &mut CounterEvent| {
        if e.value == 7 {
            e.cancelled = true;
        }
        async {}
    });
    let cancelled = bus
        .post_mut(KEY, CounterEvent { value: 7, cancelled: false })
        .await;
    assert!(cancelled.cancelled);
    let kept = bus
        .post_mut(KEY, CounterEvent { value: 8, cancelled: false })
        .await;
    assert!(!kept.cancelled);
}

#[tokio::test]
async fn no_subscribers_is_safe() {
    let bus = EventBus::new();
    // post / post_mut 无订阅者：不 panic，原样返回
    bus.post(KEY, 42i32).await;
    let out = bus.post_mut(KEY, "unchanged".to_string()).await;
    assert_eq!(out, "unchanged");
}

#[tokio::test]
async fn handler_panic_isolated() {
    let bus = EventBus::new();
    let panicked = Arc::new(AtomicUsize::new(0));
    let p = panicked.clone();
    let _bad = bus.subscribe(KEY, move |_: &i32| {
        let p = p.clone();
        async move {
            p.fetch_add(1, Ordering::SeqCst);
            panic!("handler bug");
        }
    });
    let got = Arc::new(AtomicUsize::new(0));
    let g = got.clone();
    let _good = bus.subscribe(KEY, move |_: &i32| {
        let g = g.clone();
        async move { g.fetch_add(1, Ordering::SeqCst); }
    });

    // post 不因 handler panic 而失败
    bus.post(KEY, 1).await;
    assert_eq!(panicked.load(Ordering::SeqCst), 1);
    assert_eq!(got.load(Ordering::SeqCst), 1, "其余 handler 仍执行");
}

#[tokio::test]
async fn post_mut_panic_isolated_but_value_kept() {
    let bus = EventBus::new();
    let _bad = bus.subscribe_mut(KEY, |e: &mut i32| {
        *e += 1;
        async { panic!("mut handler bug"); }
    });
    // post_mut 内部 catch_unwind：panic 被隔离，事件仍返回（此处 handler 在 panic 前已改写）
    let out = bus.post_mut(KEY, 0i32).await;
    assert_eq!(out, 1);
}

#[tokio::test]
async fn different_keys_are_isolated() {
    let bus = EventBus::new();
    let got_str = Arc::new(AtomicUsize::new(0));
    let s = got_str.clone();
    let _s1 = bus.subscribe(KEY, move |_: &String| {
        let s = s.clone();
        async move { s.fetch_add(1, Ordering::SeqCst); }
    });
    let got_int = Arc::new(AtomicUsize::new(0));
    let i = got_int.clone();
    let _s2 = bus.subscribe(KEY2, move |_: &i32| {
        let i = i.clone();
        async move { i.fetch_add(1, Ordering::SeqCst); }
    });
    // 各 key 只发布匹配类型的事件
    bus.post(KEY, "hi".to_string()).await;
    bus.post(KEY2, 5i32).await;
    assert_eq!(got_str.load(Ordering::SeqCst), 1);
    assert_eq!(got_int.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn many_subscribers_high_volume() {
    let bus = EventBus::new();
    const N: usize = 50;
    let mut subs = Vec::new();
    for _ in 0..N {
        let _sub = bus.subscribe(KEY, |_: &u64| async {});
        subs.push(_sub);
    }
    for i in 0..500u64 {
        bus.post(KEY, i).await;
    }
    drop(subs);
    bus.post(KEY, 999u64).await;
}

#[tokio::test]
async fn subscription_lifetime_across_keys() {
    let bus = EventBus::new();
    let got = Arc::new(AtomicUsize::new(0));
    let g1 = got.clone();
    let _s1 = bus.subscribe(KEY, move |_: &i32| {
        let g = g1.clone();
        async move { g.fetch_add(1, Ordering::SeqCst); }
    });
    let g2 = got.clone();
    let _s2 = bus.subscribe(KEY2, move |_: &i32| {
        let g = g2.clone();
        async move { g.fetch_add(10, Ordering::SeqCst); }
    });
    bus.post(KEY, 1).await;
    bus.post(KEY2, 1).await;
    assert_eq!(got.load(Ordering::SeqCst), 11);
}
