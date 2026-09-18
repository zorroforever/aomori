<div align="center">

# Aomori

**用 Rust 与 Lua 构建的确定性、可编程 MUD 世界。**

[![Rust](https://img.shields.io/badge/Rust-2021-000000?logo=rust&logoColor=white)](https://www.rust-lang.org/)
[![License: MIT](https://img.shields.io/badge/license-MIT-blue.svg)](LICENSE)
[![JSON-RPC](https://img.shields.io/badge/API-JSON--RPC%202.0-2f855a)](doc/api.md)
[![WebSocket](https://img.shields.io/badge/events-WebSocket-7c3aed)](doc/api.md)

[English](README.md) · [简体中文](README.zh-CN.md) · [日本語](README.ja.md)

</div>

Aomori 是一个单节点自治世界 MVP。它将 Rust 运行时、受限 Lua 世界脚本、确定性状态、事务化命令执行和浏览器客户端组合成一个小而完整的 MUD 平台。

> Aomori 是实验性的游戏运行时与协议原型，并非生产级区块链或公链网络。

## 核心特性

- **确定性世界状态**：持久化账号、合约、区域、Actor、实体、任务、Inventory、收据和事件，并生成可校验的 `state_root`。
- **受限 Lua 脚本**：查询和命令脚本运行在沙箱中，并受指令数与内存上限保护。
- **事务化执行**：命令失败、非法 Host API 调用或脚本超限时，世界状态自动回滚。
- **可编程任务与实体**：Rust 管理任务链、前置条件、并行进度、奖励、实体所有权、位置和 Inventory 索引。
- **签名交易**：支持 Ed25519 身份、nonce 校验、确定性交易 ID、所有权校验和执行收据。
- **实时事件**：提供增量事件查询和 WebSocket 推送，并支持断线重连与延迟补偿。
- **持久化快照**：支持版本化 JSON 快照、完整性校验、迁移、原子替换、备份和断电风险控制。
- **可运维默认配置**：HTTP JSON-RPC、CORS 控制、请求体限制、令牌桶限流、管理员认证、健康检查、就绪检查、指标、Docker 和 systemd 部署。
- **可玩的 Demo 世界**：通过 Web 客户端探索 Village、Forest 和 Ruins，完成任务、收集物品并获得奖励。

## 架构

```text
浏览器客户端
     │ HTTP JSON-RPC + WebSocket 事件
     ▼
Axum API / 认证 / 限流 / 可观测性
     │
     ▼
Rust 世界运行时 ───── Lua 脚本 + 受限 Host API
     │
     ├── 确定性状态 + state_root
     ├── 交易校验 + 事务回滚
     ├── 任务 / 实体 / Inventory / 事件
     └── 版本化 JSON 快照存储
```

## 快速开始

### 环境要求

- Rust stable 与 Cargo
- Node.js 与 npm（仅运行 Web 客户端时需要）
- Docker（可选，用于容器部署）

### 启动节点

```bash
cargo run -- --listen 127.0.0.1:8091 --allow-unsigned-commands
```

检查节点状态：

```bash
curl -s http://127.0.0.1:8091/health | jq
curl -s http://127.0.0.1:8091/ready | jq
```

### 启动 Demo 世界

```bash
export AOMORI_ADMIN_TOKEN="replace-with-a-long-random-token"
export AOMORI_CORS_ORIGINS="http://127.0.0.1:5173,http://localhost:5173"

cargo run -- \
  --listen 127.0.0.1:8091 \
  --data-dir ./demo-data \
  --demo \
  --allow-unsigned-commands
```

Demo 默认使用 Actor ID `4`，包含 Village、Forest、Ruins、brass key、stone tablet，以及围绕 Mira 和 Rowan 展开的任务链。Web 客户端默认连接 `8091` 端口。

### 启动 Web 客户端

在另一个终端执行：

```bash
cd web
npm ci
npm run dev
```

打开 <http://127.0.0.1:5173>。

无签名命令仅用于本地开发。默认安全模式下，请在 Web 客户端使用管理员 Token 创建 Ed25519 账户和 Actor。

## API 示例

查询某个位置的实体：

```bash
curl -s http://127.0.0.1:8091/rpc \
  -H 'content-type: application/json' \
  -d '{"jsonrpc":"2.0","id":1,"method":"aomori_list_entities","params":{"location":1}}' | jq
```

订阅实时事件：

```text
ws://127.0.0.1:8091/events
```

客户端应记录最后收到的事件 ID。重连后先调用 `aomori_get_events` 补齐断线期间的事件，再重新建立订阅。RPC 方法、交易格式、错误和事件语义见 [API 文档](doc/api.md)。

## Lua 世界脚本

脚本可以查询世界，并通过 Host API 执行受控命令。命令可以移动 Actor、创建实体、修改自己拥有的实体属性、管理 Inventory 和发送事件。查询执行不能修改状态；尝试修改时会失败并回滚。

## 开发与验证

```bash
cargo fmt --all -- --check
cargo test --locked
cargo check --locked
cargo clippy --locked --all-targets -- -D warnings

cd web
npm ci
npm run build
npm run test:e2e
```

GitHub Actions 会在推送到 `main` 和创建 Pull Request 时检查 Rust 质量、Web 构建、浏览器 E2E 测试和 Docker smoke 测试。

## 部署

Docker Compose 提供非 root 用户、只读根文件系统、持久化 named volume 和 `/ready` 健康检查：

```bash
cp .env.example .env
# 在 .env 中写入足够长的随机 Token

docker compose up --build -d
docker compose ps
curl --fail http://127.0.0.1:8091/ready
```

宿主机部署可使用 [`deploy/systemd/aomori.service`](deploy/systemd/aomori.service) 和对应的环境变量模板。对外暴露节点前，请先阅读[运行手册](doc/operations.md)中的监听地址、TLS、指标、备份和升级建议。

## 文档

- [API 文档](doc/api.md)
- [运行手册](doc/operations.md)
- [MVP 范围](doc/mvp.md)
- [产品需求](doc/prd.md)
- [合约与 Demo 脚本](contracts/)

## 许可证

Aomori 使用 [MIT License](LICENSE) 发布。
