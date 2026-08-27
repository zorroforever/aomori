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

Compose 默认只把节点发布到宿主机 `127.0.0.1:8091`。推荐由同机反向代理终止 TLS，再代理 `/rpc`、`/events`、`/health` 和 `/ready`。只有在防火墙规则已经明确限制来源时，才把 `AOMORI_PUBLISH_ADDRESS` 设置为 `0.0.0.0`。

RPC token bucket 默认直接使用 TCP 对端 IP，且忽略 `X-Forwarded-For`。如果反向代理与节点之间不是一对一的 loopback 连接，应将节点实际看到的代理 IP 以逗号分隔写入 `AOMORI_TRUSTED_PROXIES`；只接受精确 IP，不接受 CIDR。配置后节点会从右向左剥离受信代理地址。反向代理必须先删除客户端传入的 `X-Forwarded-For`，再用连接来源构造新链，否则攻击者可能伪造限流身份。不要把 `0.0.0.0`、任意客户端网段或不受控制的上游代理加入信任列表；信任同机 loopback 代理时，也必须确保节点端口不能被其他本地进程或容器直接访问。

镜像以 UID/GID `10001` 非 root 用户运行，根文件系统只读，唯一持久写路径为 `/data`。默认使用 Docker named volume `aomori-data`。如果改用宿主机 bind mount，需先将目录所有者设为 `10001:10001`，并确认临时文件、主快照和备份处于同一本地文件系统。

## 探针和监控

- `/health` 是存活探针，只确认进程能够响应并返回当前状态摘要。
- `/ready` 是就绪探针，会执行世界语义校验并检查快照父目录。
- `/metrics/prometheus` 是 Prometheus 文本指标。
- `/metrics` 是便于人工排查的 JSON 指标。

Compose 和镜像都使用 `/ready` 作为 healthcheck。指标端点当前未认证，不应直接暴露到公网；在反向代理中只允许监控网络访问。

## 升级和回滚

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

## 日志和告警建议

节点向 stderr 输出 JSON 行，容器平台应采集 stdout/stderr，并至少针对以下情况告警：

- `/ready` 连续失败；
- `aomori_snapshot_failures_total` 增加；
- `aomori_websocket_lag_incidents_total` 持续增加；
- RPC 错误率或最大延迟突增；
- 容器反复重启。

日志不包含 RPC params、Authorization、签名或私钥。管理员 Token 仍应仅通过环境变量或编排平台 secret 注入，不要写入 Compose 文件、命令参数或镜像。
