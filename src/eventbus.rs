//! 简化事件总线（第 8 节缩减版）。
//!
//! 原 Java 版使用 Orbit EventBus + 插件系统；本项目按要求**不实现插件系统**，
//! 仅保留一个轻量发布-订阅总线供内部/测试使用。

use std::any::Any;
use std::collections::HashMap;
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::sync::{Arc, RwLock};

type Handler = Arc<dyn Fn(&dyn Any) + Send + Sync>;

#[derive(Default)]
pub struct EventBus {
    handlers: RwLock<HashMap<&'static str, Vec<Handler>>>,
}

impl EventBus {
    pub fn new() -> Self {
        Self::default()
    }

    /// 订阅事件（key 为事件名，如 "player.disconnect"）。
    pub fn subscribe<F, T>(&self, key: &'static str, f: F)
    where
        F: Fn(&T) + Send + Sync + 'static,
        T: Any + Send + Sync + 'static,
    {
        let wrapper: Handler = Arc::new(move |any| {
            if let Some(ev) = any.downcast_ref::<T>() {
                f(ev);
            }
        });
        self.handlers.write().unwrap().entry(key).or_default().push(wrapper);
    }

    /// 发布事件。handler panic 不影响其他订阅者。
    pub fn post<T: Any + Send + Sync>(&self, key: &'static str, event: &T) {
        let handlers = match self.handlers.read() {
            Ok(map) => map.get(key).cloned().unwrap_or_default(),
            Err(_) => return,
        };
        for h in handlers {
            let _ = catch_unwind(AssertUnwindSafe(|| h(event as &dyn Any)));
        }
    }
}
