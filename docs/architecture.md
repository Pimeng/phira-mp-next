# 代码结构与阅读入口

项目以 Rust library 承载服务端逻辑，`src/main.rs` 只负责日志初始化、参数加载与启动。
`terminal-console` 是独立的终端库，负责输入编辑和日志渲染。

## 从一条连接开始读

1. `server/runtime.rs`：`run` 按顺序启动服务、注册关闭信号、接受连接、执行关闭流程。
2. `network/connection.rs`：处理连接生命周期、帧读写、超时和断线清理。
3. `network/authenticate_handler.rs`：认证玩家并创建或恢复会话。
4. `network/play_handler.rs`：处理大厅中的创建房间、加入房间等请求。
5. `network/room_handler/mod.rs`：分发房间内的包，具体操作交给同目录的职责模块。
6. `room/local.rs` 与 `room/behavior.rs`：在锁内完成领域操作，返回需要发送的广播计划。

`PacketHandler` 的返回值决定当前连接继续处理、切换处理器或关闭。
身份和房间通过 `Player`、`Room` trait 传递；协议层不要求所有实现都持有本地连接。

## 模块边界

| 模块 | 文件 | 负责的内容 |
| --- | --- | --- |
| 服务端 | `server/args.rs` | CLI 定义、默认值与配置文件加载入口 |
| 服务端 | `server/context.rs` | 共享依赖、全局上下文、关闭通知 |
| 服务端 | `server/extensions.rs` | 初始处理器、断线清理、玩家解析的扩展点 |
| 服务端 | `server/runtime.rs` | TCP/HTTP 启动、连接接收、优雅关闭 |
| 玩家 | `player/mod.rs` | `Player` 接口、扩展数据与公开导出 |
| 玩家 | `player/local.rs` | 本地玩家、可换绑连接及本地类型转换 |
| 玩家 | `player/registry.rs` | 玩家唯一性、创建、重连与注册清理 |
| 玩家 | `player/providers.rs` | 认证、谱面、成绩数据源的替换入口 |
| 房间 | `room/mod.rs` | `Room` 接口、结果类型、房间注册与广播发送 |
| 房间 | `room/local.rs` | 默认实现、查询快照和领域操作入口 |
| 房间 | `room/local/membership.rs` | 入退房、房主转移和空房销毁 |
| 房间 | `room/behavior.rs`、`room/state.rs` | 设置操作、对局状态变化和广播计划 |
| 房间协议 | `network/room_handler/mod.rs` | 包分发、操作结果回复、状态事件 |
| 房间协议 | `controls.rs`、`chart.rs` | 聊天、离房、房间设置与选曲 |
| 房间协议 | `gameplay.rs`、`results.rs` | 开局准备、中止、成绩提交与录制调度 |
| 命令 | `command/console.rs` | 终端适配、停止服务与未知命令事件 |
| 命令 | `command/mod.rs`、`command/parsing.rs` | 命令分发、参数解析与共享查找 |
| 命令 | `status.rs`、`moderation.rs` | 查询、帮助与封禁管理 |
| 命令 | `messaging.rs`、`rooms.rs` | 消息广播与房间管理 |

较小的独立职责继续保留单文件：`config.rs` 合并 YAML 与 CLI，`session.rs` 管理挂起会话，
`http.rs` 提供查询接口，`phira.rs` 处理上游 API，`record.rs` 写入录制文件，
`eventbus.rs` / `events.rs` 定义扩展事件，`i18n.rs` / `log.rs` 负责多语言消息和日志。
`packet/`、`bytes.rs`、`frame.rs`、`float16.rs` 负责协议编解码。

目录中的实现模块默认保持私有，通过 `mod.rs` 暴露稳定入口。
例如 `server::ServerArgs`、`player::LocalPlayer`、`command::process_command`、
`network::room_handler::RoomHandler` 的使用路径不随内部文件拆分改变。

## 修改时需要保留的约定

- **锁内修改状态，锁外发送消息**：房间方法返回 `Broadcast` 计划；网络处理器在方法返回后回复请求并发送广播，不能在持有房间互斥锁时执行网络 I/O。
- **事件是扩展接口**：前置事件允许取消或改写操作，后置事件报告结果。调整处理器时保留回复、广播和事件的先后顺序；不要顺手改变某些失败路径仍触发后置事件的既有行为。
- **掉线不等于退房**：`SessionManager` 保存挂起的房间上下文，重连时注册表恢复它；旧连接的清理不能移除已经绑定新连接的玩家。
- **连接与身份分离**：通用操作面向 `Player`；只有本地连接专属操作使用 `LocalPlayer` / `ConnectionReference`。
- **命令入口分工明确**：`process_command` 返回是否识别命令，参数错误仍算已处理；`stop` 和未知命令事件由终端适配层负责。消息正文保留内部空白，不能通过参数数组重新拼接。
- **全局上下文仍是单例**：本次整理不改变该约束。启动真实服务端的同一测试二进制应持有 `tests/common::SERIAL` 锁。

新增功能先判断属于协议处理、领域操作还是基础设施，再放到对应模块。
不要把网络请求、状态修改和终端输出都塞进一个处理函数；也无需为简单操作增加新的抽象层。

## 测试与验证

```bash
cargo fmt --all -- --check
cargo clippy --locked --all-targets --all-features -- -D warnings
cargo test --locked --all-targets --all-features
```

| 修改范围 | 优先阅读 / 运行的测试 |
| --- | --- |
| 命令 | `command::tests`、`tests/command_test.rs` |
| 房间领域 | `room::local::tests`、`tests/room_state_machine_test.rs` |
| 房间协议与连接 | `tests/smoke_test.rs`、`tests/server_extreme_test.rs` |
| 玩家与会话恢复 | `tests/session_manager_test.rs`、`tests/server_extreme_test.rs` |
| 编解码 | `tests/protocol_extreme_test.rs` |
| HTTP / 上游 / 录制 | `tests/http_rooms_test.rs`、`tests/phira_fetcher_test.rs`、`tests/record_file_test.rs` |

`room/local/tests/` 按成员、设置、对局、管理操作组织原有单元测试，共享辅助放在其 `mod.rs` 中。
Nightly Build 在生成发布二进制前运行格式、Clippy、编译与全部主 crate 测试，检查任务最长运行 10 分钟。
