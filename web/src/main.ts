import './style.css';
import nacl from 'tweetnacl';
import { pbkdf2 } from '@noble/hashes/pbkdf2';
import { sha256 } from '@noble/hashes/sha256';

type RpcResult = { result?: any; error?: { code: number; message: string; data?: { retry_after_ms?: number } } };
type WorldEvent = { id: number; head: number; kind: string; entity_id?: number; data: Record<string, unknown> };
type EventStreamLagged = { type: 'event_stream_lagged'; missed: number; last_event_id: number };
type IdentityBackup = { format: 'aomori-ed25519-backup'; version: 1; account: string; publicKey: string; salt: string; nonce: string; ciphertext: string; iterations: number };

const defaultRpc = import.meta.env.VITE_AOMORI_RPC || `${window.location.protocol}//${window.location.hostname}:8091`;
const state = { rpc: defaultRpc, actor: 4, account: '', secretKey: null as Uint8Array | null, lastEvent: 0, seenEvents: new Set<number>(), recoveringEvents: null as Promise<void> | null, history: [] as string[], historyIndex: -1, socket: null as WebSocket | null, reconnectTimer: 0, roomActors: [] as any[], quests: [] as any[] };
const app = document.querySelector<HTMLDivElement>('#app')!;

app.innerHTML = `
  <header class="topbar">
    <div class="brand"><span class="mark">A</span><div><strong>AOMORI</strong><small>single-node world</small></div></div>
    <div class="connection"><span id="statusDot" class="dot offline"></span><span id="statusText">未连接</span><button id="connectBtn" class="button">连接节点</button></div>
  </header>
  <main class="layout">
    <aside class="sidebar">
      <section class="section"><div class="section-title">WORLD NODE</div><label>RPC 地址<input id="rpcInput" value="${state.rpc}" /></label><label>Actor ID<input id="actorInput" type="number" value="${state.actor}" min="1" /></label></section>
      <section class="section"><div class="section-title">签名身份</div><label>新账户名<input id="accountInput" placeholder="player-name" /></label><label>管理员 Token<input id="adminTokenInput" type="password" autocomplete="off" placeholder="仅创建身份时使用" /></label><button id="createIdentityBtn" class="button identity-button">创建签名身份</button><div class="stat"><span>写入模式</span><strong id="writeMode">开发 command</strong></div><div class="identity-actions"><button id="exportIdentityBtn" class="text-button" hidden>导出加密备份</button><button id="importIdentityBtn" class="text-button">导入加密备份</button><input id="importIdentityFile" type="file" accept="application/json,.json" hidden /></div><button id="forgetIdentityBtn" class="text-button" hidden>清除此账户私钥</button></section>
      <section class="section"><div class="section-title">当前角色</div><div class="identity"><div class="avatar">03</div><div><strong id="actorLabel">Actor #4</strong><span>explorer</span></div></div><div class="stat"><span>位置</span><strong id="location">未知</strong></div><div class="stat"><span>世界高度</span><strong id="head">-</strong></div></section>
      <section class="section"><div class="section-title">出口</div><div id="exits" class="exits"><span class="muted">执行 look 查看</span></div></section><section class="section"><div class="section-title">当前位置</div><div id="roomEntities" class="room-entities"><span class="muted">执行 look 查看</span></div></section><section class="section"><div class="section-title">背包</div><div id="inventory" class="inventory"><span class="muted">暂无物品</span></div></section><section class="section"><div class="section-title">任务</div><div id="questList" class="quest-list"><span class="muted">暂无任务</span></div><div class="stat"><span>余额</span><strong id="balance">0</strong></div></section>
    </aside>
    <section class="play"><div class="play-head"><div><span class="eyebrow">LIVE NARRATIVE</span><h1 id="zoneName">未进入世界</h1></div><button id="lookBtn" class="icon-button" title="查看当前位置">◉ <span>查看</span></button></div><div id="log" class="log"></div><form id="commandForm" class="command"><span class="prompt">›</span><input id="commandInput" autocomplete="off" placeholder="输入命令，例如 go north" /><button title="执行命令">执行</button></form></section>
    <aside class="events"><div class="panel-head"><div><span class="eyebrow">EVENT STREAM</span><h2>世界事件</h2></div><span id="eventCount" class="count">0</span></div><div id="eventList" class="event-list"><span class="muted">等待事件...</span></div><div class="receipt-panel"><span class="eyebrow">LAST RECEIPT</span><div id="receipt"><span class="muted">暂无交易</span></div></div></aside>
  </main>`;

const $ = <T extends HTMLElement>(id: string) => document.getElementById(id) as T;
const log = $('log');
function addLog(text: string, type = '') { const row = document.createElement('div'); row.className = `log-row ${type}`; row.innerHTML = `<span class="log-time">${new Date().toLocaleTimeString([], { hour: '2-digit', minute: '2-digit' })}</span><span>${escapeHtml(text)}</span>`; log.append(row); log.scrollTop = log.scrollHeight; }
function escapeHtml(value: string) { return value.replace(/[&<>'"]/g, c => ({ '&':'&amp;', '<':'&lt;', '>':'&gt;', "'":'&#39;', '"':'&quot;' }[c]!)); }
function setStatus(online: boolean, text: string) { $('statusDot').className = `dot ${online ? 'online' : 'offline'}`; $('statusText').textContent = text; }
function selectRpc(rpcUrl: string) { const next = rpcUrl.replace(/\/$/, ''); if (next !== state.rpc) { window.clearTimeout(state.reconnectTimer); if (state.socket) { state.socket.onclose = null; state.socket.close(); state.socket = null; } state.lastEvent = 0; state.seenEvents.clear(); state.recoveringEvents = null; $('eventCount').textContent = '0'; $('eventList').innerHTML = '<span class="muted">等待事件...</span>'; } state.rpc = next; }
function bytesToHex(bytes: Uint8Array) { return Array.from(bytes, byte => byte.toString(16).padStart(2, '0')).join(''); }
function hexToBytes(hex: string) { if (!/^[0-9a-f]+$/i.test(hex) || hex.length % 2) throw new Error('无效的本地私钥'); return new Uint8Array(hex.match(/.{2}/g)!.map(byte => Number.parseInt(byte, 16))); }
function keyStorageName(account: string) { return `aomori:ed25519:${state.rpc}:${account}`; }
function loadIdentity(account: string) { const encoded = localStorage.getItem(keyStorageName(account)); state.account = account; state.secretKey = encoded ? hexToBytes(encoded) : null; $('writeMode').textContent = state.secretKey ? `签名交易 · ${account}` : '开发 command'; $('forgetIdentityBtn').hidden = !state.secretKey; $('exportIdentityBtn').hidden = !state.secretKey; }
function deriveBackupKey(password: string, salt: Uint8Array, iterations: number) { return pbkdf2(sha256, new TextEncoder().encode(password), salt, { c: iterations, dkLen: nacl.secretbox.keyLength }); }
function password(promptText: string) { const value = window.prompt(promptText); if (!value || value.length < 8) throw new Error('备份密码至少需要 8 个字符'); return value; }
function exportIdentity() {
  if (!state.secretKey || !state.account) throw new Error('当前没有可导出的签名身份');
  const backupPassword = password('输入备份密码（至少 8 个字符）');
  const salt = nacl.randomBytes(16);
  const nonce = nacl.randomBytes(nacl.secretbox.nonceLength);
  const iterations = 210_000;
  const key = deriveBackupKey(backupPassword, salt, iterations);
  const publicKey = state.secretKey.slice(32);
  const backup: IdentityBackup = { format: 'aomori-ed25519-backup', version: 1, account: state.account, publicKey: bytesToHex(publicKey), salt: bytesToHex(salt), nonce: bytesToHex(nonce), ciphertext: bytesToHex(nacl.secretbox(state.secretKey, nonce, key)), iterations };
  const link = document.createElement('a');
  link.href = URL.createObjectURL(new Blob([JSON.stringify(backup, null, 2)], { type: 'application/json' }));
  link.download = `aomori-${state.account}-identity.json`;
  link.click();
  URL.revokeObjectURL(link.href);
  addLog(`已导出 ${state.account} 的加密身份备份`, 'system');
}
async function importIdentity(file: File) {
  const backup = JSON.parse(await file.text()) as IdentityBackup;
  if (backup.format !== 'aomori-ed25519-backup' || backup.version !== 1 || !backup.account || backup.iterations < 100_000) throw new Error('不支持的身份备份格式');
  const backupPassword = password('输入身份备份密码');
  const key = deriveBackupKey(backupPassword, hexToBytes(backup.salt), backup.iterations);
  const secretKey = nacl.secretbox.open(hexToBytes(backup.ciphertext), hexToBytes(backup.nonce), key);
  if (!secretKey || secretKey.length !== nacl.sign.secretKeyLength) throw new Error('备份密码错误或文件已损坏');
  const keys = nacl.sign.keyPair.fromSecretKey(secretKey);
  if (bytesToHex(keys.publicKey) !== backup.publicKey.toLowerCase()) throw new Error('备份公钥校验失败');
  selectRpc(($('rpcInput') as HTMLInputElement).value);
  const account = await rpc('aomori_get_account', { name: backup.account });
  if (!account || account.public_key?.toLowerCase() !== backup.publicKey.toLowerCase()) throw new Error('节点账户与备份公钥不匹配');
  localStorage.setItem(keyStorageName(backup.account), bytesToHex(secretKey));
  loadIdentity(backup.account);
  addLog(`已导入 ${backup.account} 的签名身份`, 'system');
}
function transactionBytes(tx: any) { return new TextEncoder().encode(JSON.stringify({ from: tx.from, nonce: tx.nonce, entity_id: tx.entity_id, action: tx.action, args: tx.args, signature: null })); }
const readMethods = new Set(['aomori_get_info', 'aomori_get_account', 'aomori_get_entity', 'aomori_list_entities', 'aomori_get_quests', 'aomori_get_events', 'aomori_query']);
class RpcError extends Error { constructor(message: string, readonly code?: number, readonly data?: Record<string, unknown>) { super(message); this.name = 'RpcError'; } }
function wait(ms: number) { return new Promise(resolve => window.setTimeout(resolve, ms)); }
async function rpc(method: string, params: object, adminToken?: string) {
  const headers: Record<string, string> = { 'content-type': 'application/json' };
  if (adminToken) headers.authorization = `Bearer ${adminToken}`;
  for (let attempt = 0; attempt < 2; attempt++) {
    const response = await fetch(`${state.rpc}/rpc`, { method: 'POST', headers, body: JSON.stringify({ jsonrpc: '2.0', id: Date.now(), method, params }) });
    const body: RpcResult = await response.json();
    const retryAfterMs = Number(body.error?.data?.retry_after_ms);
    if (response.status === 429 && body.error?.code === -32004 && readMethods.has(method) && attempt === 0) {
      const headerSeconds = Number(response.headers.get('retry-after'));
      const delay = Number.isFinite(retryAfterMs) ? Math.max(1, Math.min(retryAfterMs, 2_000)) : Number.isFinite(headerSeconds) ? Math.max(1, Math.min(headerSeconds * 1_000, 2_000)) : 1_000;
      await wait(delay);
      continue;
    }
    if (body.error) {
      const message = body.error.code === -32004 && Number.isFinite(retryAfterMs) ? `请求过于频繁，请在 ${Math.ceil(retryAfterMs)} 毫秒后重试` : body.error.message;
      throw new RpcError(message, body.error.code, body.error.data);
    }
    if (!response.ok) throw new RpcError(`HTTP ${response.status}`);
    return body.result;
  }
  throw new RpcError('RPC retry exhausted');
}
function updateReceipt(receipt: any) { $('receipt').innerHTML = `<div class="receipt-row"><span>状态</span><strong class="${receipt.ok ? 'ok' : 'bad'}">${receipt.ok ? 'SUCCESS' : 'FAILED'}</strong></div><div class="receipt-row"><span>交易</span><code>${escapeHtml(receipt.tx_id || 'query')}</code></div><div class="receipt-row"><span>state root</span><code>${escapeHtml(receipt.state_root || '-')}</code></div>`; }
function renderEvent(event: WorldEvent) { if (state.seenEvents.has(event.id)) return; state.seenEvents.add(event.id); const list = $('eventList'); if (list.querySelector('.muted')) list.innerHTML = ''; const row = document.createElement('div'); row.className = 'event'; row.innerHTML = `<div><span class="event-kind">${escapeHtml(event.kind)}</span><span class="event-id">#${event.id}</span></div><p>${escapeHtml(JSON.stringify(event.data))}</p>`; list.prepend(row); $('eventCount').textContent = String(state.seenEvents.size); state.lastEvent = Math.max(state.lastEvent, event.id); }
function isLagMessage(value: WorldEvent | EventStreamLagged): value is EventStreamLagged { return 'type' in value && value.type === 'event_stream_lagged'; }
function recoverEvents() { if (!state.recoveringEvents) state.recoveringEvents = refreshEvents().finally(() => { state.recoveringEvents = null; }); return state.recoveringEvents; }
function connectEvents() { state.socket?.close(); state.socket = new WebSocket(state.rpc.replace(/^http/, 'ws') + '/events'); state.socket.onopen = () => { addLog('实时事件通道已连接', 'system'); recoverEvents().catch(error => addLog(`事件补偿失败: ${error.message}`, 'error')); }; state.socket.onmessage = event => { const value = JSON.parse(event.data) as WorldEvent | EventStreamLagged; if (isLagMessage(value)) { addLog(`事件流落后 ${value.missed} 条，正在补偿`, 'error'); recoverEvents().catch(error => addLog(`事件补偿失败: ${error.message}`, 'error')); } else renderEvent(value); }; state.socket.onclose = () => { addLog('实时事件通道已断开，准备重连', 'error'); window.clearTimeout(state.reconnectTimer); state.reconnectTimer = window.setTimeout(connectEvents, 3000); }; }
async function refreshEvents() { let since = state.lastEvent; while (true) { const result = await rpc('aomori_get_events', { since, limit: 500 }); result.events.forEach(renderEvent); if (!result.events.length || result.events.length < 500 || result.next <= since) break; since = result.next; } }
async function renderRoomEntities(location: number) { const entities = await rpc('aomori_list_entities', { location }); const room = entities.filter((entity: any) => entity.id !== state.actor && entity.kind !== 'zone'); state.roomActors = room.filter((entity: any) => entity.kind === 'actor'); $('roomEntities').innerHTML = room.length ? room.map((entity: any) => { const name = entity.data?.name || `${entity.kind} #${entity.id}`; const action = entity.kind === 'item' ? `take ${entity.id}` : `talk ${entity.id}`; const available = state.quests.filter(quest => quest.status === 'available' && quest.giver_entity_id === entity.id); const extra = available.map(quest => `<button class="mini-action" data-command="accept ${entity.id} ${escapeHtml(quest.id)}">接取 ${escapeHtml(quest.title)}</button>`).join(''); return `<div class="entity-card"><button class="entity-action" data-command="${action}"><span>${escapeHtml(name)}</span><small>#${entity.id} · ${action}</small></button>${extra}</div>`; }).join('') : '<span class="muted">这里没有其他实体</span>'; }
async function refreshInventory() { const result = await rpc('aomori_query', { entity_id: state.actor, action: 'inventory', args: {} }); const ids = result.result.items || []; if (!ids.length) { $('inventory').innerHTML = '<span class="muted">暂无物品</span>'; return; } const entities = await Promise.all(ids.map((id: number) => rpc('aomori_get_entity', { entity_id: id }))); $('inventory').innerHTML = entities.map((entity: any) => { const targets = state.roomActors.map(target => `<option value="${target.id}">${escapeHtml(target.data?.name || `Actor #${target.id}`)}</option>`).join(''); return `<div class="inventory-item"><span>${escapeHtml(entity?.data?.name || `Item #${entity?.id}`)}</span><small>#${entity?.id}</small><button class="inventory-action" data-command="drop ${entity?.id}">丢弃</button>${targets ? `<select class="transfer-target" data-item-id="${entity.id}" aria-label="转移目标"><option value="">给予...</option>${targets}</select>` : ''}</div>`; }).join(''); }
async function look() { const result = await rpc('aomori_query', { entity_id: state.actor, action: 'look', args: {} }); updateReceipt(result); $('location').textContent = String(result.result.location); $('zoneName').textContent = result.result.name || `Zone #${result.result.location}`; result.messages?.forEach((message: string) => addLog(message)); const entity = await rpc('aomori_get_entity', { entity_id: result.result.location }); const exits = entity?.data?.exits || {}; $('exits').innerHTML = Object.keys(exits).map(direction => `<button class="exit" data-direction="${escapeHtml(direction)}">${escapeHtml(direction)} <span>→</span></button>`).join('') || '<span class="muted">没有出口</span>'; await renderRoomEntities(result.result.location); await refreshInventory(); $('head').textContent = String((await rpc('aomori_get_info', {})).head); }
async function refreshStatus() { const result = await rpc('aomori_get_quests', { actor_id: state.actor }); const quests = result.quests || []; state.quests = quests; $('questList').innerHTML = quests.length ? quests.map((quest: any) => `<div class="quest-item"><div class="quest-head"><strong>${escapeHtml(quest.title)}</strong><span class="quest-status ${escapeHtml(quest.status)}">${escapeHtml(quest.status)}</span></div><dl><div><dt>目标</dt><dd>${escapeHtml(quest.required_item)}</dd></div><div><dt>交付</dt><dd>${escapeHtml(quest.completion_zone)}</dd></div><div><dt>奖励</dt><dd>${quest.reward_balance} coins</dd></div>${quest.prerequisite_quest_ids?.length ? `<div><dt>前置</dt><dd>${quest.prerequisite_quest_ids.map((id: string) => escapeHtml(id)).join(', ')}</dd></div>` : ''}</dl>${quest.status === 'accepted' ? `<button class="task-button" data-command="complete ${escapeHtml(quest.id)}">完成任务</button>` : ''}</div>`).join('') : '<span class="muted">暂无任务</span>';const actor = await rpc('aomori_get_entity', { entity_id: state.actor }); const account = actor?.owner ? await rpc('aomori_get_account', { name: actor.owner }) : null; $('balance').textContent = String(account?.balance || 0); }
async function submitCommand(action: string, args: Record<string, unknown>) {
  if (!state.secretKey || !state.account) return rpc('aomori_command', { entity_id: state.actor, action, args });
  for (let attempt = 0; attempt < 2; attempt++) {
    const account = await rpc('aomori_get_account', { name: state.account });
    if (!account) throw new Error(`账户不存在: ${state.account}`);
    const tx = { from: state.account, nonce: account.nonce, entity_id: state.actor, action, args, signature: null as string | null };
    tx.signature = bytesToHex(nacl.sign.detached(transactionBytes(tx), state.secretKey));
    try { return await rpc('aomori_submit_transaction', tx); }
    catch (error) { if (attempt === 0 && (error as Error).message.startsWith('invalid nonce')) continue; throw error; }
  }
  throw new Error('签名交易重试失败');
}
async function command(raw: string) {
  const parts = raw.trim().split(/\s+/);
  if (!parts[0]) return;
  if (parts[0] === 'look') return look();
  if (parts[0] === 'inventory' || parts[0] === 'i') {
    const result = await rpc('aomori_query', { entity_id: state.actor, action: 'inventory', args: {} });
    addLog(`inventory: ${JSON.stringify(result.result.items || [])}`);
    return;
  }
  if (parts[0] === 'status') {
    await refreshStatus();
    addLog(`任务状态已刷新，balance: ${$('balance').textContent}`);
    return;
  }
  const action = parts[0];
  let args: Record<string, unknown> = {};
  if (action === 'go' && parts[1]) args = { direction: parts[1] };
  else if (action === 'take' && parts[1]) args = { item_id: Number(parts[1]) };
  else if (action === 'drop' && parts[1]) args = { item_id: Number(parts[1]) };
  else if (action === 'give' && parts[1] && parts[2]) args = { item_id: Number(parts[1]), target_id: Number(parts[2]) };
  else if (action === 'talk' && parts[1]) args = { npc_id: Number(parts[1]) };
  else if (action === 'accept' && parts[1]) args = { npc_id: Number(parts[1]), quest_id: parts[2] || 'lost_key' };
  else if (action === 'complete') args = { quest_id: parts[1] || 'lost_key' };
  else throw new Error('支持 look、go <direction>、take <item_id>、drop <item_id>、give <item_id> <target_id>、talk <npc_id>、accept <npc_id>、complete <quest_id>、status、inventory');
  const result = await submitCommand(action, args);
  updateReceipt(result);
  result.messages?.forEach((message: string) => addLog(message));
  await refreshStatus();
  await refreshInventory();
  if (action === 'go' || action === 'accept' || action === 'complete') await look();
}
async function connect() { selectRpc(($('rpcInput') as HTMLInputElement).value); state.actor = Number(($('actorInput') as HTMLInputElement).value); try { await rpc('aomori_get_info', {}); const actor = await rpc('aomori_get_entity', { entity_id: state.actor }); loadIdentity(actor?.owner || ''); setStatus(true, '节点在线'); addLog('已连接 Aomori 节点', 'system'); connectEvents(); await refreshEvents(); await refreshStatus(); await look(); } catch (error) { setStatus(false, '连接失败'); addLog((error as Error).message, 'error'); } }
async function createIdentity() {
  selectRpc(($('rpcInput') as HTMLInputElement).value);
  const account = ($('accountInput') as HTMLInputElement).value.trim();
  const adminToken = ($('adminTokenInput') as HTMLInputElement).value;
  if (!account || !adminToken) throw new Error('账户名和管理员 Token 必填');
  const keys = nacl.sign.keyPair();
  await rpc('aomori_create_account', { name: account, public_key: bytesToHex(keys.publicKey), balance: 0 }, adminToken);
  const created = await rpc('aomori_create_entity', { kind: 'actor', owner: account, contract: 'demo', location: 1, data: { name: account } }, adminToken);
  localStorage.setItem(keyStorageName(account), bytesToHex(keys.secretKey));
  ($('adminTokenInput') as HTMLInputElement).value = '';
  ($('actorInput') as HTMLInputElement).value = String(created.entity_id);
  state.actor = created.entity_id;
  loadIdentity(account);
  addLog(`已创建签名身份 ${account}，Actor #${state.actor}`, 'system');
  await connect();
}
$('connectBtn').onclick = connect;
$('createIdentityBtn').onclick = () => createIdentity().catch(error => addLog(error.message, 'error'));
$('exportIdentityBtn').onclick = () => { try { exportIdentity(); } catch (error) { addLog((error as Error).message, 'error'); } };
$('importIdentityBtn').onclick = () => ($('importIdentityFile') as HTMLInputElement).click();
$('importIdentityFile').addEventListener('change', event => { const input = event.target as HTMLInputElement; const file = input.files?.[0]; if (file) importIdentity(file).catch(error => addLog(error.message, 'error')).finally(() => { input.value = ''; }); });
$('forgetIdentityBtn').onclick = () => { if (state.account) localStorage.removeItem(keyStorageName(state.account)); state.secretKey = null; $('writeMode').textContent = '开发 command'; $('forgetIdentityBtn').hidden = true; $('exportIdentityBtn').hidden = true; addLog('已清除当前账户的本地私钥', 'system'); };
$('lookBtn').onclick = () => look().catch(error => addLog(error.message, 'error')); const commandInput = $('commandInput') as HTMLInputElement; commandInput.addEventListener('keydown', event => { if (event.key === 'ArrowUp') { event.preventDefault(); state.historyIndex = Math.max(0, state.historyIndex - 1); commandInput.value = state.history[state.historyIndex] || ''; } if (event.key === 'ArrowDown') { event.preventDefault(); state.historyIndex = Math.min(state.history.length, state.historyIndex + 1); commandInput.value = state.history[state.historyIndex] || ''; } }); $('commandForm').addEventListener('submit', event => { event.preventDefault(); const input = $('commandInput') as HTMLInputElement; const value = input.value.trim(); if (!value) return; state.history = [...state.history.filter(item => item !== value), value].slice(-50); state.historyIndex = state.history.length; addLog(`> ${value}`, 'command'); input.value = ''; command(value).catch(error => addLog(error.message, 'error')); }); $('exits').addEventListener('click', event => { const button = (event.target as HTMLElement).closest<HTMLButtonElement>('[data-direction]'); if (button) command(`go ${button.dataset.direction}`).catch(error => addLog(error.message, 'error')); });
$('roomEntities').addEventListener('click', event => { const button = (event.target as HTMLElement).closest<HTMLButtonElement>('[data-command]'); if (button) command(button.dataset.command || '').catch(error => addLog(error.message, 'error')); });
$('inventory').addEventListener('click', event => { const button = (event.target as HTMLElement).closest<HTMLButtonElement>('[data-command]'); if (button) command(button.dataset.command || '').catch(error => addLog(error.message, 'error')); });
$('inventory').addEventListener('change', event => { const select = (event.target as HTMLElement).closest<HTMLSelectElement>('.transfer-target'); if (select?.value) command(`give ${select.dataset.itemId} ${select.value}`).catch(error => addLog(error.message, 'error')); });
$('questList').addEventListener('click', event => { const button = (event.target as HTMLElement).closest<HTMLButtonElement>('[data-command]'); if (button) command(button.dataset.command || '').catch(error => addLog(error.message, 'error')); });
addLog('输入节点地址与 Actor ID 后连接', 'system');
