# Aomori 总体 PRD

[English](README.md) · [简体中文](README.zh-CN.md) · [日本語](README.ja.md)

> 本文档当前以中文维护。

## 1. 产品定位

Aomori 是面向 MUD/文字冒险世界的确定性自治世界引擎。Rust 负责账号、权限、资产、实体、事务和状态一致性；Lua 负责可迭代的世界规则、命令、任务和 NPC 行为。

## 2. 目标用户

- 世界规则和剧情设计者
- MUD 客户端开发者
- 希望验证链上游戏状态机的工程团队

## 3. 核心原则

1. 世界事实由 Rust 状态机掌握，Lua 不能绕过权限修改任意状态。
2. 查询和命令分离：查询不改变状态，命令在事务中执行。
3. 相同初始状态和输入必须产生相同状态根。
4. 合约版本发布后不可变，升级采用新版本和迁移。
5. RPC 面向结构化 action，不要求客户端拼接 Lua 字符串。

## 4. 当前架构与演进方向

当前实现：

```text
浏览器客户端
     │ HTTP JSON-RPC + WebSocket events
     ▼
Axum API / 认证 / 限流 / 可观测性
     │
     ▼
Rust Runtime ───── Lua Host API
     │
     ├── World State + BLAKE3 state_root
     ├── transaction validation + rollback
     ├── quests / entities / inventory / events
     └── versioned JSON snapshot store
```

当前节点是单进程、单节点运行时。后续可以在不改变 Lua 世界脚本边界的前提下，逐步加入 RocksDB、历史状态索引、区块层、P2P、共识和多节点同步。

## 5. 主要领域对象

- Account：账号、公钥、nonce、余额
- Entity：通用实体
- Actor：可操作角色，具有当前位置
- Zone：区域及出口图
- Contract：Lua 源码、哈希、版本和发布状态
- Transaction：签名操作和执行收据

## 6. 当前非目标

当前阶段暂不实现：

- 多节点共识和 P2P 网络
- 区块生产、确认和链重组
- 经济通胀、Gas 和生产级资产系统
- 生产级托管钱包或硬件钱包集成
- 大规模历史状态索引
- 与 Taiyi 二进制协议的兼容

## 7. 成功标准

当前 MVP 已达到以下标准：

- 能在本地启动单节点并持久化世界状态。
- 能部署 Lua 合约，创建相连区域和角色，执行 `look`、`go` 等命令。
- 查询不会修改状态，失败命令不会改变状态根。
- 能通过签名交易执行命令，并生成可查询的 receipt。
- 能通过快照迁移、备份恢复和 `/ready` 校验保证重启后的世界一致性。
- 能通过 WebSocket 接收事件，并通过增量 RPC 补偿断线期间的事件。

后续阶段的成功标准是：在不破坏当前确定性和事务语义的前提下，引入可测量的高性能存储、多节点同步和明确的共识安全模型。
