# phira-mp-next

Phira 多人联机服务端

## 功能

- 完整协议实现（握手 / VarInt 帧 / 36 个包 + 16 种 Message + 3 种 GameState）
- 认证（Phira API `/me` 集成，含缓存与重试）
- 房间状态机（SelectChart → WaitForReady → Playing）
- 会话挂起 / 恢复（掉线不掉房，默认 5 分钟超时）
- 对局录制（`.phirarec`，可选）
- HAProxy PROXY 协议支持（可选）
- 控制台命令（`stop` / `online` / `rooms`）

## 运行

```bash
cargo run --release -- --port 12346
```

## 构建

```bash
cargo build --release
```

## 参数

| 参数 | 默认值 | 说明 |
| --- | --- | --- |
| `--port` | `12346` | 偷听端口 |
| `--host` | `0.0.0.0` | 绑定地址 |
| `--language` | `zh-CN` | 默认玩家语言 |
| `--proxy-protocol` | `false` | 启用 HAProxy PROXY 协议 |
| `--session-timeout` | `300` | 会话挂起超时（秒） |
| `--phira-api` | `https://phira.5wyxi.com/` | Phira API 地址 |
| `--record-dir` | — | 对局录制输出目录 |

## 测试

```bash
cargo test
```

## 致谢

该项目由 lRENyaaa 的 jphira-mp 项目移植而来，感谢其提供的协议实现与思路。

- [lRENyaaa](https://github.com/lRENyaaa)
    - [jphira-mp-protocol](https://github.com/lRENyaaa/jphira-mp-protocol)
    - [jphira-mp](https://github.com/lRENyaaa/jphira-mp)