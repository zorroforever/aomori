<div align="center">

# Aomori

**Rust と Lua で構築する、決定論的でプログラマブルな MUD ワールド。**

[![Rust](https://img.shields.io/badge/Rust-2021-000000?logo=rust&logoColor=white)](https://www.rust-lang.org/)
[![License: MIT](https://img.shields.io/badge/license-MIT-blue.svg)](LICENSE)
[![JSON-RPC](https://img.shields.io/badge/API-JSON--RPC%202.0-2f855a)](doc/api.md)
[![WebSocket](https://img.shields.io/badge/events-WebSocket-7c3aed)](doc/api.md)

[English](README.md) · [简体中文](README.zh-CN.md) · [日本語](README.ja.md)

</div>

Aomori は、Rust ランタイム、制限付き Lua ワールドスクリプト、決定論的な状態、トランザクション型コマンド実行、ブラウザクライアントを組み合わせたシングルノードの自律世界 MVP です。

> Aomori は実験的なゲームランタイムおよびプロトコルのプロトタイプです。本番用ブロックチェーンやパブリックネットワークではありません。

## 主な機能

- **決定論的なワールド状態**：アカウント、コントラクト、ゾーン、Actor、エンティティ、クエスト、Inventory、レシート、イベントを保存し、検証可能な `state_root` を生成。
- **制限付き Lua スクリプト**：クエリとコマンドをサンドボックスで実行し、命令数とメモリ上限を適用。
- **トランザクション実行**：コマンド失敗、無効な Host API 呼び出し、スクリプト上限超過時に状態を自動ロールバック。
- **プログラマブルなクエストとエンティティ**：クエストチェーン、前提条件、並列進行、報酬、所有権、位置、Inventory インデックスを Rust で管理。
- **署名トランザクション**：Ed25519 ID、nonce 検証、決定論的 transaction ID、所有権検証、実行レシートに対応。
- **リアルタイムイベント**：増分イベント取得と WebSocket 配信、再接続時の欠損補填を提供。
- **永続スナップショット**：バージョン付き JSON、整合性検証、マイグレーション、アトミック置換、バックアップに対応。
- **運用向け機能**：HTTP JSON-RPC、CORS 制御、リクエスト制限、トークンバケット、管理者認証、ヘルスチェック、メトリクス、Docker、systemd。
- **プレイ可能な Demo ワールド**：Web クライアントから Village、Forest、Ruins を探索し、クエスト、アイテム、報酬を体験可能。

## アーキテクチャ

```text
ブラウザクライアント
     │ HTTP JSON-RPC + WebSocket イベント
     ▼
Axum API / 認証 / レート制限 / 可観測性
     │
     ▼
Rust ワールドランタイム ───── Lua スクリプト + 制限付き Host API
     │
     ├── 決定論的状態 + state_root
     ├── トランザクション検証 + ロールバック
     ├── クエスト / エンティティ / Inventory / イベント
     └── バージョン付き JSON スナップショット
```

## クイックスタート

### 前提条件

- Rust stable と Cargo
- Node.js と npm（Web クライアントを実行する場合のみ）
- Docker（任意。コンテナデプロイ用）

### ノードを起動

```bash
cargo run -- --listen 127.0.0.1:8091 --allow-unsigned-commands
```

```bash
curl -s http://127.0.0.1:8091/health | jq
curl -s http://127.0.0.1:8091/ready | jq
```

### Demo ワールドを起動

```bash
export AOMORI_ADMIN_TOKEN="replace-with-a-long-random-token"
export AOMORI_CORS_ORIGINS="http://127.0.0.1:5173,http://localhost:5173"

cargo run -- \
  --listen 127.0.0.1:8091 \
  --data-dir ./demo-data \
  --demo \
  --allow-unsigned-commands
```

Demo のデフォルト Actor ID は `4` です。Village、Forest、Ruins、brass key、stone tablet、Mira と Rowan に関係するクエストチェーンが含まれます。Web クライアントはデフォルトでポート `8091` に接続します。

### Web クライアントを起動

別のターミナルで実行します。

```bash
cd web
npm ci
npm run dev
```

<http://127.0.0.1:5173> を開いてください。

署名なしコマンドはローカル開発専用です。通常の安全なモードでは、管理者 Token を使って Web クライアントから Ed25519 アカウントと Actor を作成してください。

## API 例

指定した場所のエンティティを取得します。

```bash
curl -s http://127.0.0.1:8091/rpc \
  -H 'content-type: application/json' \
  -d '{"jsonrpc":"2.0","id":1,"method":"aomori_list_entities","params":{"location":1}}' | jq
```

リアルタイムイベントの購読先：

```text
ws://127.0.0.1:8091/events
```

クライアントは最後に受信したイベント ID を保持してください。再接続後は `aomori_get_events` を呼び出して欠損分を補完してから、再度購読します。RPC メソッド、トランザクション形式、エラー、イベント仕様は [API リファレンス](doc/api.md) を参照してください。

## 開発と検証

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

GitHub Actions は `main` への push と Pull Request で Rust 品質、Web ビルド、ブラウザ E2E、Docker smoke test を検証します。

## デプロイ

Docker Compose は非 root ユーザー、読み取り専用 root filesystem、永続 named volume、`/ready` ヘルスチェックを使用します。

```bash
cp .env.example .env
# .env に十分な長さのランダムな Token を設定

docker compose up --build -d
docker compose ps
curl --fail http://127.0.0.1:8091/ready
```

ホスト環境では [`deploy/systemd/aomori.service`](deploy/systemd/aomori.service) と環境変数テンプレートを利用できます。localhost の外部へ公開する前に、リスナー、TLS、メトリクス、バックアップ、アップグレードについて [運用ガイド](doc/operations.md) を確認してください。

## ドキュメント

- [API リファレンス](doc/api.md)
- [運用ガイド](doc/operations.md)
- [MVP スコープ](doc/mvp.md)
- [プロダクト要件](doc/prd.md)
- [コントラクトと Demo スクリプト](contracts/)

## ライセンス

Aomori は [MIT License](LICENSE) のもとで公開されています。
