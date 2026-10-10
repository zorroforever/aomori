//! Ledger v1 primitives only. Not connected to execution, RPC or persistence.
use anyhow::{bail, ensure, Result};
use ed25519_dalek::{Signature, VerifyingKey};
use serde::{Deserialize, Serialize};
use serde_json::Value;

pub const PROTOCOL_VERSION: u32 = 1;
pub const MAX_BYTES: usize = 1_048_576;
pub const MAX_DEPTH: usize = 32;
pub const MAX_INTEGER: i64 = 9_007_199_254_740_991;

/// Protocol JSON: recursively sorted UTF-8 keys, no floats, safe integers.
pub fn canonical_json(value: &Value) -> Result<Vec<u8>> {
    fn normalize(value: &Value, depth: usize) -> Result<Value> {
        ensure!(depth <= MAX_DEPTH, "JSON depth exceeded");
        Ok(match value {
            Value::Number(n) => {
                let integer = n
                    .as_i64()
                    .ok_or_else(|| anyhow::anyhow!("integer required"))?;
                ensure!(
                    (-MAX_INTEGER..=MAX_INTEGER).contains(&integer),
                    "unsafe integer"
                );
                Value::from(integer)
            }
            Value::Array(items) => Value::Array(
                items
                    .iter()
                    .map(|v| normalize(v, depth + 1))
                    .collect::<Result<_>>()?,
            ),
            Value::Object(items) => {
                let mut keys: Vec<_> = items.keys().collect();
                keys.sort_by(|a, b| a.as_bytes().cmp(b.as_bytes()));
                let mut map = serde_json::Map::new();
                for key in keys {
                    map.insert(key.clone(), normalize(&items[key], depth + 1)?);
                }
                Value::Object(map)
            }
            _ => value.clone(),
        })
    }
    let bytes = serde_json::to_vec(&normalize(value, 0)?)?;
    ensure!(bytes.len() <= MAX_BYTES, "JSON size exceeded");
    Ok(bytes)
}

/// Strict wire parser rejects duplicate keys before Value could discard them.
pub fn parse_json(bytes: &[u8]) -> Result<Value> {
    ensure!(bytes.len() <= MAX_BYTES, "JSON size exceeded");
    #[derive(Debug)]
    struct Strict(Value);
    impl<'de> Deserialize<'de> for Strict {
        fn deserialize<D: serde::Deserializer<'de>>(d: D) -> std::result::Result<Self, D::Error> {
            struct Visitor;
            impl<'de> serde::de::Visitor<'de> for Visitor {
                type Value = Strict;
                fn expecting(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
                    f.write_str("protocol JSON")
                }
                fn visit_bool<E: serde::de::Error>(
                    self,
                    v: bool,
                ) -> std::result::Result<Strict, E> {
                    Ok(Strict(v.into()))
                }
                fn visit_i64<E: serde::de::Error>(self, v: i64) -> std::result::Result<Strict, E> {
                    Ok(Strict(v.into()))
                }
                fn visit_u64<E: serde::de::Error>(self, v: u64) -> std::result::Result<Strict, E> {
                    Ok(Strict(v.into()))
                }
                fn visit_f64<E: serde::de::Error>(self, _: f64) -> std::result::Result<Strict, E> {
                    Err(E::custom("float forbidden"))
                }
                fn visit_str<E: serde::de::Error>(self, v: &str) -> std::result::Result<Strict, E> {
                    Ok(Strict(v.into()))
                }
                fn visit_unit<E: serde::de::Error>(self) -> std::result::Result<Strict, E> {
                    Ok(Strict(Value::Null))
                }
                fn visit_seq<A: serde::de::SeqAccess<'de>>(
                    self,
                    mut a: A,
                ) -> std::result::Result<Strict, A::Error> {
                    let mut values = Vec::new();
                    while let Some(Strict(v)) = a.next_element()? {
                        values.push(v);
                    }
                    Ok(Strict(Value::Array(values)))
                }
                fn visit_map<A: serde::de::MapAccess<'de>>(
                    self,
                    mut a: A,
                ) -> std::result::Result<Strict, A::Error> {
                    let mut map = serde_json::Map::new();
                    while let Some(key) = a.next_key::<String>()? {
                        if map.contains_key(&key) {
                            return Err(serde::de::Error::custom("duplicate key"));
                        }
                        let Strict(v) = a.next_value()?;
                        map.insert(key, v);
                    }
                    Ok(Strict(Value::Object(map)))
                }
            }
            d.deserialize_any(Visitor)
        }
    }
    let Strict(value) = serde_json::from_slice(bytes)?;
    canonical_json(&value)?;
    Ok(value)
}

#[derive(Debug, Clone, Copy)]
pub enum Domain {
    Sign,
    Transaction,
    Genesis,
    Block,
    Transactions,
    Results,
    Events,
    State,
}
impl Domain {
    pub fn label(self) -> &'static str {
        match self {
            Self::Sign => "aomori/ledger/sign/v1",
            Self::Transaction => "aomori/ledger/tx/v1",
            Self::Genesis => "aomori/ledger/genesis/v1",
            Self::Block => "aomori/ledger/block/v1",
            Self::Transactions => "aomori/ledger/transactions/v1",
            Self::Results => "aomori/ledger/results/v1",
            Self::Events => "aomori/ledger/events/v1",
            Self::State => "aomori/ledger/state/v1",
        }
    }
}
pub fn digest(domain: Domain, value: &Value) -> Result<[u8; 32]> {
    let mut hash = blake3::Hasher::new();
    hash.update(domain.label().as_bytes());
    hash.update(&[0]);
    hash.update(&canonical_json(value)?);
    Ok(*hash.finalize().as_bytes())
}
fn encoded<T: Serialize>(v: &T) -> Result<Value> {
    Ok(serde_json::to_value(v)?)
}
fn hash_hex(s: &str, bytes: usize) -> Result<Vec<u8>> {
    ensure!(
        s.len() == bytes * 2
            && s.bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b)),
        "invalid lowercase hex"
    );
    Ok(hex::decode(s)?)
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LedgerTransaction {
    pub protocol_version: u32,
    pub chain_id: String,
    pub genesis_hash: String,
    pub from: String,
    pub nonce: u64,
    pub entity_id: u64,
    pub action: String,
    pub args: Value,
    pub signature: String,
}
impl LedgerTransaction {
    pub fn signing_digest(&self) -> Result<[u8; 32]> {
        ensure!(
            self.protocol_version == PROTOCOL_VERSION,
            "unsupported version"
        );
        ensure!(
            !self.chain_id.is_empty() && !self.from.is_empty() && !self.action.is_empty(),
            "empty transaction field"
        );
        hash_hex(&self.genesis_hash, 32)?;
        let mut value = encoded(self)?;
        value.as_object_mut().unwrap().remove("signature");
        digest(Domain::Sign, &value)
    }
    pub fn verify(&self, public_key: &str, chain_id: &str, genesis_hash: &str) -> Result<()> {
        ensure!(
            self.chain_id == chain_id && self.genesis_hash == genesis_hash,
            "chain mismatch"
        );
        let key: [u8; 32] = hash_hex(public_key, 32)?.try_into().unwrap();
        let signature = Signature::from_slice(&hash_hex(&self.signature, 64)?)?;
        VerifyingKey::from_bytes(&key)?.verify_strict(&self.signing_digest()?, &signature)?;
        Ok(())
    }
    pub fn tx_id(&self) -> Result<String> {
        self.signing_digest()?;
        hash_hex(&self.signature, 64)?;
        Ok(hex::encode(digest(Domain::Transaction, &encoded(self)?)?))
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GenesisManifest {
    pub protocol_version: u32,
    pub chain_id: String,
    pub runtime_rules_version: u32,
    pub encoding_version: u32,
    pub initial_world: Value,
    pub execution_limits: Value,
}
impl GenesisManifest {
    pub fn hash(&self) -> Result<String> {
        ensure!(
            self.protocol_version == 1
                && self.encoding_version == 1
                && self.runtime_rules_version == 1,
            "unsupported genesis version"
        );
        ensure!(!self.chain_id.is_empty(), "empty chain id");
        Ok(hex::encode(digest(Domain::Genesis, &encoded(self)?)?))
    }
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BlockHeader {
    pub protocol_version: u32,
    pub chain_id: String,
    pub genesis_hash: String,
    pub runtime_rules_version: u32,
    pub height: u64,
    pub parent_hash: String,
    pub tx_count: u32,
    pub transactions_digest: String,
    pub results_digest: String,
    pub post_state_digest: String,
}
impl BlockHeader {
    pub fn hash(&self) -> Result<String> {
        ensure!(
            self.protocol_version == 1
                && self.runtime_rules_version == 1
                && !self.chain_id.is_empty(),
            "invalid block protocol"
        );
        if (self.height == 0 && self.tx_count != 0) || (self.height > 0 && self.tx_count != 1) {
            bail!("invalid transaction count");
        }
        for hash in [
            &self.genesis_hash,
            &self.parent_hash,
            &self.transactions_digest,
            &self.results_digest,
            &self.post_state_digest,
        ] {
            hash_hex(hash, 32)?;
        }
        ensure!(
            self.height != 0 || self.parent_hash == "0".repeat(64),
            "invalid genesis parent"
        );
        Ok(hex::encode(digest(Domain::Block, &encoded(self)?)?))
    }
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LedgerBlock {
    pub header: BlockHeader,
    pub transactions: Vec<LedgerTransaction>,
    pub execution_results: Vec<ExecutionResult>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ExecutionResult {
    pub tx_id: String,
    pub from: String,
    pub nonce: u64,
    pub ok: bool,
    pub messages: Vec<String>,
    pub result: Value,
    pub first_event_id: Option<u64>,
    pub last_event_id: Option<u64>,
    pub events_digest: String,
}
impl LedgerBlock {
    /// Structural commitments only; execution/state validation belongs to L2.
    pub fn validate_commitments(&self) -> Result<String> {
        let hash = self.header.hash()?;
        ensure!(
            self.transactions.len() == self.header.tx_count as usize
                && self.execution_results.len() == self.transactions.len(),
            "block count mismatch"
        );
        let mut ids = Vec::new();
        for (tx, result) in self.transactions.iter().zip(&self.execution_results) {
            ensure!(
                tx.chain_id == self.header.chain_id && tx.genesis_hash == self.header.genesis_hash,
                "block chain mismatch"
            );
            let id = tx.tx_id()?;
            ensure!(
                result.ok
                    && result.tx_id == id
                    && result.from == tx.from
                    && result.nonce == tx.nonce,
                "result mismatch"
            );
            hash_hex(&result.events_digest, 32)?;
            match (result.first_event_id, result.last_event_id) {
                (None, None) => {}
                (Some(first), Some(last)) if first > 0 && first <= last => {}
                _ => bail!("invalid event range"),
            }
            ids.push(id);
        }
        ensure!(
            hex::encode(digest(Domain::Transactions, &encoded(&ids)?)?)
                == self.header.transactions_digest,
            "transactions digest mismatch"
        );
        ensure!(
            hex::encode(digest(Domain::Results, &encoded(&self.execution_results)?)?)
                == self.header.results_digest,
            "results digest mismatch"
        );
        Ok(hash)
    }
}
