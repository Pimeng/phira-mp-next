# 错误提示
ERROR_INVALID_STATE = 你不能在当前状态执行这个操作
ERROR_PERMISSION_DENIED = 你没有权限
ERROR_ROOM_FULL = 房间已满
ERROR_ROOM_LOCKED = 房间已锁定
ERROR_ROOM_NOT_FOUND = 房间不存在
ERROR_ROOM_ALREADY_EXISTS = 房间已存在
ERROR_CHART_NOT_SELECTED = 未选择谱面
ERROR_CHART_NOT_FOUND = 谱面信息获取失败
ERROR_CHAT_NOT_ENABLED = 房间未启用聊天
ERROR_RECORD_NOT_FOUND = 查询记录失败
ERROR_ALREADY_IN_ROOM = 你已经在房间中
ERROR_NOT_IN_ROOM = 你不在房间中
ERROR_NOT_HOST = 你不是房主
ERROR_PLAYER_NOT_FOUND = 玩家不存在
ERROR_SESSION_EXPIRED = 会话已过期
ERROR_AUTHENTICATION_FAILED = 认证失败
ERROR_LOGGED_IN_ELSEWHERE = 账号在其他地方登录
ERROR_PLAYER_ALREADY_ONLINE = 玩家已在线
ERROR_BANNED = 你已被封禁
ERROR_BANNED_FROM_ROOM = 你已被禁止进入该房间
ERROR_PLAYER_NOT_IN_ROOM = 玩家不在该房间

# 系统账号
SYSTEM_LIVE_RECORDER_NAME = 录制状态设置器(请忽略该账号)

# 日志消息（多语言日志系统；LOG_* 键，渲染回退链：指定语言 → 服务器默认 → zh-CN → key）
LOG_LANGUAGE = 服务器语言: { $lang }
LOG_BOOTING = 正在启动服务器...
LOG_INIT_NETWORK = 正在准备偷听...
LOG_LISTENING = 偷听 { $host }:{ $port } 成功
LOG_INIT_HTTP = 正在准备启动 HTTP API 服务...
LOG_DONE = 服务器启动好了！耗时 { $secs }s
LOG_SERVER_RUNNING = Tip: 执行 'stop' 或 Ctrl+C 关掉服务器。
LOG_SHUTTING_DOWN = 正在关闭服务器...
LOG_KICKING_PLAYERS = 正在让 { $count } 名玩家飞起来...
LOG_CLOSING_CHANNELS = 正在让 { $count } 个连接滚出去...
LOG_CHANNELS_CLOSED = 连接已关闭。
LOG_UPTIME = 已运行 { $min } 分 { $sec } 秒
LOG_SHUTDOWN_COMPLETED = 关闭完成，耗时 { $ms }ms。

# ---- 控制台命令（command.rs）----
LOG_CMD_ONLINE_TITLE = 在线玩家 ({ $count }):
LOG_CMD_ONLINE_ITEM =   { $id } ({ $name })
LOG_CMD_ROOMS_TITLE = 房间 ({ $count }):
LOG_CMD_ROOMS_ITEM =   { $id } 状态={ $state } 玩家={ $players } 观战={ $monitors } 锁定={ $locked }
LOG_CMD_HELP_TITLE = 命令:
LOG_CMD_HELP_STOP =   stop                         停止服务器
LOG_CMD_HELP_ONLINE =   online                       列出在线玩家
LOG_CMD_HELP_ROOMS =   rooms                        列出房间
LOG_CMD_HELP_BAN =   ban <userId>                 封禁用户
LOG_CMD_HELP_UNBAN =   unban/pardon <userId>        解除封禁
LOG_CMD_HELP_BANLIST =   banlist                      列出封禁用户
LOG_CMD_HELP_BANROOM =   banroom <userId> <roomId>   封禁用户出房间
LOG_CMD_HELP_UNBANROOM =   unbanroom <userId> <roomId> 解除房间封禁
LOG_CMD_HELP_BROADCAST =   broadcast/say <message>     向所有玩家广播
LOG_CMD_HELP_ROOMSAY =   roomsay <roomId> <message>  向房间发送消息
LOG_CMD_HELP_MAXUSERS =   maxusers <roomId> <count>   设置房间最大人数
LOG_CMD_HELP_NEXTHOST =   nexthost <roomId> <userId>  设置下一房主（循环模式）
LOG_CMD_HELP_LOCK =   lock <roomId> <true|false>   强制锁定/解锁房间
LOG_CMD_HELP_CYCLE =   cycle <roomId> <true|false>  切换循环模式
LOG_CMD_HELP_SETHOST =   sethost <roomId> <userId>   立即转移房主
LOG_CMD_HELP_ROOMINFO =   roominfo <roomId>           显示房间详情
LOG_CMD_USAGE_BAN = 用法: ban <userId>
LOG_CMD_ALREADY_BANNED = 用户 { $id } 已被封禁。
LOG_CMD_BANNED_NAMED = 已封禁 { $id } ({ $name })。
LOG_CMD_BANNED = 已封禁 { $id }。
LOG_CMD_USAGE_UNBAN = 用法: unban <userId>
LOG_CMD_UNBANNED = 已解除封禁 { $id }。
LOG_CMD_NOT_BANNED = 用户 { $id } 未被封禁。
LOG_CMD_USAGE_BANLIST = 用法: banlist
LOG_CMD_NO_BANNED = 没有封禁用户。
LOG_CMD_BANLIST_TITLE = 封禁用户 ({ $count }):
LOG_CMD_BANLIST_ITEM =   { $id } ({ $reason })
LOG_CMD_BANLIST_ITEM_PLAIN =   { $id }
LOG_CMD_USAGE_BANROOM = 用法: banroom <userId> <roomId>
LOG_CMD_REMOVED_FROM_ROOM = 已将 { $id } 移出房间 { $room }。
LOG_CMD_BANNED_FROM_ROOM = 已将 { $id } 从房间 { $room } 封禁。
LOG_CMD_USAGE_UNBANROOM = 用法: unbanroom <userId> <roomId>
LOG_CMD_UNBANNED_FROM_ROOM = 已解除 { $id } 在房间 { $room } 的封禁。
LOG_CMD_NOT_BANNED_FROM_ROOM = 用户 { $id } 未被禁止进入房间 { $room }。
LOG_CMD_USAGE_BROADCAST = 用法: broadcast <message>
LOG_CMD_BROADCAST_SENT = 已向 { $count } 名玩家广播: { $content }
LOG_CMD_USAGE_ROOMSAY = 用法: roomsay <roomId> <message>
LOG_CMD_ROOM_NOT_FOUND = 房间 { $room } 不存在。
LOG_CMD_ROOMSAY_SENT = 已向房间 { $room } 发送消息。
LOG_CMD_ROOMSAY_FAILED = 发送消息失败: { $err }
LOG_CMD_USAGE_MAXUSERS = 用法: maxusers <roomId> <count>
LOG_CMD_INVALID_COUNT = 无效数量: { $count }
LOG_CMD_MAXUSERS_SET = 房间 { $room } 最大人数已设为 { $count }。
LOG_CMD_FAILED = 操作失败: { $err }
LOG_CMD_USAGE_NEXTHOST = 用法: nexthost <roomId> <userId>
LOG_CMD_INVALID_USER_ID = 无效 userId: { $id }
LOG_CMD_NEXTHOST_NOT_CYCLE = 房间 { $room } 未启用循环模式；启用后下一房主生效。
LOG_CMD_NEXTHOST_SET = 房间 { $room } 的下一房主已设为 { $id }。
LOG_CMD_USAGE_LOCK = 用法: lock <roomId> <true|false>
LOG_CMD_INVALID_BOOL = 无效布尔值: { $value }（使用 true|false）
LOG_CMD_LOCK_SET = 房间 { $room } 锁定 = { $value }。
LOG_CMD_USAGE_CYCLE = 用法: cycle <roomId> <true|false>
LOG_CMD_CYCLE_SET = 房间 { $room } 循环 = { $value }。
LOG_CMD_USAGE_SETHOST = 用法: sethost <roomId> <userId>
LOG_CMD_HOST_TRANSFERRED = 房间 { $room } 房主已转移给 { $id }。
LOG_CMD_USAGE_ROOMINFO = 用法: roominfo <roomId>
LOG_CMD_ROOMINFO_TITLE = 房间 { $room }:
LOG_CMD_ROOMINFO_STATE =   状态: { $state }
LOG_CMD_ROOMINFO_LOCKED =   锁定: { $locked }
LOG_CMD_ROOMINFO_CYCLE =   循环: { $cycle }
LOG_CMD_ROOMINFO_CHAT =   聊天: { $chat }
LOG_CMD_ROOMINFO_MAX =   最大人数: { $count }
LOG_CMD_ROOMINFO_HOST =   房主: { $host }
LOG_CMD_ROOMINFO_CHART =   谱面: { $chart }
LOG_CMD_ROOMINFO_PLAYERS =   玩家 ({ $count }):
LOG_CMD_ROOMINFO_PLAYER_ITEM =     { $id } ({ $name })
LOG_CMD_ROOMINFO_MONITORS =   观战 ({ $count }):
LOG_CMD_NONE = none
LOG_CMD_UNKNOWN = 未知命令: { $cmd }

# ---- HTTP（http.rs）----
LOG_HTTP_LISTENING = HTTP API 正在偷听 { $addr }
LOG_HTTP_ERROR = HTTP 服务器错误: { $err }

# ---- 网络连接（network/connection.rs）----
LOG_CONN_NEW = 新连接: { $peer }
LOG_CONN_PROXY_FAILED = Proxy 协议解析失败: { $err }
LOG_CONN_HANDSHAKE_FAILED = 握手失败: { $err }
LOG_CONN_BAD_VERSION = 协议版本错误: { $version }
LOG_CONN_WRITER_FRAME = Writer 帧: { $bytes } 字节
LOG_CONN_WRITER_SHARED = Writer 共享帧: { $bytes } 字节
LOG_CONN_FRAME_ERROR = 帧错误: { $err }
LOG_CONN_READ_ERROR = 读取错误: { $err }
LOG_CONN_CLOSED = 连接已关闭: { $reason }
LOG_CONN_DECODE_ERROR = 数据包解码错误: { $err }
LOG_CONN_SESSION_SUSPENDED = 会话已挂起 (用户 { $id })

# ---- 认证（network/authenticate_handler.rs）----
LOG_AUTH_LOGGED_IN = { $peer } [{ $id }] { $name } 加入了服务器

# ---- 对局阶段（network/play_handler.rs）----
LOG_PLAY_CREATOR_JOIN_FAILED = 创建者加入房间失败 (房间 { $room }): { $err }
LOG_PLAY_UNEXPECTED_PACKET = 对局阶段收到意外数据包 (用户 { $id })

# ---- 房间阶段（network/room_handler.rs）----
LOG_ROOM_UNEXPECTED_PACKET = 房间阶段收到意外数据包，踢出 (用户 { $id })
LOG_ROOM_RECORD_WRITE_FAILED = 录制写入失败: { $err }

# ---- 玩家注册（player.rs）----
LOG_PLAYER_CHECK_SUSPENDED = Resolve 或恢复: 检查挂起会话 (用户 { $id })
LOG_PLAYER_TAKE_SUSPENDED = Resolve 或恢复: 取出挂起会话 (用户 { $id })
LOG_PLAYER_FILTER_CHECK = Resolve 或恢复: 过滤检查 (用户 { $id })

# ---- Phira API（phira.rs）----
LOG_PHIRA_FETCH_OK = Phira 获取成功: { $path } ({ $status })
LOG_PHIRA_FETCH_NON_2XX = Phira 获取非 2xx: { $path } ({ $err })
LOG_PHIRA_FETCH_TRANSPORT = Phira 获取传输错误: { $path } ({ $err })
LOG_PHIRA_FETCH_RETRY = Phira 获取重试: { $path } (第 { $attempt } 次)

# ---- 会话（session.rs）----
LOG_SESSION_TIMEOUT = 会话超时，正在让 { $id } 滚出去

# ---- 服务器（server.rs）----
LOG_SERVER_ACCEPT_ERROR = 接受连接错误: { $err }
