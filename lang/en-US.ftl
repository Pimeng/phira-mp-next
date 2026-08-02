# Error messages
ERROR_INVALID_STATE = You cannot perform this action in current state
ERROR_PERMISSION_DENIED = Permission denied
ERROR_ROOM_FULL = Room is full
ERROR_ROOM_LOCKED = Room is locked
ERROR_ROOM_NOT_FOUND = Room not found
ERROR_ROOM_ALREADY_EXISTS = Room already exists
ERROR_CHART_NOT_SELECTED = Chart not selected
ERROR_CHART_NOT_FOUND = Failed to get chart information
ERROR_CHAT_NOT_ENABLED = Chat is not enabled in this room
ERROR_RECORD_NOT_FOUND = Failed to query record
ERROR_ALREADY_IN_ROOM = You are already in a room
ERROR_NOT_IN_ROOM = You are not in a room
ERROR_NOT_HOST = You are not the host
ERROR_PLAYER_NOT_FOUND = Player not found
ERROR_SESSION_EXPIRED = Session expired
ERROR_AUTHENTICATION_FAILED = Authentication failed
ERROR_LOGGED_IN_ELSEWHERE = Account logged in from another location
ERROR_PLAYER_ALREADY_ONLINE = Player is already online
ERROR_BANNED = You have been banned
ERROR_BANNED_FROM_ROOM = You are banned from this room
ERROR_PLAYER_NOT_IN_ROOM = Player is not in this room

# System accounts
SYSTEM_LIVE_RECORDER_NAME = Live Recorder (Please ignore this account)

# Log messages (multi-language log system; LOG_* keys, fallback: specified → server default → zh-CN → key)
LOG_LANGUAGE = Server language: { $lang }
LOG_BOOTING = Booting up Phira Server...
LOG_INIT_NETWORK = Initializing network...
LOG_LISTENING = Listening on { $host }:{ $port }
LOG_INIT_HTTP = Initializing HTTP API...
LOG_DONE = Done ({ $secs }s)!
LOG_SERVER_RUNNING = Server is running. Type 'stop' to stop.
LOG_SHUTTING_DOWN = Shutting down...
LOG_KICKING_PLAYERS = Kicking { $count } player(s)...
LOG_CLOSING_CHANNELS = Closing { $count } channel(s)...
LOG_CHANNELS_CLOSED = Channels closed.
LOG_UPTIME = Uptime: { $min }m { $sec }s
LOG_SHUTDOWN_COMPLETED = Shutdown completed in { $ms }ms. Goodbye!

# ---- Console commands (command.rs) ----
LOG_CMD_ONLINE_TITLE = Online players ({ $count }):
LOG_CMD_ONLINE_ITEM =   { $id } ({ $name })
LOG_CMD_ROOMS_TITLE = Rooms ({ $count }):
LOG_CMD_ROOMS_ITEM =   { $id } state={ $state } players={ $players } monitors={ $monitors } locked={ $locked }
LOG_CMD_HELP_TITLE = Commands:
LOG_CMD_HELP_STOP =   stop                         Stop the server
LOG_CMD_HELP_ONLINE =   online                       List online players
LOG_CMD_HELP_ROOMS =   rooms                        List rooms
LOG_CMD_HELP_BAN =   ban <userId>                 Ban a user
LOG_CMD_HELP_UNBAN =   unban/pardon <userId>        Unban a user
LOG_CMD_HELP_BANLIST =   banlist                      List banned users
LOG_CMD_HELP_BANROOM =   banroom <userId> <roomId>    Ban a user from a room
LOG_CMD_HELP_UNBANROOM =   unbanroom <userId> <roomId>  Unban a user from a room
LOG_CMD_HELP_BROADCAST =   broadcast/say <message>      Broadcast to all players
LOG_CMD_HELP_ROOMSAY =   roomsay <roomId> <message>   Send a message to a room
LOG_CMD_HELP_MAXUSERS =   maxusers <roomId> <count>    Set room max players
LOG_CMD_HELP_NEXTHOST =   nexthost <roomId> <userId>   Set next host (cycle mode only)
LOG_CMD_HELP_LOCK =   lock <roomId> <true|false>   Force lock/unlock a room
LOG_CMD_HELP_CYCLE =   cycle <roomId> <true|false>  Toggle room cycle mode
LOG_CMD_HELP_SETHOST =   sethost <roomId> <userId>    Transfer host immediately
LOG_CMD_HELP_ROOMINFO =   roominfo <roomId>            Show room details
LOG_CMD_USAGE_BAN = Usage: ban <userId>
LOG_CMD_ALREADY_BANNED = User { $id } is already banned.
LOG_CMD_BANNED_NAMED = Banned { $id } ({ $name }).
LOG_CMD_BANNED = Banned { $id }.
LOG_CMD_USAGE_UNBAN = Usage: unban <userId>
LOG_CMD_UNBANNED = Unbanned { $id }.
LOG_CMD_NOT_BANNED = User { $id } is not banned.
LOG_CMD_USAGE_BANLIST = Usage: banlist
LOG_CMD_NO_BANNED = No banned users.
LOG_CMD_BANLIST_TITLE = Banned users ({ $count }):
LOG_CMD_BANLIST_ITEM =   { $id } ({ $reason })
LOG_CMD_BANLIST_ITEM_PLAIN =   { $id }
LOG_CMD_USAGE_BANROOM = Usage: banroom <userId> <roomId>
LOG_CMD_REMOVED_FROM_ROOM = Removed { $id } from room { $room }.
LOG_CMD_BANNED_FROM_ROOM = Banned { $id } from room { $room }.
LOG_CMD_USAGE_UNBANROOM = Usage: unbanroom <userId> <roomId>
LOG_CMD_UNBANNED_FROM_ROOM = Unbanned { $id } from room { $room }.
LOG_CMD_NOT_BANNED_FROM_ROOM = User { $id } is not banned from room { $room }.
LOG_CMD_USAGE_BROADCAST = Usage: broadcast <message>
LOG_CMD_BROADCAST_SENT = Broadcast to { $count } player(s): { $content }
LOG_CMD_USAGE_ROOMSAY = Usage: roomsay <roomId> <message>
LOG_CMD_ROOM_NOT_FOUND = Room { $room } not found.
LOG_CMD_ROOMSAY_SENT = Sent message to room { $room }.
LOG_CMD_ROOMSAY_FAILED = Failed to send message: { $err }
LOG_CMD_USAGE_MAXUSERS = Usage: maxusers <roomId> <count>
LOG_CMD_INVALID_COUNT = Invalid count: { $count }
LOG_CMD_MAXUSERS_SET = Room { $room } max players set to { $count }.
LOG_CMD_FAILED = Failed: { $err }
LOG_CMD_USAGE_NEXTHOST = Usage: nexthost <roomId> <userId>
LOG_CMD_INVALID_USER_ID = Invalid userId: { $id }
LOG_CMD_NEXTHOST_NOT_CYCLE = Room { $room } is not in cycle mode; next host takes effect once cycle is enabled.
LOG_CMD_NEXTHOST_SET = Next host of room { $room } set to { $id }.
LOG_CMD_USAGE_LOCK = Usage: lock <roomId> <true|false>
LOG_CMD_INVALID_BOOL = Invalid boolean: { $value } (use true|false)
LOG_CMD_LOCK_SET = Room { $room } locked = { $value }.
LOG_CMD_USAGE_CYCLE = Usage: cycle <roomId> <true|false>
LOG_CMD_CYCLE_SET = Room { $room } cycle = { $value }.
LOG_CMD_USAGE_SETHOST = Usage: sethost <roomId> <userId>
LOG_CMD_HOST_TRANSFERRED = Room { $room } host transferred to { $id }.
LOG_CMD_USAGE_ROOMINFO = Usage: roominfo <roomId>
LOG_CMD_ROOMINFO_TITLE = Room { $room }:
LOG_CMD_ROOMINFO_STATE =   State: { $state }
LOG_CMD_ROOMINFO_LOCKED =   Locked: { $locked }
LOG_CMD_ROOMINFO_CYCLE =   Cycle: { $cycle }
LOG_CMD_ROOMINFO_CHAT =   Chat: { $chat }
LOG_CMD_ROOMINFO_MAX =   Max players: { $count }
LOG_CMD_ROOMINFO_HOST =   Host: { $host }
LOG_CMD_ROOMINFO_CHART =   Chart: { $chart }
LOG_CMD_ROOMINFO_PLAYERS =   Players ({ $count }):
LOG_CMD_ROOMINFO_PLAYER_ITEM =     { $id } ({ $name })
LOG_CMD_ROOMINFO_MONITORS =   Monitors ({ $count }):
LOG_CMD_NONE = none
LOG_CMD_UNKNOWN = Unknown command: { $cmd }

# ---- HTTP (http.rs) ----
LOG_HTTP_LISTENING = HTTP API listening on { $addr } (GET /api/rooms)
LOG_HTTP_ERROR = HTTP server error: { $err }

# ---- Network connection (network/connection.rs) ----
LOG_CONN_NEW = New connection: { $peer }
LOG_CONN_PROXY_FAILED = Proxy protocol failed: { $err }
LOG_CONN_HANDSHAKE_FAILED = Handshake failed: { $err }
LOG_CONN_BAD_VERSION = Bad protocol version: { $version }
LOG_CONN_WRITER_FRAME = Writer frame: { $bytes } bytes
LOG_CONN_WRITER_SHARED = Writer shared frame: { $bytes } bytes
LOG_CONN_FRAME_ERROR = Frame error: { $err }
LOG_CONN_READ_ERROR = Read error: { $err }
LOG_CONN_CLOSED = Connection closed: { $reason }
LOG_CONN_DECODE_ERROR = Packet decode error: { $err }
LOG_CONN_SESSION_SUSPENDED = Session suspended (user { $id })

# ---- Auth (network/authenticate_handler.rs) ----
LOG_AUTH_TOKEN = { $peer } sent his token [{ $token }]
LOG_AUTH_LOGGED_IN = { $peer } has logged in as [{ $id }] { $name }

# ---- Play stage (network/play_handler.rs) ----
LOG_PLAY_CREATOR_JOIN_FAILED = creator join failed (room { $room }): { $err }
LOG_PLAY_UNEXPECTED_PACKET = Play stage: unexpected packet (user { $id })

# ---- Room stage (network/room_handler.rs) ----
LOG_ROOM_UNEXPECTED_PACKET = Room stage: unexpected packet, kicking (user { $id })
LOG_ROOM_RECORD_WRITE_FAILED = Record write failed: { $err }

# ---- Player registry (player.rs) ----
LOG_PLAYER_CHECK_SUSPENDED = Resolve or resume: checking suspended (user { $id })
LOG_PLAYER_TAKE_SUSPENDED = Resolve or resume: taking suspended (user { $id })
LOG_PLAYER_FILTER_CHECK = Resolve or resume: filter check (user { $id })

# ---- Phira API (phira.rs) ----
LOG_PHIRA_FETCH_OK = Phira fetch ok: { $path } ({ $status })
LOG_PHIRA_FETCH_NON_2XX = Phira fetch non-2xx: { $path } ({ $err })
LOG_PHIRA_FETCH_TRANSPORT = Phira fetch transport error: { $path } ({ $err })
LOG_PHIRA_FETCH_RETRY = Phira fetch retry: { $path } (attempt { $attempt })

# ---- Session (session.rs) ----
LOG_SESSION_TIMEOUT = Session timeout, force leave (user { $id })

# ---- Server (server.rs) ----
LOG_SERVER_ACCEPT_ERROR = Accept error: { $err }
