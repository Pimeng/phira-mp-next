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

## 配置

服务器启动时自动从工作目录加载 `config.yml`（不存在则使用内置默认值）。
可复制 [config.example.yml](./config.example.yml) 作为起点：

```bash
cp config.example.yml config.yml
```

**加载优先级：命令行参数 > config.yml > 内置默认值**（CLI 显式指定的参数会覆盖配置文件）。

```yaml
server:
  host: 0.0.0.0           # 监听地址
  port: 12346             # 游戏端口
  http_port: 12347        # HTTP 查询 API 端口（0 = 禁用）
  proxy_protocol: false   # 启用 HAProxy PROXY 协议
  language: zh-CN         # 服务器默认语言
  session_timeout: 300    # 会话挂起超时（秒）
  record_dir: null        # 对局录制输出目录（null = 禁用）

phira:
  api: "https://phira.5wyxi.com/"   # Phira API Base URL
  max_attempts: 5                   # GET 重试次数
  retry_base_ms: 150                # 重试退避基数（毫秒）
  cache:                            # 各缓存 TTL（秒）/ 容量
    token_ttl: 600
    token_cap: 10000
    user_ttl: 600
    user_cap: 5000
    chart_ttl: 1800
    chart_cap: 10000
    record_ttl: 1800
    record_cap: 50000

network:
  handshake_timeout: 5   # 握手超时（秒）
  read_timeout: 5        # 读帧超时（秒）
  proxy_timeout: 5       # PROXY 协议解析超时（秒）

room:
  default_max_player: 8  # 新建房间默认最大人数
```

## 参数

| 参数 | 默认值 | 说明 |
| --- | --- | --- |
| `--config` | `config.yml` | 配置文件路径（自动加载） |
| `--port` | `12346` | 偷听端口 |
| `--host` | `0.0.0.0` | 绑定地址 |
| `--http-port` | `12347` | HTTP 查询 API 端口（0 = 禁用） |
| `--language` | `zh-CN` | 默认玩家语言 |
| `--proxy-protocol` | `false` | 启用 HAProxy PROXY 协议 |
| `--session-timeout` | `300` | 会话挂起超时（秒） |
| `--phira-api` | `https://phira.5wyxi.com/` | Phira API 地址 |
| `--record-dir` | — | 对局录制输出目录 |
| `--phira-max-attempts` | `5` | Phira API GET 重试次数 |
| `--phira-retry-base-ms` | `150` | 重试退避基数（毫秒） |
| `--phira-token-cache-ttl/cap` | `600` / `10000` | token 缓存 TTL（秒）/容量 |
| `--phira-user-cache-ttl/cap` | `600` / `5000` | 用户缓存 TTL（秒）/容量 |
| `--phira-chart-cache-ttl/cap` | `1800` / `10000` | 谱面缓存 TTL（秒）/容量 |
| `--phira-record-cache-ttl/cap` | `1800` / `50000` | 成绩缓存 TTL（秒）/容量 |
| `--handshake-timeout` | `5` | 握手超时（秒） |
| `--read-timeout` | `5` | 读帧超时（秒） |
| `--proxy-timeout` | `5` | PROXY 协议解析超时（秒） |
| `--default-max-player` | `8` | 新建房间默认最大人数 |

## 测试

```bash
cargo test
```

## 致谢

该项目由 lRENyaaa 的 jphira-mp 项目移植而来，感谢其提供的协议实现与思路。

- [lRENyaaa](https://github.com/lRENyaaa)
    - [jphira-mp-protocol](https://github.com/lRENyaaa/jphira-mp-protocol)
    - [jphira-mp](https://github.com/lRENyaaa/jphira-mp)