# Aomori 单节点运行手册

## 容器启动

复制环境变量模板并生成管理凭据：

```bash
cp .env.example .env
openssl rand -hex 32
```

将生成值写入 `.env` 的 `AOMORI_ADMIN_TOKEN`，然后启动：

```bash
docker compose up --build -d
docker compose ps
docker compose logs -f aomori
```

若要执行完整的容器 smoke 验证（需要 Docker daemon 权限），运行：

```bash
./scripts/docker-smoke.sh
```

脚本会使用随机管理 Token、临时 Compose 项目和临时 named volume，构建镜像，检查 `/health` 与 `/ready`，验证容器 UID/GID、只读根文件系统、capability 删除和 `/data/state.json`，然后删除并重新创建容器确认数据卷可恢复。成功后会自动清理测试容器和数据卷；失败时先查看脚本输出和临时项目日志。当前用户若无 `/var/run/docker.sock` 访问权限，脚本会明确失败，不能作为验证通过。

Compose 默认只把节点发布到宿主机 `127.0.0.1:8091`。推荐由同机反向代理终止 TLS，再代理 `/rpc`、`/events`、`/health` 和 `/ready`。只有在防火墙规则已经明确限制来源时，才把 `AOMORI_PUBLISH_ADDRESS` 设置为 `0.0.0.0`。



RPC token bucket 默认直接使用 TCP 对端 IP，且忽略 `X-Forwarded-For`。如果反向代理与节点之间不是一对一的 loopback 连接，应将节点实际看到的代理 IP 以逗号分隔写入 `AOMORI_TRUSTED_PROXIES`；只接受精确 IP，不接受 CIDR。配置后节点会从右向左剥离受信代理地址。反向代理必须先删除客户端传入的 `X-Forwarded-For`，再用连接来源构造新链，否则攻击者可能伪造限流身份。不要把 `0.0.0.0`、任意客户端网段或不受控制的上游代理加入信任列表；信任同机 loopback 代理时，也必须确保节点端口不能被其他本地进程或容器直接访问。

镜像以 UID/GID `10001` 非 root 用户运行，根文件系统只读，唯一持久写路径为 `/data`。默认使用 Docker named volume `aomori-data`。如果改用宿主机 bind mount，需先将目录所有者设为 `10001:10001`，并确认临时文件、主快照和备份处于同一本地文件系统。

## systemd 启动

先构建 release 二进制并创建不可登录的固定服务用户：

```bash
source "$HOME/.cargo/env"
cargo build --release --locked
sudo useradd --system --user-group --home-dir /var/lib/aomori \
  --no-create-home --shell /usr/sbin/nologin aomori
sudo install -o root -g root -m 0755 target/release/aomori /usr/local/bin/aomori
```

安装 root-only 环境文件。必须先替换 Token 占位值和示例 CORS Origin，再启动服务：

```bash
sudo install -d -o root -g root -m 0750 /etc/aomori
sudo install -o root -g root -m 0600 \
  deploy/systemd/aomori.env.example /etc/aomori/aomori.env
sudoedit /etc/aomori/aomori.env
sudo install -o root -g root -m 0644 \
  deploy/systemd/aomori.service /etc/systemd/system/aomori.service
sudo systemctl daemon-reload
sudo systemctl enable --now aomori
```

unit 默认启动 Demo 并仅监听 `127.0.0.1:8091`。自定义世界应在安装前从 `ExecStart` 删除 `--demo`；不要直接修改 `/etc/systemd/system/aomori.service` 中的凭据。`StateDirectory=aomori` 由 systemd 创建权限为 `0700` 的 `/var/lib/aomori`，服务以固定 `aomori` 用户运行；系统目录只读，capability 集为空，设备、内核接口、namespace 和系统调用均受限。

本机反向代理需要可信转发链时，在 `/etc/aomori/aomori.env` 将 `AOMORI_TRUSTED_PROXIES` 设置为 `127.0.0.1`，并遵守上文清理客户端转发头的要求。查看状态和 JSON 日志：

```bash
systemctl status aomori
journalctl -u aomori -f -o cat
curl --fail http://127.0.0.1:8091/ready
```

## HTTP/RPC smoke

若要执行 HTTP/RPC 运行级 smoke 验证（需要已构建的二进制），运行：

```bash
source "$HOME/.cargo/env"
cargo build --locked
./scripts/rpc-smoke.sh
```

该脚本验证 `/health`、`/ready`、JSON/Prometheus metrics、管理 Token 鉴权、默认拒绝 unsigned command，以及状态快照生成；测试 Token 只存在于进程环境和内存中，结束时会清理临时数据和日志。

## 探针和监控

- `/health` 是存活探针，只确认进程能够响应并返回当前状态摘要。
- `/ready` 是就绪探针，会执行世界语义校验并检查快照父目录。
- `/metrics/prometheus` 是 Prometheus 文本指标。
- `/metrics` 是便于人工排查的 JSON 指标。

Compose 和镜像都使用 `/ready` 作为 healthcheck。指标端点当前未认证，不应直接暴露到公网；在反向代理中只允许监控网络访问。

## 升级和回滚

写入的快照 envelope 当前为 `format_version: 2`。v2 要求当前 schema 的集合、任务、合约和事件字段全部显式存在，并在 state root 校验后执行完整世界校验；缺字段不会再由反序列化默认值静默补齐。节点仍可读取 `format_version: 1` 和更早的裸 `WorldState`，但它们只作为启动迁移输入：启动时会先执行已注册的连续格式迁移、已启用的世界迁移和完整校验，再原子重写为 v2。`snapshot_migrated` 结构化日志包含 `from_format_version`、`to_format_version` 和固定 `steps`；裸快照的来源版本为 `null`，日志不包含世界 payload。未来格式只有在代码中存在从来源版本到当前版本的连续注册路径时才能加载，缺少任一步都会拒绝启动。旧 `actor.data.inventory` 会对所有 Actor 迁移到正式 Inventory；非数组、非整数、重复认领、缺失实体或非 Item 引用都会拒绝迁移。迁移或校验失败不会改写原快照，应停止升级并检查数据，不能依靠重复重启跳过。未知的未来版本会被拒绝，也不能通过降级二进制强行打开。

### Compose 升级

升级前先备份 named volume。项目目录名会影响 Compose 自动生成的 volume 名称，因此先运行 `docker volume ls` 确认实际名称，再替换下面的 `aomori_aomori-data`：

```bash
docker compose stop aomori
docker run --rm \
  -v aomori_aomori-data:/source:ro \
  -v "$PWD/backups:/backup" \
  debian:bookworm-slim \
  tar -C /source -czf /backup/aomori-data.tar.gz .
docker compose up --build -d
```

节点收到 SIGTERM 后会停止接受新连接并等待在途请求完成；写 RPC 在返回成功前已经同步提交快照。

升级后检查：

```bash
curl --fail http://127.0.0.1:8091/ready
curl --fail http://127.0.0.1:8091/metrics/prometheus
docker compose logs --since=5m aomori
```

若新版本无法读取旧数据，停止服务，保留故障现场，并从升级前备份恢复整个数据卷。不要只复制 `state.json` 而遗漏同目录中的 `state.json.bak`。

### systemd 升级

先停止服务并备份完整状态目录，再替换二进制。临时文件、主快照和备份必须一起保留：

```bash
sudo systemctl stop aomori
sudo tar --acls --xattrs -C /var/lib -czf \
  "aomori-$(date -u +%Y%m%dT%H%M%SZ).tar.gz" aomori
source "$HOME/.cargo/env"
cargo build --release --locked
sudo install -o root -g root -m 0755 target/release/aomori /usr/local/bin/aomori
sudo systemctl start aomori
curl --fail http://127.0.0.1:8091/ready
journalctl -u aomori --since=-5m -o cat
```

升级失败时立即停止服务，保存失败日志和当前 `/var/lib/aomori`，恢复上一版二进制及升级前的完整目录备份后再启动。不要在服务运行期间覆盖或单独恢复 `state.json`。

## 日志和告警建议

节点向 stderr 输出 JSON 行，容器平台应采集 stdout/stderr，并至少针对以下情况告警：

- `/ready` 连续失败；
- `aomori_snapshot_failures_total` 增加；
- `aomori_websocket_lag_incidents_total` 持续增加；
- RPC 错误率或最大延迟突增；
- 容器反复重启。

日志不包含 RPC params、Authorization、签名或私钥。管理员 Token 仍应仅通过环境变量或编排平台 secret 注入，不要写入 Compose 文件、命令参数或镜像。
