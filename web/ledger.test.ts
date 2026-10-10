import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import { createPublicKey, verify } from 'node:crypto';
import { canonicalJson, digest, signingDigest, transactionId, parseJson } from './src/ledger.ts';
const vectors = JSON.parse(readFileSync(new URL('../tests/fixtures/ledger-v1.json', import.meta.url), 'utf8'));
for (const vector of vectors.codec) {
  assert.equal(canonicalJson(vector.value), vector.canonical);
  for (const domain of ['sign', 'tx', 'genesis', 'block', 'transactions', 'results', 'state'] as const) assert.equal(digest(domain, vector.value), vector.hashes[domain]);
}
const vector = vectors.transaction;
assert.equal(signingDigest(vector.value), vector.signing_digest);
assert.equal(transactionId(vector.value), vector.tx_id);
const key = createPublicKey({ key: Buffer.from('302a300506032b6570032100' + vector.public_key, 'hex'), format: 'der', type: 'spki' });
assert(verify(null, Buffer.from(signingDigest(vector.value), 'hex'), key, Buffer.from(vector.value.signature, 'hex')));
assert(!verify(null, Buffer.from(signingDigest({...vector.value, chain_id:'other'}), 'hex'), key, Buffer.from(vector.value.signature, 'hex')));
for (const text of ['{"a":1,"a":2}', '{"a":1,"\\u0061":2}', '1.0', '1e0', '-0', '9007199254740992', '"\\ud800"', '{} true']) assert.throws(() => parseJson(text));
for (const value of [1.5, NaN, Infinity, 9007199254740992, '\ud800', undefined, new Date(), new Array(1)]) assert.throws(() => canonicalJson(value));
assert.deepEqual(parseJson('{"b":2,"a":1}'), {a:1,b:2});
assert.throws(() => canonicalJson('x'.repeat(1048576)));
assert.throws(() => parseJson('['.repeat(34)+'null'+']'.repeat(34)));
console.log('Ledger v1 cross-language golden vectors and rejection tests passed');
