//! 异步事件总线（对应 Java 版 PluginEventBus / Orbit EventBus）。
//!
//! - handler 返回 `BoxFuture`，可在事件处理中 `await`。
//! - 订阅返回 `Subscription` 句柄，drop 即退订。
//! - `post` 只读派发；`post_mut` 对应 Java 的 CancellableEvent / 可改写内容事件。

use std::any::Any;
use std::collections::HashMap;
use std::panic::AssertUnwindSafe;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, RwLock, Weak};

/// 异步事件处理器。
pub type EventHandler =
    Arc<dyn Fn(Arc<dyn Any + Send + Sync>) -> futures::future::BoxFuture<'static, ()> + Send + Sync>;

struct Entry {
    id: u64,
    expect: &'static str, // type_name::<T>()
    handler: EventHandler,
}

#[derive(Default)]
struct Registry {
    handlers: RwLock<HashMap<&'static str, Vec<Entry>>>,
}

#[derive(Clone)]
pub struct EventBus {
    reg: Arc<Registry>,
}

/// 订阅句柄；drop 时自动退订。
pub struct Subscription {
    key: &'static str,
    id: u64,
    reg: Weak<Registry>,
}

impl Drop for Subscription {
    fn drop(&mut self) {
        if let Some(reg) = self.reg.upgrade()
            && let Some(list) = reg.handlers.write().unwrap().get_mut(self.key) {
                list.retain(|e| e.id != self.id);
            }
    }
}

static NEXT_SUB_ID: AtomicU64 = AtomicU64::new(1);

impl Default for EventBus {
    fn default() -> Self {
        Self::new()
    }
}

impl EventBus {
    pub fn new() -> Self {
        Self {
            reg: Arc::new(Registry::default()),
        }
    }

    /// 订阅事件。`key` 为事件名（如 `events::ROOM_CHAT`）。
    pub fn subscribe<F, Fut, T>(&self, key: &'static str, f: F) -> Subscription
    where
        F: Fn(&T) -> Fut + Send + Sync + 'static,
        Fut: std::future::Future<Output = ()> + Send + 'static,
        T: Any + Send + Sync + 'static,
    {
        let f = Arc::new(f);
        let handler: EventHandler = Arc::new(move |any| {
            let f = f.clone();
            match any.downcast::<T>() {
                Ok(ev) => Box::pin(async move { f(&ev).await }),
                Err(_) => Box::pin(async {}),
            }
        });
        self.register(key, std::any::type_name::<T>(), handler)
    }

    /// 订阅可变事件：handler 直接拿到 `&mut T`（内部经 Mutex 串行化），
    /// 可设置取消标记 / 改写内容（对应 Java 的 CancellableEvent 偷听器）。
    pub fn subscribe_mut<F, Fut, T>(&self, key: &'static str, f: F) -> Subscription
    where
        F: Fn(&mut T) -> Fut + Send + Sync + 'static,
        Fut: std::future::Future<Output = ()> + Send + 'static,
        T: Any + Send + Sync + 'static,
    {
        let f = Arc::new(f);
        let handler: EventHandler = Arc::new(move |any| {
            let f = f.clone();
            match any.downcast::<tokio::sync::Mutex<T>>() {
                Ok(m) => Box::pin(async move { f(&mut *m.lock().await).await }),
                Err(_) => Box::pin(async {}),
            }
        });
        self.register(key, std::any::type_name::<T>(), handler)
    }

    fn register(&self, key: &'static str, expect: &'static str, handler: EventHandler) -> Subscription {
        let id = NEXT_SUB_ID.fetch_add(1, Ordering::Relaxed);
        self.reg
            .handlers
            .write()
            .unwrap()
            .entry(key)
            .or_default()
            .push(Entry { id, expect, handler });
        Subscription {
            key,
            id,
            reg: Arc::downgrade(&self.reg),
        }
    }

    /// 发布事件（异步派发，按订阅顺序执行；handler panic 被隔离）。
    pub async fn post<T: Any + Send + Sync>(&self, key: &'static str, event: T) {
        let handlers = self.snapshot(key);
        if handlers.is_empty() {
            return;
        }
        let expect = std::any::type_name::<T>();
        let event: Arc<dyn Any + Send + Sync> = Arc::new(event);
        for h in handlers {
            debug_assert_eq!(h.expect, expect, "event key/type mismatch: {key}");
            let _ = invoke(&h.handler, event.clone()).await;
        }
    }

    /// 发布可变事件（对应 Java 的 CancellableEvent / 可改写内容事件）。
    ///
    /// 所有订阅者按序执行后取回最终值（含取消标记 / 改写内容）。
    pub async fn post_mut<T: Any + Send + Sync>(&self, key: &'static str, event: T) -> T {
        let handlers = self.snapshot(key);
        if handlers.is_empty() {
            return event;
        }
        let expect = std::any::type_name::<T>();
        let event = Arc::new(tokio::sync::Mutex::new(event));
        for h in handlers {
            debug_assert_eq!(h.expect, expect, "event key/type mismatch: {key}");
            let any: Arc<dyn Any + Send + Sync> = event.clone();
            let _ = invoke(&h.handler, any).await;
        }
        match Arc::try_unwrap(event) {
            Ok(m) => m.into_inner(),
            Err(_) => panic!("event leaked by handler: {key}"),
        }
    }

    fn snapshot(&self, key: &'static str) -> Vec<EntryRef> {
        self.reg
            .handlers
            .read()
            .map(|m| {
                m.get(key)
                    .map(|v| {
                        v.iter()
                            .map(|e| EntryRef {
                                expect: e.expect,
                                handler: e.handler.clone(),
                            })
                            .collect()
                    })
                    .unwrap_or_default()
            })
            .unwrap_or_default()
    }
}

/// 订阅项的轻量克隆（读锁外执行用）。
struct EntryRef {
    expect: &'static str,
    handler: EventHandler,
}

async fn invoke(h: &EventHandler, event: Arc<dyn Any + Send + Sync>) -> Result<(), ()> {
    use futures::FutureExt;
    let fut = h(event);
    // 捕获 handler 异步体内部的 panic（catch_unwind 必须包住 await，而非仅包调用）。
    AssertUnwindSafe(fut).catch_unwind().await.map_err(|_| ())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering as AOrd};

    #[tokio::test]
    async fn post_dispatches_in_order() {
        let bus = EventBus::new();
        let log = Arc::new(std::sync::Mutex::new(Vec::new()));
        let l1 = log.clone();
        let _s1 = bus.subscribe("ev", move |v: &i32| {
            let l = l1.clone();
            let v = *v;
            async move { l.lock().unwrap().push(("a", v)); }
        });
        let l2 = log.clone();
        let _s2 = bus.subscribe("ev", move |v: &i32| {
            let l = l2.clone();
            let v = *v;
            async move { l.lock().unwrap().push(("b", v)); }
        });
        bus.post("ev", 42).await;
        assert_eq!(*log.lock().unwrap(), vec![("a", 42), ("b", 42)]);
    }

    #[tokio::test]
    async fn post_mut_allows_mutation() {
        let bus = EventBus::new();
        let _s = bus.subscribe_mut("ev", |v: &mut i32| {
            *v += 10;
            async {}
        });
        let out = bus.post_mut("ev", 5).await;
        assert_eq!(out, 15);
    }

    #[tokio::test]
    async fn cancellable_event_flow() {
        // 用独立测试事件验证取消/改写机制（不依赖 Arc<dyn Room>/Player 构造）。
        struct TestCancellable {
            message: String,
            cancel_reason: Option<String>,
        }
        impl TestCancellable {
            fn is_cancelled(&self) -> bool { self.cancel_reason.is_some() }
            fn cancel(&mut self, r: impl Into<String>) { self.cancel_reason = Some(r.into()); }
        }
        let bus = EventBus::new();
        let _s = bus.subscribe_mut("ev", |ev: &mut TestCancellable| {
            ev.message = "rewritten".to_string();
            ev.cancel("spam");
            async {}
        });
        let out = bus
            .post_mut("ev", TestCancellable { message: "orig".to_string(), cancel_reason: None })
            .await;
        assert!(out.is_cancelled());
        assert_eq!(out.cancel_reason.as_deref(), Some("spam"));
        assert_eq!(out.message, "rewritten");
    }

    #[tokio::test]
    async fn unsubscribe_on_drop() {
        let bus = EventBus::new();
        let count = Arc::new(AtomicUsize::new(0));
        let c = count.clone();
        let sub = bus.subscribe("ev", move |_: &i32| {
            let c = c.clone();
            async move { c.fetch_add(1, AOrd::SeqCst); }
        });
        bus.post("ev", 1).await;
        assert_eq!(count.load(AOrd::SeqCst), 1);
        drop(sub);
        bus.post("ev", 1).await;
        assert_eq!(count.load(AOrd::SeqCst), 1, "dropped subscription should not fire");
    }

    #[tokio::test]
    async fn handler_panic_isolated() {
        let bus = EventBus::new();
        let count = Arc::new(AtomicUsize::new(0));
        let _s1 = bus.subscribe("ev", |_: &i32| async move { panic!("boom") });
        let c = count.clone();
        let _s2 = bus.subscribe("ev", move |_: &i32| {
            let c = c.clone();
            async move { c.fetch_add(1, AOrd::SeqCst); }
        });
        bus.post("ev", 1).await; // 不应 panic 传播
        assert_eq!(count.load(AOrd::SeqCst), 1, "later handler should still run");
    }

    #[tokio::test]
    async fn no_subscribers_is_noop() {
        let bus = EventBus::new();
        bus.post("nothing", 1).await;
        let out = bus.post_mut("nothing", 7).await;
        assert_eq!(out, 7);
    }
}
