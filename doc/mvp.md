# Aomori 单节点 MVP 说明

[English](README.md) · [简体中文](README.zh-CN.md) · [日本語](README.ja.md)

> 本文档当前以中文维护。

## 1. 范围

Aomori 当前是一个单进程、单节点的确定性 MUD 世界运行时。它验证 Rust 状态机、Lua 玩法层、事务回滚、签名交易、持久化快照和实时事件之间的最小闭环。

Aomori 目前不是完整区块链：没有区块生产、多节点同步、P2P 网络或共识机制。`state_root` 是对规范化 JSON 世界状态计算的 BLAKE3 摘要，用于一致性校验，不是 Merkle root。

## 2. 已实现功能

### 世界状态和脚本

- `WorldState`：使用确定性集合保存 Account、Contract、Entity、Quest、Inventory、Receipt 和 Event。
- `EntityKind`：Actor、Zone、Item。
- Lua 合约入口：`query_<action>`、`command_<action>`。
- 受限 Lua 执行：指令数和内存上限，超限自动失败并回滚。
- 查询和命令分离：查询不可修改世界状态，命令在事务副本中执行。
- Lua Host API：实体查询、出口查询、Actor 移动、实体创建和更新、Inventory 操作、事件发送、叙事消息。

### 状态、任务和交易

- 对规范化 JSON 状态计算 BLAKE3 `state_root`。
- Rust 管理任务发布者、前置任务、任务链、并行进度和奖励。
- Entity owner、位置、Inventory 双向引用和依赖关系启动时严格校验。
- Ed25519 公钥、签名交易、nonce、确定性 transaction ID 和执行 receipt。
- unsigned command 默认关闭，仅可显式开启用于本地 Demo。

### RPC、事件和客户端

- HTTP JSON-RPC：读取世界、账号、实体、任务、receipt 和事件。
- 管理 RPC：账号创建、合约发布和实体创建，使用 Bearer Token 认证。
- WebSocket `/events` 实时事件推送。
- `aomori_get_events` 增量事件查询，以及断线和 lag 补偿协议。
- Vite Web 客户端：Ed25519 身份创建、加密密钥仓、签名交易和可玩的 Demo 任务流程。

### 持久化和运维

- 版本化 JSON 快照、`state_root` 完整性校验和连续格式迁移。
- 原子快照提交、备份、恢复和临时文件同步。
- Legacy snapshot 与旧 Actor Inventory 自动迁移。
- `/health`、`/ready`、JSON metrics 和 Prometheus metrics。
- Docker 非 root 部署、只读根文件系统、持久化数据卷和 systemd 服务。

## 3. Lua ABI

```lua
function query_look(ctx, args)
  local actor = host.get_entity(ctx.entity_id)
  local zone = host.get_entity(actor.location)
  host.narrate(zone.data.name)
  return { location = actor.location, name = zone.data.name }
end

function command_go(ctx, args)
  local actor = host.get_entity(ctx.entity_id)
  local target = host.get_exit(actor.location, args.direction)
  if not target then error("no exit") end
  host.move_actor(ctx.entity_id, target)
  return { location = target }
end
```

完整 RPC、Host API、交易和事件定义见 [API 文档](api.md)。

## 4. 运行和验证

启动本地 Demo：

```bash
cargo test --locked
cargo run -- --listen 127.0.0.1:8091 --demo --allow-unsigned-commands
```

启动 Web 客户端：

```bash
cd web
npm ci
npm run dev
```

当前验证覆盖：

1. Rust 单元测试和集成测试验证世界状态、Lua 执行、事务回滚、RPC、事件、迁移和快照恢复。
2. Web 构建验证 TypeScript 和 Vite 生产构建。
3. Playwright E2E 验证 Demo 中的任务流程。
4. RPC smoke 验证健康检查、metrics、管理员认证、unsigned command 策略和快照生成。
5. Docker smoke 验证非 root、只读根文件系统、capability 删除和数据卷恢复。

## 5. 后续阶段

### 存储和性能

1. 引入 RocksDB 或其他适合生产负载的持久化存储。
2. 增加快照压缩、历史状态查询和更大规模世界状态基准。
3. 评估 Merkle tree 或其他可验证状态承诺结构。

### 链上协议

1. 定义区块、交易排序、确认和重放规则。
2. 增加多节点状态同步和 P2P 网络。
3. 选择并实现共识机制。
4. 在安全模型明确后再设计 Gas、资产发行和经济系统。

### 客户端和平台

1. 扩充 Web E2E：身份解锁、签名交易、nonce 冲突、WebSocket 重连和异常网络状态。
2. 增加 TLS 反向代理、Prometheus/Grafana 和备份恢复示例。
3. 补充高并发、长时间运行和磁盘故障演练。
