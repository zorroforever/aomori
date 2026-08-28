# Aomori

Aomori 是一个以 Rust 实现的单点自治世界 MVP。脚本使用 Lua，目标是验证 MUD 世界状态、脚本行为和事务回滚的最小闭环。

## 当前能力

- 内存世界状态：账号、合约、Zone、Actor
- Lua `query_*` 查询和 `command_*` 命令
- 命令事务失败自动回滚
- Lua 每次执行默认限制为 200,000 条指令和 16 MiB 内存，超限自动中止并回滚
- 确定性状态根
- 版本化 JSON 快照和 `state_root` 完整性校验
- 临时文件与备份先 `sync_all`，原子替换后同步数据目录，降低断电丢失风险
- `state.json.bak` 仅从验证有效的主快照更新，并支持同步恢复
- HTTP JSON-RPC API（1 MiB 请求体上限，非法 JSON 返回标准 `-32700`）
- CORS 默认仅允许本机 Vite Origin，可通过 `AOMORI_CORS_ORIGINS` 配置局域网页面
- `/rpc` 默认按客户端 IP 使用每秒补充 100 个 token、容量 100 的 token bucket；Web 只对幂等读请求自动退避重试一次，写请求不自动重放
- 管理 RPC 使用 `Authorization: Bearer` 认证；未配置 `AOMORI_ADMIN_TOKEN` 时默认禁用
- Web 客户端可生成 Ed25519 身份，本地仅持久化密码加密密钥仓；刷新后需显式解锁，明文私钥只保留在当前页面内存，并支持导出/导入加密备份
- 签名交易遇到并发 nonce 冲突时自动刷新 nonce、重新签名并重试一次
- 无签名 command 默认禁用；本地 Demo 需显式使用 `--allow-unsigned-commands`
- `aomori_create_account`、`aomori_get_account`、`aomori_submit_transaction`
- Rust 管理的任务定义、发布者关系、前置条件、任务链、并行任务进度、奖励和 Inventory 索引；Account、Contract、Entity、QuestProgress、Receipt、Event、location 和 Inventory 引用在加载与保存时统一校验引用和依赖环
- Lua Host API：`spawn_entity`、`update_entity_data`、`take_item`、`drop_item`、`transfer_item`、`get_inventory`、`emit_event`
- `aomori_get_events` 增量事件查询
- `aomori_list_entities` 按位置和类型查询实体
- WebSocket `/events` 实时事件推送，lag 时发送控制消息并由客户端自动通过 HTTP 分页补偿
- Rust 自动产生 `entity_changed`、`quest_progress_changed`、`command_executed` 和 `transaction_executed` 系统事件
- 交易 nonce、实体 owner 校验和确定性 transaction id
- 带 `tx_id`、`from`、`nonce` 的执行收据

当前 `aomori_submit_transaction` 支持无公钥的本地开发账户，也支持使用 Ed25519 公钥和十六进制签名的账户交易。

实体查询示例：

```bash
curl -s http://127.0.0.1:8091/rpc \
  -H 'content-type: application/json' \
  -d '{"jsonrpc":"2.0","id":1,"method":"aomori_list_entities","params":{"location":1}}' | jq
```


```text
ws://127.0.0.1:8091/events
```

客户端应记录最后收到的事件 ID；重连后先调用 `aomori_get_events` 补齐断线期间的事件，再重新连接 WebSocket。

Lua command 可以通过受限 Host API 创建实体、修改自己拥有的实体属性并发送事件；query 调用这些修改 API 会失败并回滚。

## 运行

```bash
cargo test
cd web && npm run test:e2e
cargo run -- --listen 127.0.0.1:8091 --allow-unsigned-commands
```

启动可玩 Demo 世界（已有数据会按源码哈希自动发布新合约版本并迁移任务进度）：

```bash
export AOMORI_ADMIN_TOKEN='replace-with-a-long-random-token'
export AOMORI_CORS_ORIGINS='http://127.0.0.1:5173,http://localhost:5173'
cargo run -- --listen 127.0.0.1:8091 --data-dir ./demo-data --demo \
  --allow-unsigned-commands \
  --lua-instruction-limit 200000 \
  --lua-memory-limit 16777216
```

Demo 默认 Actor ID 为 `4`，初始世界包含 Village、Forest、Ruins、brass key、stone tablet，以及任务发布者 Mira 和 Rowan。`lost_key` 会消耗 brass key 并奖励 10 coins；`ruins_tablet` 保留 stone tablet 并奖励 6 coins；完成 `lost_key` 后会解锁 `open_shrine`，携带 tablet 到 Ruins 可再获得 4 coins。使用默认 Actor 试玩需要 `--allow-unsigned-commands`；也可以在 Web 客户端输入管理员 Token 创建独立的 Ed25519 账户与 Actor，在默认安全模式下游玩。


```bash
curl -s http://127.0.0.1:8091/rpc \
  -H 'content-type: application/json' \
  -d '{"jsonrpc":"2.0","id":1,"method":"aomori_get_info","params":{}}'
```

健康与就绪检查：

```bash
curl -s http://127.0.0.1:8091/health | jq
curl -s http://127.0.0.1:8091/ready | jq
```

`/health` 返回当前 `head` 和 `state_root`；`/ready` 额外执行只读世界语义校验并检查快照数据目录。累计 RPC、快照和 WebSocket 指标可通过 JSON `GET /metrics` 或 Prometheus `GET /metrics/prometheus` 查看。每个进入 RPC handler 的请求响应包含 `X-Request-Id`，节点向 stderr 输出不含 params、Authorization、签名或请求 body 的 JSON 行日志。

协议定义见 [`doc/api.md`](doc/api.md)，容器与 systemd 部署、备份、升级和告警建议见 [`doc/operations.md`](doc/operations.md)。

## Docker

镜像采用多阶段构建，并以非 root 用户运行。Compose 默认启用只读根文件系统、持久化 named volume 和 `/ready` 健康检查，只向宿主机回环地址发布端口：

```bash
cp .env.example .env
# 使用 `openssl rand -hex 32` 生成 Token，并写入 .env

docker compose up --build -d
docker compose ps
curl --fail http://127.0.0.1:8091/ready
```

节点会处理 SIGINT/SIGTERM，停止接受新连接并等待在途请求结束。也提供 [`deploy/systemd/aomori.service`](deploy/systemd/aomori.service) 和 root-only [`deploy/systemd/aomori.env.example`](deploy/systemd/aomori.env.example)，用于固定非 root 用户的宿主机部署。局域网或公网部署前请阅读运行手册中的监听地址、TLS、指标访问和数据备份说明。

Web 客户端默认连接页面所在主机的 `8091` 端口，也可在界面中覆盖 RPC 地址。Playwright 端到端测试会自动启动隔离的 Demo 节点和 Vite 服务。


