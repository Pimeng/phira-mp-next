//! 服务端装配与生命周期。
//!
//! - `args`：命令行参数与配置加载。
//! - `context`：共享依赖、全局上下文与关闭信号。
//! - `extensions`：可替换的连接处理和玩家解析入口。
//! - `runtime`：启动监听、接受连接与优雅关闭。
//!
//! 断线清理由 [`crate::network::connection`] 负责，
//! 会话挂起与恢复由 [`crate::session::SessionManager`] 负责。
//! 本模块重导出原有公开接口，调用方仍使用 `server::ServerArgs` 等路径。

mod args;
mod context;
mod extensions;
mod runtime;

pub use args::ServerArgs;
pub(crate) use context::with_server_ctx;
pub use context::{ServerContext, global_ctx, test_global_ctx, test_listen_addr};
pub use extensions::{DisconnectHandler, Extensions, PlayerResolver};
pub use runtime::run;
