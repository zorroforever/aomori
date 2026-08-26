use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;

pub type EntityId = u64;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum EntityKind {
    Actor,
    Zone,
    Item,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Entity {
    pub id: EntityId,
    pub kind: EntityKind,
    pub owner: String,
    pub location: Option<EntityId>,
    pub contract: Option<String>,
    pub data: BTreeMap<String, Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Contract {
    pub name: String,
    pub source: String,
    pub source_hash: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Account {
    pub name: String,
    pub nonce: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorldState {
    pub accounts: BTreeMap<String, Account>,
    pub entities: BTreeMap<EntityId, Entity>,
    pub contracts: BTreeMap<String, Contract>,
    pub next_entity_id: EntityId,
    pub head: u64,
}

impl WorldState {
    pub fn genesis() -> Self {
        let mut accounts = BTreeMap::new();
        accounts.insert(
            "admin".into(),
            Account {
                name: "admin".into(),
                nonce: 0,
            },
        );
        Self {
            accounts,
            entities: BTreeMap::new(),
            contracts: BTreeMap::new(),
            next_entity_id: 1,
            head: 0,
        }
    }

    pub fn root(&self) -> String {
        let bytes = serde_json::to_vec(self).expect("state is serializable");
        blake3::hash(&bytes).to_hex().to_string()
    }
}
