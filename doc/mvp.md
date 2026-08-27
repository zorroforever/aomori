# Aomori 单点 MVP 说明

## 1. 范围

本 MVP 是单进程、单节点、内存状态引擎，不代表完整区块链。它用于验证 Rust 状态机与 Lua 玩法层之间的边界。

## 2. 已实现功能

- `WorldState`：BTreeMap 保存账号、实体和 Lua 合约
- `Overlay`：命令执行使用状态副本，成功提交，失败丢弃
- `EntityKind`：Actor、Zone、Item
- Lua 合约入口：`query_<action>`、`command_<action>`
- Host API：`get_entity`、`get_exit`、`move_actor`、`narrate`
- `state_root`：对规范化 JSON 状态计算 BLAKE3
- JSON-RPC：`aomori_get_info`、`aomori_get_entity`、`aomori_query`、`aomori_command`

## 3. Lua ABI

```lua
function query_look(ctx, args)
  local actor = ctx:get_entity(ctx.entity_id)
  local zone = ctx:get_entity(actor.location)
  ctx:narrate(zone.data.name)
  return { location = actor.location }
end

function command_go(ctx, args)
  local actor = ctx:get_entity(ctx.entity_id)
  local target = ctx:get_exit(actor.location, args.direction)
  if not target then error("没有这个出口") end
  ctx:move_actor(ctx.entity_id, target)
  ctx:narrate("移动成功")
  return { location = target }
end
```

## 4. 运行和验证

```bash
cd /home/developer/workspace/aomori
cargo test
cargo run -- --listen 127.0.0.1:8091
```

测试会验证：

1. 两个 Zone 建立 north/south 出口。
2. Actor 初始位于第一个 Zone。
3. `look` 返回第一个 Zone。
4. `go north` 将 Actor 移到第二个 Zone。
5. Lua 命令失败时 Actor 位置和状态根都不改变。

## 5. 后续阶段

1. 加入 RocksDB 快照和恢复。
2. 引入签名交易、nonce 和 receipt 持久化。
3. 将状态根升级为 Merkle root。
4. 增加 WebSocket 事件订阅。
5. 最后接入区块和共识。
