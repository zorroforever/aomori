# 单节点链式账本原型设计（Draft v1）

状态：设计提案。L1 基础编码、域哈希、签名及类型已独立实现，见 [实现范围](ledger-v1-primitives.md)；L2–L5 未实现。现有 Runtime/API 未接入账本，不改变其能力声明。

## 1. 目标与范围

把当前即时执行器扩展为可验证、可重放的单节点链式账本：签名交易 → 串行执行 → 区块落盘 → 返回已入块回执；独立进程从同一 genesis 重放，验证相同区块哈希和状态摘要。

本阶段只有一个出块者，无 P2P、分叉选择、验证者共识、Gas、经济安全或拜占庭容错。所谓“已入块”只是本机持久提交，不代表网络最终性。区块链式链接提供完整性检查，不阻止有权访问磁盘的运营者重写整条历史；外部保存的检查点才提供独立比较依据。

首版选择 **每笔成功交易一个区块、同步提交、无持久 mempool、无空块**。先证明正确性，再评估批量出块；不引入定时批次和队列恢复的额外状态机。

## 2. 当前实现与需要隔离的语义

依据 `src/model.rs`、`src/runtime.rs`、`src/storage.rs`：

- Transaction 已含 from/nonce/entity_id/action/args/signature，但没有 chain ID 或签名域版本。
- 签名是移除 signature 后的 serde JSON；tx ID 是含签名交易的 JSON BLAKE3。这是现有 legacy 协议，不直接升级为跨实现编码规范。
- WorldState.head 是执行计数，管理操作也可能推进；不能直接作为 block_height。
- WorldState.root 排除 receipts，但包含事件等世界字段。它是 JSON BLAKE3 摘要，不是 Merkle root。
- 当前可直接创建账户、部署合约及执行 unsigned command；链模式中若这些操作继续绕过区块，就无法完整重放。
- 当前快照与上一份备份不构成完整交易历史。

新模式必须显式开启（建议 `--ledger`），使用新的 genesis 和独立数据目录；legacy runtime 默认行为保持不变。首版不原地导入现有目录、不混用两种交易签名格式。链目录的协议/genesis 不匹配时拒绝启动。

## 3. Genesis 和确定性执行

Genesis manifest 包含：协议版本、chain_id、runtime_rules_version、编码版本、初始世界内容、合约源码和哈希、Lua 执行预算以及账本边界限制。

- chain_id 为显式配置字符串，genesis_hash 为 manifest 的域隔离摘要；两者均进入签名域，防止同名链不同 genesis 重放。
- demo 初始化只用于生成 manifest，启动重放时不再隐式执行 `--demo` 的变更。
- 初始账户必须绑定公钥；账户、实体、合约首版均预置。链模式拒绝旧的直接写入管理 API 和 unsigned command。动态管理操作后续作为新交易类型单独设计。
- 禁止合约读取系统时间、随机数、网络及主机文件；Lua 库、迭代顺序、数值转换、指令/内存限制必须完成审计和确定性测试，不能仅因使用 Lua 就宣称确定性。
- 首版协议 JSON 禁止浮点数：仅 null/bool/string/数组/对象/有界整数；整数限制为 JavaScript 安全整数范围，递归深度和大小受限。对象键按 UTF-8 字节顺序递归排序，无空白；字符串转义及 Unicode 规则通过 Rust/TypeScript 黄金向量固定。拒绝重复键、不做隐式 Unicode 归一化。
- 若现有合约产生不兼容数值，链模式必须拒绝该候选执行，而不是悄悄舍入。协议升级不得静默改变既有历史解释。

编码规则、Lua 行为和状态表示完成跨环境验证，是进入可重放实现的门禁。

## 4. 交易协议 v1

交易信封：`protocol_version, chain_id, genesis_hash, from, nonce, entity_id, action, args, signature`。

定义 `H(domain, payload) = BLAKE3(UTF8(domain) || 0x00 || canonical_json(payload))`；domain 为固定 ASCII 常量，不允许调用方自定义。

- signing_digest = H("aomori/ledger/sign/v1", 信封去除 signature)。Ed25519 签署这 32 字节，不是其 hex 文本。
- tx_id = H("aomori/ledger/tx/v1", 完整签名信封)，对外用小写 hex。
- 公钥、签名严格校验长度和编码；chain/genesis/版本不匹配直接拒绝。
- nonce 必须等于当前账户 nonce；链模式使用已确定的规则，无客户端自动盲目重试。
- 精确重复 tx_id 优先返回已有入块回执，不重复执行；同 nonce 的不同交易按序只允许一笔成功。

执行失败、签名无效、nonce 不匹配、超限等 **不入块、不消耗 nonce、不改变状态**，返回结构化 RPC 错误。本阶段区块仅包含成功交易；失败请求没有永久链上记录，也不承诺拒绝日志不可篡改。

## 5. 区块、摘要与回执

Genesis block 高度为 0，无交易，parent_hash 为固定全零值。后续高度严格 +1。

BlockHeader：

- protocol_version、chain_id、genesis_hash、runtime_rules_version
- height、parent_hash
- tx_count（首版为 1，genesis 为 0）
- transactions_digest：有序 tx_id 数组的域隔离摘要
- results_digest：有序 execution result 数组的域隔离摘要
- post_state_digest：候选世界的协议规范编码摘要

Block 包含 header、完整交易和 execution results；block_hash = H("aomori/ledger/block/v1", header)。首版不包含 wall-clock timestamp，也不需要出块者签名；它们不是当前单节点安全目标的一部分。

ExecutionResult 包含 tx_id/from/nonce/ok/messages/result，以及本笔新增事件的 id 范围和摘要；它不包含 block_hash 或完整 RPC 回执，避免哈希循环。

post_state_digest 排除 receipts 和新增的账本元数据，但包含世界业务状态、执行计数、nonce、事件与事件游标。沿用包含事件的设计意味着历史增长影响摘要成本，不在本阶段隐式加入 retention。

RPC Receipt 在提交完成后派生：execution result + post_state_digest + block_height/block_hash/transaction_index。首版 index=0。新的 ledger_state_digest 与 legacy state_root 分别命名，不暗示兼容；两者都不是 Merkle root。区块结果及 receipts 不回灌到参与 post_state_digest 的字段中。

## 6. 唯一提交点与存储恢复

建议目录：

```
ledger/genesis.json
ledger/blocks/00000000000000000000.json
ledger/blocks/00000000000000000001.json
ledger/cache/state.json
```

**区块历史是权威数据，状态快照只是可重建缓存。** 不把两个独立文件更新误当成一次原子提交。

写路径（单一写锁，读请求只见已发布状态）：

1. 以已提交状态执行到候选副本，校验候选世界和编码规则。
2. 生成区块并验证摘要、parent、高度和结果。
3. 写同目录临时块文件，sync 文件；原子 rename 成最终块文件；sync blocks 目录。必须使用支持所需语义的本地文件系统。
4. 发布候选内存状态和索引。更新快照缓存可延后；缓存写失败不撤销已提交区块。
5. 返回 `INCLUDED` 回执。客户端断线则保留 UNKNOWN，通过 tx_id 查询，不重新执行。

若 rename 后目录 sync 失败，提交是否耐久不确定：停止接受写入、不得继续出下一块或当作确定失败重试；恢复时检查权威块文件并重放，再决定该 tx_id 是否存在。内部发布失败同样进入停写恢复路径。

启动时验证 genesis、连续高度和 parent 链，逐块重放并比对交易/结果/状态摘要。损坏最终块、中间缺块、多个同高度块、无效签名或摘要不符时失败关闭，不静默回滚到较短链。仅未提交临时文件可以忽略。

首版全量重放：缓存缺失、损坏或落后不会改变权威链结果；不盲信最新快照。未来加速检查点需有独立校验策略。运营者删除最后区块无法仅凭剩余链检测，外部检查点可发现此类截断。

进程级锁防止两个节点同时写同一账本目录；RPC 内单一写锁不能替代它。

## 7. RPC / Web 兼容

链模式新增建议接口：

- `aomori_get_chain_info`：协议、chain/genesis、tip height/hash、ledger_state_digest、模式。
- `aomori_get_block`：按高度或哈希查询，带大小上限。
- `aomori_submit_ledger_transaction`：只接受 v1 信封，提交完成才返回 INCLUDED。
- `aomori_get_ledger_receipt`：按 tx_id 查询；没有记录返回 NOT_FOUND，而非推断交易永不入块。

旧查询接口可复用业务视图；旧写接口在链模式明确返回 MODE_DISABLED。新接口不悄悄改变旧接口签名/回执语义。

Web 必须检测模式；链身份备份按 endpoint + chain_id + genesis_hash 隔离。界面区分执行 SUCCESS 和本机 INCLUDED，不展示“共识确认”。切换链时沿用现有异步取消、密钥清理与 UNKNOWN 查询规则。

## 8. 实施切片与门禁

| 切片 | 内容 | 必须通过 |
|---|---|---|
| L1 | 编码、域哈希、genesis、交易/区块类型 | Rust/TS 黄金向量；字段/顺序/域/chain 变化向量；重复键/浮点/超限拒绝 |
| L2 | 纯内存候选执行、区块验证 | 同输入两实例同结果；失败完全不变；nonce/重复 tx；管理写绕过拒绝；合约确定性审计 |
| L3 | 权威块文件、目录锁、启动全重放 | ENOSPC；写/rename/sync 边界故障；确认后 SIGKILL；缓存损坏；缺块/坏块失败关闭 |
| L4 | 新 RPC 和 Web 模式 | 入块查询；提交断线 UNKNOWN；重复提交幂等；旧模式回归 |
| L5 | 有限容量基线与发布说明 | 独立目录重放一致；小时读写资源增长；存储限制/恢复手册；对应提交 CI |

首个实现切片只做 L1，不立刻修改现有 submit_transaction。L3 之前不能宣称耐久账本可用；L5 之前不能宣称容量验收完成。

## 9. 本轮建议确认的决策

推荐采用：新模式新 genesis、每笔成功交易一块、拒绝交易不上链、预置账户/合约、域隔离 v1 签名、区块文件权威/快照缓存、首版无 P2P/共识/Gas。

需在 L1 前固定：编码黄金向量、初始预算数值、Lua 确定性限制及是否需要动态账户/合约。如动态管理是首版必需项，应增加可授权的管理交易协议，而不是保留绕过账本的后门。
