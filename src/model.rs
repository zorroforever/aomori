use anyhow::{anyhow, Result};
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

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Entity {
    pub id: EntityId,
    pub kind: EntityKind,
    pub owner: String,
    pub location: Option<EntityId>,
    pub contract: Option<String>,
    pub data: BTreeMap<String, Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub enum ContractStatus {
    #[default]
    Published,
    Deprecated,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Contract {
    pub name: String,
    #[serde(default = "default_contract_version")]
    pub version: u32,
    pub source: String,
    pub source_hash: String,
    #[serde(default)]
    pub status: ContractStatus,
}

fn default_contract_version() -> u32 {
    1
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Account {
    pub name: String,
    pub public_key: Option<String>,
    pub nonce: u64,
    pub balance: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Transaction {
    pub from: String,
    pub nonce: u64,
    pub entity_id: EntityId,
    pub action: String,
    pub args: Value,
    pub signature: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Receipt {
    pub tx_id: String,
    pub from: String,
    pub nonce: u64,
    pub ok: bool,
    pub messages: Vec<String>,
    pub result: Value,
    pub state_root: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum QuestStatus {
    Accepted,
    Completed,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct QuestDefinition {
    pub id: String,
    pub title: String,
    #[serde(default)]
    pub giver_entity_id: EntityId,
    #[serde(default)]
    pub prerequisite_quest_ids: Vec<String>,
    pub required_item: String,
    pub completion_zone: String,
    pub reward_balance: u64,
    #[serde(default)]
    pub consume_required_item: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct QuestProgress {
    pub quest_id: String,
    pub actor_id: EntityId,
    pub status: QuestStatus,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorldEvent {
    pub id: u64,
    pub head: u64,
    pub kind: String,
    pub entity_id: Option<EntityId>,
    pub data: Value,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorldState {
    pub accounts: BTreeMap<String, Account>,
    pub entities: BTreeMap<EntityId, Entity>,
    pub contracts: BTreeMap<String, Contract>,
    pub receipts: BTreeMap<String, Receipt>,
    #[serde(default)]
    pub quests: BTreeMap<String, QuestDefinition>,
    #[serde(default)]
    pub quest_progress: BTreeMap<String, QuestProgress>,
    #[serde(default)]
    pub inventories: BTreeMap<EntityId, Vec<EntityId>>,
    #[serde(default)]
    pub events: Vec<WorldEvent>,
    #[serde(default = "default_next_event_id")]
    pub next_event_id: u64,
    pub next_entity_id: EntityId,
    pub head: u64,
}

fn default_next_event_id() -> u64 {
    1
}

fn validate_hash(label: &str, value: &str) -> Result<()> {
    let bytes = hex::decode(value).map_err(|_| anyhow!("{label} is not hex"))?;
    if bytes.len() != 32 {
        return Err(anyhow!("{label} must be 32 bytes"));
    }
    Ok(())
}

impl WorldState {
    pub fn genesis() -> Self {
        let mut accounts = BTreeMap::new();
        accounts.insert(
            "admin".into(),
            Account {
                name: "admin".into(),
                public_key: None,
                nonce: 0,
                balance: 0,
            },
        );
        Self {
            accounts,
            entities: BTreeMap::new(),
            contracts: BTreeMap::new(),
            receipts: BTreeMap::new(),
            quests: BTreeMap::new(),
            quest_progress: BTreeMap::new(),
            inventories: BTreeMap::new(),
            events: Vec::new(),
            next_event_id: 1,
            next_entity_id: 1,
            head: 0,
        }
    }

    pub fn validate_events(&self) -> Result<()> {
        let mut previous_id = 0;
        let mut transaction_events = std::collections::BTreeSet::new();
        for event in &self.events {
            if event.id == 0 || event.id <= previous_id {
                return Err(anyhow!(
                    "event ids must be positive and strictly increasing: {} after {previous_id}",
                    event.id
                ));
            }
            previous_id = event.id;
            if event.head > self.head {
                return Err(anyhow!(
                    "event {} head exceeds world head: {} > {}",
                    event.id,
                    event.head,
                    self.head
                ));
            }
            if event.kind.is_empty() {
                return Err(anyhow!("event {} kind must not be empty", event.id));
            }

            match event.kind.as_str() {
                "transaction_executed" => {
                    let tx_id =
                        event
                            .data
                            .get("tx_id")
                            .and_then(Value::as_str)
                            .ok_or_else(|| {
                                anyhow!("transaction event {} is missing tx_id", event.id)
                            })?;
                    let from = event
                        .data
                        .get("from")
                        .and_then(Value::as_str)
                        .ok_or_else(|| anyhow!("transaction event {} is missing from", event.id))?;
                    let nonce =
                        event
                            .data
                            .get("nonce")
                            .and_then(Value::as_u64)
                            .ok_or_else(|| {
                                anyhow!("transaction event {} is missing nonce", event.id)
                            })?;
                    let receipt = self.receipts.get(tx_id).ok_or_else(|| {
                        anyhow!(
                            "transaction event {} receipt does not exist: {tx_id}",
                            event.id
                        )
                    })?;
                    if receipt.from != from || receipt.nonce != nonce {
                        return Err(anyhow!(
                            "transaction event {} does not match receipt {tx_id}",
                            event.id
                        ));
                    }
                    if !transaction_events.insert(tx_id) {
                        return Err(anyhow!("duplicate transaction event for receipt: {tx_id}"));
                    }
                }
                "entity_changed" => {
                    if event.entity_id.is_none() {
                        return Err(anyhow!(
                            "entity changed event {} is missing entity_id",
                            event.id
                        ));
                    }
                    if !matches!(
                        event.data.get("change").and_then(Value::as_str),
                        Some("created" | "updated" | "deleted")
                    ) {
                        return Err(anyhow!(
                            "entity changed event {} has invalid change",
                            event.id
                        ));
                    }
                }
                "quest_progress_changed" => {
                    if event.entity_id.is_none() {
                        return Err(anyhow!(
                            "quest progress event {} is missing actor entity_id",
                            event.id
                        ));
                    }
                    if event
                        .data
                        .get("quest_id")
                        .and_then(Value::as_str)
                        .filter(|id| !id.is_empty())
                        .is_none()
                    {
                        return Err(anyhow!(
                            "quest progress event {} is missing quest_id",
                            event.id
                        ));
                    }
                    if !matches!(
                        event.data.get("status").and_then(Value::as_str),
                        Some("accepted" | "completed")
                    ) {
                        return Err(anyhow!(
                            "quest progress event {} has invalid status",
                            event.id
                        ));
                    }
                }
                "command_executed" => {
                    if event.entity_id.is_none() {
                        return Err(anyhow!("command event {} is missing entity_id", event.id));
                    }
                    if event
                        .data
                        .get("action")
                        .and_then(Value::as_str)
                        .filter(|action| !action.is_empty())
                        .is_none()
                    {
                        return Err(anyhow!("command event {} is missing action", event.id));
                    }
                }
                _ => {}
            }
        }
        if self.next_event_id == 0 || self.next_event_id <= previous_id {
            return Err(anyhow!(
                "next_event_id must be positive and greater than every existing event id"
            ));
        }
        Ok(())
    }

    pub fn validate_references(&self) -> Result<()> {
        for (key, account) in &self.accounts {
            if key != &account.name {
                return Err(anyhow!(
                    "account map key does not match account name: {key} != {}",
                    account.name
                ));
            }
            if account.name.is_empty() {
                return Err(anyhow!("account name must not be empty"));
            }
            if let Some(public_key) = &account.public_key {
                let bytes = hex::decode(public_key)
                    .map_err(|_| anyhow!("account {} public key is not hex", account.name))?;
                if bytes.len() != 32 {
                    return Err(anyhow!(
                        "account {} public key must be 32 bytes",
                        account.name
                    ));
                }
            }
        }

        for (key, contract) in &self.contracts {
            if contract.name.is_empty() {
                return Err(anyhow!("contract name must not be empty"));
            }
            if contract.version == 0 {
                return Err(anyhow!(
                    "contract {} version must be greater than zero",
                    key
                ));
            }
            let expected_key = if contract.version == 1 {
                contract.name.clone()
            } else {
                format!("{}@{}", contract.name, contract.version)
            };
            if key != &expected_key {
                return Err(anyhow!(
                    "contract map key does not match name and version: {key} != {expected_key}"
                ));
            }
            let expected_hash = blake3::hash(contract.source.as_bytes())
                .to_hex()
                .to_string();
            if contract.source_hash != expected_hash {
                return Err(anyhow!("contract {key} source hash does not match source"));
            }
        }

        for (key, entity) in &self.entities {
            if key != &entity.id {
                return Err(anyhow!(
                    "entity map key does not match entity id: {key} != {}",
                    entity.id
                ));
            }
            if !self.accounts.contains_key(&entity.owner) {
                return Err(anyhow!(
                    "entity {} owner account does not exist: {}",
                    entity.id,
                    entity.owner
                ));
            }
            if let Some(contract) = &entity.contract {
                if !self.contracts.contains_key(contract) {
                    return Err(anyhow!(
                        "entity {} contract does not exist: {contract}",
                        entity.id
                    ));
                }
            }
        }
        if self
            .entities
            .keys()
            .next_back()
            .map(|id| self.next_entity_id <= *id)
            .unwrap_or(self.next_entity_id == 0)
        {
            return Err(anyhow!(
                "next_entity_id must be greater than every existing entity id"
            ));
        }

        for (key, progress) in &self.quest_progress {
            let expected_key = format!("{}:{}", progress.actor_id, progress.quest_id);
            if key != &expected_key {
                return Err(anyhow!(
                    "quest progress key does not match actor and quest: {key} != {expected_key}"
                ));
            }
            let actor = self.entities.get(&progress.actor_id).ok_or_else(|| {
                anyhow!("quest progress actor does not exist: {}", progress.actor_id)
            })?;
            if actor.kind != EntityKind::Actor {
                return Err(anyhow!(
                    "quest progress entity is not an actor: {}",
                    progress.actor_id
                ));
            }
            if !self.quests.contains_key(&progress.quest_id) {
                return Err(anyhow!(
                    "quest progress definition does not exist: {}",
                    progress.quest_id
                ));
            }
        }

        for (key, receipt) in &self.receipts {
            if key != &receipt.tx_id {
                return Err(anyhow!(
                    "receipt map key does not match transaction id: {key} != {}",
                    receipt.tx_id
                ));
            }
            if !self.accounts.contains_key(&receipt.from) {
                return Err(anyhow!("receipt account does not exist: {}", receipt.from));
            }
            validate_hash("receipt transaction id", &receipt.tx_id)?;
            validate_hash("receipt state root", &receipt.state_root)?;
        }
        Ok(())
    }

    pub fn validate_locations(&self) -> Result<()> {
        for entity in self.entities.values() {
            match (entity.kind.clone(), entity.location) {
                (EntityKind::Zone, Some(location)) => {
                    return Err(anyhow!(
                        "zone {} must not have a location: {location}",
                        entity.id
                    ));
                }
                (EntityKind::Actor, Some(location)) => {
                    let target = self.entities.get(&location).ok_or_else(|| {
                        anyhow!("actor {} location does not exist: {location}", entity.id)
                    })?;
                    if target.kind != EntityKind::Zone {
                        return Err(anyhow!(
                            "actor {} location is not a zone: {location}",
                            entity.id
                        ));
                    }
                }
                (EntityKind::Item, Some(location)) => {
                    let target = self.entities.get(&location).ok_or_else(|| {
                        anyhow!("item {} location does not exist: {location}", entity.id)
                    })?;
                    if !matches!(target.kind, EntityKind::Zone | EntityKind::Actor) {
                        return Err(anyhow!(
                            "item {} location is neither a zone nor an actor: {location}",
                            entity.id
                        ));
                    }
                }
                _ => {}
            }
        }
        Ok(())
    }

    pub fn validate_inventories(&self) -> Result<()> {
        let mut held_by = BTreeMap::new();
        for (owner_id, item_ids) in &self.inventories {
            let owner = self
                .entities
                .get(owner_id)
                .ok_or_else(|| anyhow!("inventory owner does not exist: {owner_id}"))?;
            if owner.kind != EntityKind::Actor {
                return Err(anyhow!("inventory owner is not an actor: {owner_id}"));
            }
            let mut owner_items = std::collections::BTreeSet::new();
            for item_id in item_ids {
                if !owner_items.insert(*item_id) {
                    return Err(anyhow!(
                        "inventory {owner_id} contains duplicate item: {item_id}"
                    ));
                }
                let item = self.entities.get(item_id).ok_or_else(|| {
                    anyhow!("inventory {owner_id} item does not exist: {item_id}")
                })?;
                if item.kind != EntityKind::Item {
                    return Err(anyhow!(
                        "inventory {owner_id} entity is not an item: {item_id}"
                    ));
                }
                if let Some(previous_owner) = held_by.insert(*item_id, *owner_id) {
                    return Err(anyhow!(
                        "item {item_id} is held by multiple actors: {previous_owner} and {owner_id}"
                    ));
                }
                if item.location != Some(*owner_id) {
                    return Err(anyhow!(
                        "item {item_id} location does not match inventory owner {owner_id}"
                    ));
                }
            }
        }

        for item in self
            .entities
            .values()
            .filter(|entity| entity.kind == EntityKind::Item)
        {
            if let Some(location) = item.location {
                if self
                    .entities
                    .get(&location)
                    .map(|entity| entity.kind == EntityKind::Actor)
                    .unwrap_or(false)
                    && held_by.get(&item.id) != Some(&location)
                {
                    return Err(anyhow!(
                        "item {} located at actor {location} is missing from inventory",
                        item.id
                    ));
                }
            }
        }
        Ok(())
    }

    pub fn validate(&self) -> Result<()> {
        self.validate_references()?;
        self.validate_events()?;
        self.validate_locations()?;
        self.validate_quests()?;
        self.validate_inventories()?;
        Ok(())
    }

    pub fn validate_quests(&self) -> Result<()> {
        for (key, quest) in &self.quests {
            if key != &quest.id {
                return Err(anyhow!(
                    "quest map key does not match definition id: {key} != {}",
                    quest.id
                ));
            }
            if quest.id.is_empty() {
                return Err(anyhow!("quest id must not be empty"));
            }
            let giver = self.entities.get(&quest.giver_entity_id).ok_or_else(|| {
                anyhow!(
                    "quest {} giver does not exist: {}",
                    quest.id,
                    quest.giver_entity_id
                )
            })?;
            if giver.kind != EntityKind::Actor {
                return Err(anyhow!("quest {} giver is not an actor", quest.id));
            }
            let mut seen = std::collections::BTreeSet::new();
            for prerequisite in &quest.prerequisite_quest_ids {
                if prerequisite == &quest.id {
                    return Err(anyhow!("quest {} cannot require itself", quest.id));
                }
                if !seen.insert(prerequisite) {
                    return Err(anyhow!(
                        "quest {} has duplicate prerequisite: {prerequisite}",
                        quest.id
                    ));
                }
                if !self.quests.contains_key(prerequisite) {
                    return Err(anyhow!(
                        "quest {} prerequisite does not exist: {prerequisite}",
                        quest.id
                    ));
                }
            }
        }

        let mut indegree: BTreeMap<String, usize> = self
            .quests
            .iter()
            .map(|(id, quest)| (id.clone(), quest.prerequisite_quest_ids.len()))
            .collect();
        let mut dependents: BTreeMap<String, Vec<String>> = BTreeMap::new();
        for (id, quest) in &self.quests {
            for prerequisite in &quest.prerequisite_quest_ids {
                dependents
                    .entry(prerequisite.clone())
                    .or_default()
                    .push(id.clone());
            }
        }
        let mut ready: std::collections::VecDeque<String> = indegree
            .iter()
            .filter_map(|(id, degree)| (*degree == 0).then_some(id.clone()))
            .collect();
        let mut processed = 0;
        while let Some(id) = ready.pop_front() {
            processed += 1;
            if let Some(children) = dependents.get(&id) {
                for child in children {
                    let degree = indegree.get_mut(child).unwrap();
                    *degree -= 1;
                    if *degree == 0 {
                        ready.push_back(child.clone());
                    }
                }
            }
        }
        if processed != self.quests.len() {
            let id = indegree
                .iter()
                .find_map(|(id, degree)| (*degree > 0).then_some(id.as_str()))
                .unwrap_or("unknown");
            return Err(anyhow!("quest prerequisite cycle detected at: {id}"));
        }
        Ok(())
    }

    pub fn root(&self) -> String {
        let mut canonical = self.clone();
        canonical.receipts.clear();
        let bytes = serde_json::to_vec(&canonical).expect("state is serializable");
        blake3::hash(&bytes).to_hex().to_string()
    }
}
