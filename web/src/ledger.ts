// Ledger v1 primitives; deliberately not wired into the legacy Web client.
import { blake3 } from '@noble/hashes/blake3';
const encoder = new TextEncoder();
export const MAX_BYTES = 1048576;
export const MAX_DEPTH = 32;
const domains = ['sign', 'tx', 'genesis', 'block', 'transactions', 'results', 'events', 'state'] as const;
export type Domain = typeof domains[number];
export function canonicalJson(value: unknown): string {
  function encode(v: unknown, depth: number): string {
    if (depth > MAX_DEPTH) throw new Error('JSON depth exceeded');
    if (v === null || typeof v === 'boolean') return JSON.stringify(v);
    if (typeof v === 'number') {
      if (!Number.isSafeInteger(v)) throw new Error('safe integer required');
      return JSON.stringify(v);
    }
    if (typeof v === 'string') {
      // Rust strings are Unicode scalar values, not isolated UTF-16 surrogates.
      for (let i = 0; i < v.length; i++) {
        const c = v.charCodeAt(i);
        if (c >= 0xd800 && c <= 0xdbff) {
          const next = v.charCodeAt(++i);
          if (!(next >= 0xdc00 && next <= 0xdfff)) throw new Error('invalid Unicode');
        } else if (c >= 0xdc00 && c <= 0xdfff) throw new Error('invalid Unicode');
      }
      return JSON.stringify(v);
    }
    if (Array.isArray(v)) return '[' + Array.from(v, item => encode(item, depth + 1)).join(',') + ']';
    if (typeof v === 'object' && Object.getPrototypeOf(v) === Object.prototype) {
      const obj = v as Record<string, unknown>;
      const keys = Object.keys(obj).sort((a, b) => {
        const x = encoder.encode(a), y = encoder.encode(b);
        for (let i = 0; i < Math.min(x.length, y.length); i++) if (x[i] !== y[i]) return x[i] - y[i];
        return x.length - y.length;
      });
      return '{' + keys.map(k => encode(k, depth + 1) + ':' + encode(obj[k], depth + 1)).join(',') + '}';
    }
    throw new Error('unsupported JSON value');
  }
  const result = encode(value, 0);
  if (encoder.encode(result).length > MAX_BYTES) throw new Error('JSON size exceeded');
  return result;
}
// Parse with a structural scanner before JSON.parse can erase duplicate keys.
export function parseJson(text: string): unknown {
  if (encoder.encode(text).length > MAX_BYTES) throw new Error('JSON size exceeded');
  let pos = 0;
  const whitespace = () => { while (/[\x20\t\r\n]/.test(text[pos] ?? '') && pos < text.length) pos++; };
  function string(): string {
    const start = pos++;
    while (pos < text.length) {
      if (text[pos] === '\\') { pos += 2; continue; }
      if (text[pos++] === '"') return JSON.parse(text.slice(start, pos));
    }
    throw new Error('unterminated string');
  }
  function value(depth: number): void {
    if (depth > MAX_DEPTH) throw new Error('JSON depth exceeded');
    whitespace();
    const c = text[pos];
    if (c === '"') { string(); return; }
    if (c === '{' || c === '[') {
      const object = c === '{', end = object ? '}' : ']';
      const keys = new Set<string>(); pos++; whitespace();
      if (text[pos] === end) { pos++; return; }
      while (true) {
        if (object) {
          whitespace(); if (text[pos] !== '"') throw new Error('key expected');
          const key = string(); if (keys.has(key)) throw new Error('duplicate key'); keys.add(key);
          whitespace(); if (text[pos++] !== ':') throw new Error('colon expected');
        }
        value(depth + 1); whitespace();
        if (text[pos] === end) { pos++; return; }
        if (text[pos++] !== ',') throw new Error('comma expected');
      }
    }
    const token = /^(?:null|true|false|-?(?:0|[1-9][0-9]*))/.exec(text.slice(pos));
    if (!token || token[0] === '-0') throw new Error('invalid JSON token');
    pos += token[0].length;
  }
  value(0); whitespace();
  if (pos !== text.length) throw new Error('trailing input');
  const parsed: unknown = JSON.parse(text);
  canonicalJson(parsed);
  return parsed;
}
export function digest(domain: Domain, value: unknown): string {
  if (!domains.includes(domain)) throw new Error('unknown domain');
  const prefix = encoder.encode(`aomori/ledger/${domain}/v1\0`);
  const payload = encoder.encode(canonicalJson(value));
  const bytes = new Uint8Array(prefix.length + payload.length);
  bytes.set(prefix); bytes.set(payload, prefix.length);
  return Array.from(blake3(bytes), b => b.toString(16).padStart(2, '0')).join('');
}
export interface LedgerTransaction {
  protocol_version: number; chain_id: string; genesis_hash: string;
  from: string; nonce: number; entity_id: number; action: string;
  args: unknown; signature: string;
}
export function signingDigest(tx: LedgerTransaction): string {
  const fields = ['protocol_version', 'chain_id', 'genesis_hash', 'from', 'nonce', 'entity_id', 'action', 'args', 'signature'];
  if (Object.keys(tx).length !== fields.length || !fields.every(k => Object.prototype.hasOwnProperty.call(tx, k)) || !Number.isSafeInteger(tx.nonce) || tx.nonce < 0 || !Number.isSafeInteger(tx.entity_id) || tx.entity_id < 0) throw new Error('invalid transaction fields');
  if (tx.protocol_version !== 1 || !tx.chain_id || !tx.from || !tx.action || !/^[0-9a-f]{64}$/.test(tx.genesis_hash)) throw new Error('invalid transaction');
  const { signature: _, ...unsigned } = tx;
  return digest('sign', unsigned);
}
export function transactionId(tx: LedgerTransaction): string {
  signingDigest(tx);
  if (!/^[0-9a-f]{128}$/.test(tx.signature)) throw new Error('invalid signature');
  return digest('tx', tx);
}

export interface GenesisManifest {
  protocol_version: number; chain_id: string; runtime_rules_version: number;
  encoding_version: number; initial_world: unknown; execution_limits: unknown;
}
export interface BlockHeader {
  protocol_version: number; chain_id: string; genesis_hash: string;
  runtime_rules_version: number; height: number; parent_hash: string;
  tx_count: number; transactions_digest: string; results_digest: string; post_state_digest: string;
}
export interface ExecutionResult {
  tx_id: string; from: string; nonce: number; ok: boolean; messages: string[]; result: unknown;
  first_event_id: number | null; last_event_id: number | null; events_digest: string;
}
export interface LedgerBlock {
  header: BlockHeader; transactions: LedgerTransaction[]; execution_results: ExecutionResult[];
}
