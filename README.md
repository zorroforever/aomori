# Aomori

Aomori 是一个以 Rust 实现的单点自治世界 MVP。脚本使用 Lua，目标是验证 MUD 世界状态、脚本行为和事务回滚的最小闭环。

## 当前能力

- 内存世界状态：账号、合约、Zone、Actor
- Lua `query_*` 查询和 `command_*` 命令
- 命令事务失败自动回滚
- 确定性状态根
- HTTP JSON-RPC API
- `look -> go -> look` 集成测试

## 运行

```bash
cargo test
cargo run -- --listen 127.0.0.1:8090
```

启动后：

```bash
curl -s http://127.0.0.1:8090/rpc \
  -H 'content-type: application/json' \
  -d '{"jsonrpc":"2.0","id":1,"method":"aomori_get_info","params":{}}'
```

更多设计见 [`doc/prd.md`](doc/prd.md) 和 [`doc/mvp.md`](doc/mvp.md)。
