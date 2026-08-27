use crate::model::*;
use anyhow::{anyhow, Result};
use ed25519_dalek::{Signature, Verifier, VerifyingKey};
use mlua::{HookTriggers, Lua, Value as LuaValue, VmState};
use serde_json::Value;
use std::collections::BTreeMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

pub const DEFAULT_LUA_INSTRUCTION_LIMIT: u64 = 200_000;
pub const DEFAULT_LUA_MEMORY_LIMIT: usize = 16 * 1024 * 1024;
const LUA_HOOK_INTERVAL: u32 = 1_000;

#[derive(Clone, Copy, Debug)]
pub struct LuaLimits {
    pub instruction_limit: u64,
    pub memory_limit: usize,
}

impl Default for LuaLimits {
    fn default() -> Self {
        Self {
            instruction_limit: DEFAULT_LUA_INSTRUCTION_LIMIT,
            memory_limit: DEFAULT_LUA_MEMORY_LIMIT,
        }
    }
}

#[derive(Clone)]
struct Context {
    state: Arc<Mutex<WorldState>>,
    entity_id: EntityId,
    command: bool,
    messages: Arc<Mutex<Vec<String>>>,
}

pub fn deploy(state: &mut WorldState, name: String, source: String) -> Result<()> {
    deploy_version(state, name, 1, source)
}

pub fn deploy_version(
    state: &mut WorldState,
    name: String,
    version: u32,
    source: String,
) -> Result<()> {
    if version == 0 {
        return Err(anyhow!("contract version must be greater than zero"));
    }
    let key = contract_key(&name, version);
    if state.contracts.contains_key(&key) {
        return Err(anyhow!("contract version already exists: {key}"));
    }
    let hash = blake3::hash(source.as_bytes()).to_hex().to_string();
    state.contracts.insert(
        key,
        Contract {
            name,
            version,
            source,
            source_hash: hash,
            status: ContractStatus::Published,
        },
    );
    state.head += 1;
    Ok(())
}

pub fn contract_key(name: &str, version: u32) -> String {
    if version == 1 {
        name.to_string()
    } else {
        format!("{name}@{version}")
    }
}

pub fn create_entity(
    state: &mut WorldState,
    kind: EntityKind,
    owner: String,
    contract: Option<String>,
    location: Option<EntityId>,
    data: BTreeMap<String, Value>,
) -> Result<EntityId> {
    if !state.accounts.contains_key(&owner) {
        return Err(anyhow!("account not found: {owner}"));
    }
    if let Some(c) = &contract {
        if !state.contracts.contains_key(c) {
            return Err(anyhow!("contract not found: {c}"));
        }
    }
    let id = state.next_entity_id;
    state.next_entity_id += 1;
    state.entities.insert(
        id,
        Entity {
            id,
            kind,
            owner,
            location,
            contract,
            data,
        },
    );
    state.head += 1;
    Ok(id)
}

pub fn execute(
    state: &mut WorldState,
    entity_id: EntityId,
    action: &str,
    args: Value,
    command: bool,
) -> Result<Receipt> {
    execute_with_limits(
        state,
        entity_id,
        action,
        args,
        command,
        LuaLimits::default(),
    )
}

pub fn execute_with_limit(
    state: &mut WorldState,
    entity_id: EntityId,
    action: &str,
    args: Value,
    command: bool,
    instruction_limit: u64,
) -> Result<Receipt> {
    execute_with_limits(
        state,
        entity_id,
        action,
        args,
        command,
        LuaLimits {
            instruction_limit,
            ..LuaLimits::default()
        },
    )
}

pub fn execute_with_limits(
    state: &mut WorldState,
    entity_id: EntityId,
    action: &str,
    args: Value,
    command: bool,
    limits: LuaLimits,
) -> Result<Receipt> {
    let original = state.clone();
    let entity = state
        .entities
        .get(&entity_id)
        .cloned()
        .ok_or_else(|| anyhow!("entity not found: {entity_id}"))?;
    let contract_name = entity
        .contract
        .clone()
        .ok_or_else(|| anyhow!("entity has no contract"))?;
    let source = state
        .contracts
        .get(&contract_name)
        .ok_or_else(|| anyhow!("contract not found: {contract_name}"))?
        .source
        .clone();
    let shared = Arc::new(Mutex::new(state.clone()));
    let messages = Arc::new(Mutex::new(Vec::new()));
    let ctx = Context {
        state: shared.clone(),
        entity_id,
        command,
        messages: messages.clone(),
    };
    let lua = Lua::new();
    lua.set_memory_limit(limits.memory_limit)
        .map_err(|e| anyhow!("lua memory limit setup: {e}"))?;
    let instructions = Arc::new(AtomicU64::new(0));
    lua.set_hook(
        HookTriggers::new().every_nth_instruction(LUA_HOOK_INTERVAL),
        move |_, _| {
            let used = instructions.fetch_add(LUA_HOOK_INTERVAL as u64, Ordering::Relaxed)
                + LUA_HOOK_INTERVAL as u64;
            if used > limits.instruction_limit {
                return Err(mlua::Error::runtime(format!(
                    "Lua instruction limit exceeded ({})",
                    limits.instruction_limit
                )));
            }
            Ok(VmState::Continue)
        },
    )
    .map_err(|e| anyhow!("lua execution limit setup: {e}"))?;
    install(&lua, ctx).map_err(|e| anyhow!("lua host setup: {e}"))?;
    lua.load(&source)
        .set_name(&contract_name)
        .exec()
        .map_err(|e| anyhow!("lua load: {e}"))?;
    let globals = lua.globals();
    let function: mlua::Function = globals
        .get(format!(
            "{}_{}",
            if command { "command" } else { "query" },
            action
        ))
        .map_err(|_| anyhow!("action not found: {action}"))?;
    let lua_args = json_to_lua(&lua, &args).map_err(|e| anyhow!("lua args: {e}"))?;
    let lua_ctx = lua
        .create_table()
        .map_err(|e| anyhow!("lua context: {e}"))?;
    lua_ctx
        .set("entity_id", entity_id)
        .map_err(|e| anyhow!("lua context: {e}"))?;
    let result: LuaValue = function
        .call((lua_ctx, lua_args))
        .map_err(|e| anyhow!("lua execution: {e}"))?;
    let next = shared
        .lock()
        .map_err(|_| anyhow!("state lock poisoned"))?
        .clone();
    if command {
        *state = next;
        state.head += 1;
        let changed_ids: Vec<EntityId> = original
            .entities
            .keys()
            .chain(state.entities.keys())
            .copied()
            .collect::<std::collections::BTreeSet<_>>()
            .into_iter()
            .filter(|id| original.entities.get(id) != state.entities.get(id))
            .collect();
        for id in changed_ids {
            let change = match (original.entities.get(&id), state.entities.get(&id)) {
                (None, Some(_)) => "created",
                (Some(_), None) => "deleted",
                _ => "updated",
            };
            push_event(
                state,
                "entity_changed",
                Some(id),
                serde_json::json!({"change": change}),
            );
        }
        let changed_quests: Vec<String> = original
            .quest_progress
            .keys()
            .chain(state.quest_progress.keys())
            .cloned()
            .collect::<std::collections::BTreeSet<_>>()
            .into_iter()
            .filter(|key| original.quest_progress.get(key) != state.quest_progress.get(key))
            .collect();
        for key in changed_quests {
            if let Some(progress) = state.quest_progress.get(&key).cloned() {
                push_event(
                    state,
                    "quest_progress_changed",
                    Some(progress.actor_id),
                    serde_json::json!({
                        "quest_id": progress.quest_id,
                        "status": progress.status
                    }),
                );
            }
        }
        push_event(
            state,
            "command_executed",
            Some(entity_id),
            serde_json::json!({"action": action}),
        );
    } else {
        *state = original;
    }
    let narration = messages.lock().unwrap().clone();
    let result = lua_to_json(result).map_err(|e| anyhow!("lua result: {e}"))?;
    Ok(Receipt {
        tx_id: String::new(),
        from: String::new(),
        nonce: 0,
        ok: true,
        messages: narration,
        result,
        state_root: state.root(),
    })
}

pub fn execute_transaction(state: &mut WorldState, tx: Transaction) -> Result<Receipt> {
    execute_transaction_with_limits(state, tx, LuaLimits::default())
}

pub fn execute_transaction_with_limits(
    state: &mut WorldState,
    tx: Transaction,
    limits: LuaLimits,
) -> Result<Receipt> {
    let account = state
        .accounts
        .get(&tx.from)
        .ok_or_else(|| anyhow!("account not found: {}", tx.from))?;
    if account.nonce != tx.nonce {
        return Err(anyhow!(
            "invalid nonce: expected {}, got {}",
            account.nonce,
            tx.nonce
        ));
    }
    if let Some(public_key) = &account.public_key {
        let signature = tx
            .signature
            .as_deref()
            .ok_or_else(|| anyhow!("signature required"))?;
        verify_signature(public_key, signature, &tx)?;
    } else if tx.signature.is_some() {
        return Err(anyhow!("signature provided for account without public key"));
    }
    let entity = state
        .entities
        .get(&tx.entity_id)
        .ok_or_else(|| anyhow!("entity not found: {}", tx.entity_id))?;
    if entity.owner != tx.from {
        return Err(anyhow!("account is not the entity owner"));
    }

    let mut receipt = execute_with_limits(
        state,
        tx.entity_id,
        &tx.action,
        tx.args.clone(),
        true,
        limits,
    )?;
    let account = state
        .accounts
        .get_mut(&tx.from)
        .ok_or_else(|| anyhow!("account not found: {}", tx.from))?;
    account.nonce += 1;
    receipt.tx_id = blake3::hash(&serde_json::to_vec(&tx)?).to_hex().to_string();
    receipt.from = tx.from;
    receipt.nonce = tx.nonce;
    push_event(
        state,
        "transaction_executed",
        Some(tx.entity_id),
        serde_json::json!({"tx_id": receipt.tx_id.clone(), "from": receipt.from.clone(), "nonce": receipt.nonce}),
    );
    receipt.state_root = state.root();
    Ok(receipt)
}

fn push_event(state: &mut WorldState, kind: &str, entity_id: Option<EntityId>, data: Value) {
    let id = state.next_event_id;
    state.next_event_id += 1;
    state.events.push(WorldEvent {
        id,
        head: state.head,
        kind: kind.into(),
        entity_id,
        data,
    });
}

fn quest_progress_key(actor_id: EntityId, quest_id: &str) -> String {
    format!("{actor_id}:{quest_id}")
}

fn verify_signature(public_key: &str, encoded_signature: &str, tx: &Transaction) -> Result<()> {
    let public_key_bytes: [u8; 32] = hex::decode(public_key)
        .map_err(|_| anyhow!("public key must be 32-byte hex"))?
        .try_into()
        .map_err(|_| anyhow!("public key must be 32-byte hex"))?;
    let key =
        VerifyingKey::from_bytes(&public_key_bytes).map_err(|_| anyhow!("invalid public key"))?;
    let signature_bytes =
        hex::decode(encoded_signature).map_err(|_| anyhow!("signature must be 64-byte hex"))?;
    let signature = Signature::from_slice(&signature_bytes)
        .map_err(|_| anyhow!("signature must be 64-byte hex"))?;
    let payload = transaction_signing_bytes(tx)?;
    key.verify(&payload, &signature)
        .map_err(|_| anyhow!("invalid transaction signature"))?;
    Ok(())
}

fn transaction_signing_bytes(tx: &Transaction) -> Result<Vec<u8>> {
    let mut unsigned = tx.clone();
    unsigned.signature = None;
    Ok(serde_json::to_vec(&unsigned)?)
}

fn install(lua: &Lua, ctx: Context) -> mlua::Result<()> {
    let host = lua.create_table()?;
    let read_ctx = ctx.clone();
    host.set(
        "get_entity",
        lua.create_function(move |lua, id: u64| {
            let state = read_ctx.state.lock().unwrap();
            match state.entities.get(&id) {
                Some(entity) => json_to_lua(lua, &serde_json::to_value(entity).unwrap())
                    .map_err(mlua::Error::external),
                None => Ok(LuaValue::Nil),
            }
        })?,
    )?;
    let exit_ctx = ctx.clone();
    host.set(
        "get_exit",
        lua.create_function(move |_, (zone, direction): (u64, String)| {
            let state = exit_ctx.state.lock().unwrap();
            let target = state
                .entities
                .get(&zone)
                .and_then(|e| e.data.get("exits"))
                .and_then(|v| v.get(&direction))
                .and_then(Value::as_u64);
            Ok(target)
        })?,
    )?;
    let move_ctx = ctx.clone();
    host.set(
        "move_actor",
        lua.create_function(move |_, (actor, zone): (u64, u64)| {
            if !move_ctx.command {
                return Err(mlua::Error::external("query is read-only"));
            }
            let target = {
                let state = move_ctx.state.lock().unwrap();
                let target = state
                    .entities
                    .get(&zone)
                    .ok_or_else(|| mlua::Error::external("target zone not found"))?;
                if target.kind != EntityKind::Zone {
                    return Err(mlua::Error::external("target is not a zone"));
                }
                target.id
            };
            let mut state = move_ctx.state.lock().unwrap();
            let actor_ref = state
                .entities
                .get_mut(&actor)
                .ok_or_else(|| mlua::Error::external("actor not found"))?;
            if actor_ref.kind != EntityKind::Actor {
                return Err(mlua::Error::external("entity is not an actor"));
            }
            actor_ref.location = Some(target);
            Ok(true)
        })?,
    )?;
    let take_item_ctx = ctx.clone();
    host.set(
        "take_item",
        lua.create_function(move |_, item_id: u64| {
            if !take_item_ctx.command {
                return Err(mlua::Error::external("query is read-only"));
            }
            let mut state = take_item_ctx.state.lock().unwrap();
            let actor_location = state
                .entities
                .get(&take_item_ctx.entity_id)
                .ok_or_else(|| mlua::Error::external("caller entity not found"))?
                .location;
            let item = state
                .entities
                .get(&item_id)
                .ok_or_else(|| mlua::Error::external("item not found"))?;
            if item.kind != EntityKind::Item || item.location != actor_location {
                return Err(mlua::Error::external("item is not here"));
            }
            if state
                .inventories
                .values()
                .any(|items| items.contains(&item_id))
            {
                return Err(mlua::Error::external("item is already in an inventory"));
            }
            state.entities.get_mut(&item_id).unwrap().location = Some(take_item_ctx.entity_id);
            state
                .inventories
                .entry(take_item_ctx.entity_id)
                .or_default()
                .push(item_id);
            Ok(item_id)
        })?,
    )?;
    let drop_item_ctx = ctx.clone();
    host.set(
        "drop_item",
        lua.create_function(move |_, item_id: u64| {
            if !drop_item_ctx.command {
                return Err(mlua::Error::external("query is read-only"));
            }
            let mut state = drop_item_ctx.state.lock().unwrap();
            let actor_location = state
                .entities
                .get(&drop_item_ctx.entity_id)
                .ok_or_else(|| mlua::Error::external("caller entity not found"))?
                .location
                .ok_or_else(|| mlua::Error::external("actor has no location"))?;
            let inventory = state
                .inventories
                .get_mut(&drop_item_ctx.entity_id)
                .ok_or_else(|| mlua::Error::external("item is not in inventory"))?;
            let position = inventory
                .iter()
                .position(|id| *id == item_id)
                .ok_or_else(|| mlua::Error::external("item is not in inventory"))?;
            inventory.remove(position);
            state
                .entities
                .get_mut(&item_id)
                .ok_or_else(|| mlua::Error::external("item not found"))?
                .location = Some(actor_location);
            Ok(item_id)
        })?,
    )?;
    let transfer_item_ctx = ctx.clone();
    host.set(
        "transfer_item",
        lua.create_function(move |_, (item_id, target_id): (u64, u64)| {
            if !transfer_item_ctx.command {
                return Err(mlua::Error::external("query is read-only"));
            }
            if target_id == transfer_item_ctx.entity_id {
                return Err(mlua::Error::external("cannot transfer item to self"));
            }
            let mut state = transfer_item_ctx.state.lock().unwrap();
            let actor_location = state
                .entities
                .get(&transfer_item_ctx.entity_id)
                .ok_or_else(|| mlua::Error::external("caller entity not found"))?
                .location;
            let target = state
                .entities
                .get(&target_id)
                .ok_or_else(|| mlua::Error::external("target actor not found"))?;
            if target.kind != EntityKind::Actor {
                return Err(mlua::Error::external("transfer target is not an actor"));
            }
            if target.location != actor_location {
                return Err(mlua::Error::external("target actor is not here"));
            }
            let source_inventory = state
                .inventories
                .get(&transfer_item_ctx.entity_id)
                .ok_or_else(|| mlua::Error::external("item is not in inventory"))?;
            let position = source_inventory
                .iter()
                .position(|id| *id == item_id)
                .ok_or_else(|| mlua::Error::external("item is not in inventory"))?;
            let item = state
                .entities
                .get(&item_id)
                .ok_or_else(|| mlua::Error::external("item not found"))?;
            if item.kind != EntityKind::Item || item.location != Some(transfer_item_ctx.entity_id) {
                return Err(mlua::Error::external(
                    "inventory item location is inconsistent",
                ));
            }
            state
                .inventories
                .get_mut(&transfer_item_ctx.entity_id)
                .unwrap()
                .remove(position);
            state
                .inventories
                .entry(target_id)
                .or_default()
                .push(item_id);
            state.entities.get_mut(&item_id).unwrap().location = Some(target_id);
            Ok(item_id)
        })?,
    )?;
    let inventory_ctx = ctx.clone();
    host.set(
        "get_inventory",
        lua.create_function(move |lua, entity_id: u64| {
            let state = inventory_ctx.state.lock().unwrap();
            json_to_lua(
                lua,
                &serde_json::json!(state
                    .inventories
                    .get(&entity_id)
                    .cloned()
                    .unwrap_or_default()),
            )
            .map_err(mlua::Error::external)
        })?,
    )?;
    let move_entity_ctx = ctx.clone();
    host.set(
        "move_entity",
        lua.create_function(move |_, (entity_id, location): (u64, u64)| {
            if !move_entity_ctx.command {
                return Err(mlua::Error::external("query is read-only"));
            }
            let mut state = move_entity_ctx.state.lock().unwrap();
            let caller_owner = state
                .entities
                .get(&move_entity_ctx.entity_id)
                .ok_or_else(|| mlua::Error::external("caller entity not found"))?
                .owner
                .clone();
            let target = state
                .entities
                .get(&entity_id)
                .ok_or_else(|| mlua::Error::external("entity not found"))?;
            if target.owner != caller_owner {
                return Err(mlua::Error::external("caller is not the entity owner"));
            }
            if !state.entities.contains_key(&location) {
                return Err(mlua::Error::external("location not found"));
            }
            state.entities.get_mut(&entity_id).unwrap().location = Some(location);
            Ok(true)
        })?,
    )?;
    let spawn_ctx = ctx.clone();
    host.set(
        "spawn_entity",
        lua.create_function(
            move |_, (kind, location, data): (String, Option<u64>, LuaValue)| {
                if !spawn_ctx.command {
                    return Err(mlua::Error::external("query is read-only"));
                }
                let kind = match kind.as_str() {
                    "actor" => EntityKind::Actor,
                    "zone" => EntityKind::Zone,
                    "item" => EntityKind::Item,
                    _ => return Err(mlua::Error::external("unknown entity kind")),
                };
                let data = lua_to_json(data).map_err(mlua::Error::external)?;
                let data = match data {
                    Value::Object(data) => data.into_iter().collect(),
                    _ => return Err(mlua::Error::external("entity data must be an object")),
                };
                let mut state = spawn_ctx.state.lock().unwrap();
                let owner = state
                    .entities
                    .get(&spawn_ctx.entity_id)
                    .ok_or_else(|| mlua::Error::external("caller entity not found"))?
                    .owner
                    .clone();
                let id = create_entity(&mut state, kind, owner, None, location, data)
                    .map_err(mlua::Error::external)?;
                Ok(id)
            },
        )?,
    )?;
    let update_ctx = ctx.clone();
    host.set(
        "update_entity_data",
        lua.create_function(move |_, (id, key, value): (u64, String, LuaValue)| {
            if !update_ctx.command {
                return Err(mlua::Error::external("query is read-only"));
            }
            let value = lua_to_json(value).map_err(mlua::Error::external)?;
            let mut state = update_ctx.state.lock().unwrap();
            let caller_owner = state
                .entities
                .get(&update_ctx.entity_id)
                .ok_or_else(|| mlua::Error::external("caller entity not found"))?
                .owner
                .clone();
            let target_owner = state
                .entities
                .get(&id)
                .ok_or_else(|| mlua::Error::external("entity not found"))?
                .owner
                .clone();
            if caller_owner != target_owner {
                return Err(mlua::Error::external("caller is not the entity owner"));
            }
            let entity = state
                .entities
                .get_mut(&id)
                .ok_or_else(|| mlua::Error::external("entity not found"))?;
            entity.data.insert(key, value);
            Ok(true)
        })?,
    )?;
    let event_ctx = ctx.clone();
    host.set(
        "emit_event",
        lua.create_function(
            move |_, (kind, entity_id, data): (String, Option<u64>, LuaValue)| {
                if !event_ctx.command {
                    return Err(mlua::Error::external("query is read-only"));
                }
                if kind.is_empty() {
                    return Err(mlua::Error::external("event kind must not be empty"));
                }
                if matches!(
                    kind.as_str(),
                    "entity_changed"
                        | "quest_progress_changed"
                        | "command_executed"
                        | "transaction_executed"
                ) {
                    return Err(mlua::Error::external("event kind is reserved"));
                }
                let data = lua_to_json(data).map_err(mlua::Error::external)?;
                let mut state = event_ctx.state.lock().unwrap();
                if let Some(id) = entity_id {
                    if !state.entities.contains_key(&id) {
                        return Err(mlua::Error::external("event entity not found"));
                    }
                }
                let id = state.next_event_id;
                state.next_event_id += 1;
                let head = state.head;
                state.events.push(WorldEvent {
                    id,
                    head,
                    kind,
                    entity_id,
                    data,
                });
                Ok(id)
            },
        )?,
    )?;
    let quest_status_ctx = ctx.clone();
    host.set(
        "quest_status",
        lua.create_function(move |_, quest_id: String| {
            let state = quest_status_ctx.state.lock().unwrap();
            let key = quest_progress_key(quest_status_ctx.entity_id, &quest_id);
            Ok(
                match state
                    .quest_progress
                    .get(&key)
                    .map(|progress| &progress.status)
                {
                    Some(QuestStatus::Accepted) => "accepted",
                    Some(QuestStatus::Completed) => "completed",
                    None => "available",
                },
            )
        })?,
    )?;
    let accept_quest_ctx = ctx.clone();
    host.set(
        "accept_quest",
        lua.create_function(move |_, (quest_id, giver_entity_id): (String, EntityId)| {
            if !accept_quest_ctx.command {
                return Err(mlua::Error::external("query is read-only"));
            }
            let mut state = accept_quest_ctx.state.lock().unwrap();
            let quest = state
                .quests
                .get(&quest_id)
                .ok_or_else(|| mlua::Error::external("quest not found"))?;
            if quest.giver_entity_id != giver_entity_id {
                return Err(mlua::Error::external("quest giver does not match"));
            }
            let missing_prerequisite = quest.prerequisite_quest_ids.iter().find(|prerequisite| {
                state
                    .quest_progress
                    .get(&quest_progress_key(
                        accept_quest_ctx.entity_id,
                        prerequisite,
                    ))
                    .map(|progress| progress.status != QuestStatus::Completed)
                    .unwrap_or(true)
            });
            if let Some(prerequisite) = missing_prerequisite {
                return Err(mlua::Error::external(format!(
                    "quest prerequisite is not completed: {prerequisite}"
                )));
            }
            let actor_location = state
                .entities
                .get(&accept_quest_ctx.entity_id)
                .and_then(|actor| actor.location)
                .ok_or_else(|| mlua::Error::external("caller entity has no location"))?;
            let giver = state
                .entities
                .get(&giver_entity_id)
                .ok_or_else(|| mlua::Error::external("quest giver not found"))?;
            if giver.kind != EntityKind::Actor || giver.location != Some(actor_location) {
                return Err(mlua::Error::external("quest giver is not here"));
            }
            let key = quest_progress_key(accept_quest_ctx.entity_id, &quest_id);
            if state.quest_progress.contains_key(&key) {
                return Err(mlua::Error::external("quest already accepted"));
            }
            state.quest_progress.insert(
                key,
                QuestProgress {
                    quest_id,
                    actor_id: accept_quest_ctx.entity_id,
                    status: QuestStatus::Accepted,
                },
            );
            Ok("accepted")
        })?,
    )?;
    let complete_quest_ctx = ctx.clone();
    host.set(
        "complete_quest",
        lua.create_function(move |_, quest_id: String| {
            if !complete_quest_ctx.command {
                return Err(mlua::Error::external("query is read-only"));
            }
            let mut state = complete_quest_ctx.state.lock().unwrap();
            let quest = state
                .quests
                .get(&quest_id)
                .cloned()
                .ok_or_else(|| mlua::Error::external("quest not found"))?;
            let key = quest_progress_key(complete_quest_ctx.entity_id, &quest_id);
            match state
                .quest_progress
                .get(&key)
                .map(|progress| &progress.status)
            {
                Some(QuestStatus::Accepted) => {}
                Some(QuestStatus::Completed) => {
                    return Err(mlua::Error::external("quest already completed"));
                }
                None => return Err(mlua::Error::external("quest is not accepted")),
            }
            let actor = state
                .entities
                .get(&complete_quest_ctx.entity_id)
                .cloned()
                .ok_or_else(|| mlua::Error::external("caller entity not found"))?;
            let zone_name = actor
                .location
                .and_then(|id| state.entities.get(&id))
                .and_then(|zone| zone.data.get("name"))
                .and_then(Value::as_str);
            if zone_name != Some(quest.completion_zone.as_str()) {
                return Err(mlua::Error::external(format!(
                    "return to {} to complete the quest",
                    quest.completion_zone
                )));
            }
            let required_item_id = state
                .inventories
                .get(&complete_quest_ctx.entity_id)
                .into_iter()
                .flatten()
                .find(|id| {
                    state
                        .entities
                        .get(id)
                        .and_then(|item| item.data.get("name"))
                        .and_then(Value::as_str)
                        == Some(quest.required_item.as_str())
                })
                .copied();
            if required_item_id.is_none() {
                return Err(mlua::Error::external(format!(
                    "the {} is required",
                    quest.required_item
                )));
            }
            let account = state
                .accounts
                .get_mut(&actor.owner)
                .ok_or_else(|| mlua::Error::external("owner account not found"))?;
            account.balance = account
                .balance
                .checked_add(quest.reward_balance)
                .ok_or_else(|| mlua::Error::external("balance overflow"))?;
            if quest.consume_required_item {
                let item_id = required_item_id.unwrap();
                state
                    .inventories
                    .get_mut(&complete_quest_ctx.entity_id)
                    .unwrap()
                    .retain(|id| *id != item_id);
                state.entities.remove(&item_id);
            }
            state.quest_progress.get_mut(&key).unwrap().status = QuestStatus::Completed;
            Ok(quest.reward_balance)
        })?,
    )?;
    let balance_ctx = ctx.clone();
    host.set(
        "get_balance",
        lua.create_function(move |_, _entity_id: u64| {
            let state = balance_ctx.state.lock().unwrap();
            let owner = state
                .entities
                .get(&balance_ctx.entity_id)
                .ok_or_else(|| mlua::Error::external("caller entity not found"))?
                .owner
                .clone();
            Ok(state
                .accounts
                .get(&owner)
                .map(|account| account.balance)
                .unwrap_or(0))
        })?,
    )?;
    let credit_ctx = ctx.clone();
    host.set(
        "credit_balance",
        lua.create_function(move |_, amount: u64| {
            if !credit_ctx.command {
                return Err(mlua::Error::external("query is read-only"));
            }
            let mut state = credit_ctx.state.lock().unwrap();
            let owner = state
                .entities
                .get(&credit_ctx.entity_id)
                .ok_or_else(|| mlua::Error::external("caller entity not found"))?
                .owner
                .clone();
            let account = state
                .accounts
                .get_mut(&owner)
                .ok_or_else(|| mlua::Error::external("owner account not found"))?;
            account.balance = account
                .balance
                .checked_add(amount)
                .ok_or_else(|| mlua::Error::external("balance overflow"))?;
            Ok(account.balance)
        })?,
    )?;
    let msg_ctx = ctx.messages.clone();
    host.set(
        "narrate",
        lua.create_function(move |_, message: String| {
            msg_ctx.lock().unwrap().push(message);
            Ok(())
        })?,
    )?;
    lua.globals().set("host", host)
}

fn json_to_lua(lua: &Lua, value: &Value) -> mlua::Result<LuaValue> {
    Ok(match value {
        Value::Null => LuaValue::Nil,
        Value::Bool(v) => LuaValue::Boolean(*v),
        Value::Number(v) => match (v.as_i64(), v.as_f64()) {
            (Some(integer), _) => LuaValue::Integer(integer),
            (_, Some(number)) => LuaValue::Number(number),
            _ => LuaValue::Nil,
        },
        Value::String(v) => LuaValue::String(lua.create_string(v)?),
        Value::Array(v) => {
            let t = lua.create_table()?;
            for (i, x) in v.iter().enumerate() {
                t.set(i + 1, json_to_lua(lua, x)?)?;
            }
            LuaValue::Table(t)
        }
        Value::Object(v) => {
            let t = lua.create_table()?;
            for (k, x) in v {
                t.set(k.as_str(), json_to_lua(lua, x)?)?;
            }
            LuaValue::Table(t)
        }
    })
}
fn lua_to_json(value: LuaValue) -> Result<Value> {
    Ok(match value {
        LuaValue::Nil => Value::Null,
        LuaValue::Boolean(v) => Value::Bool(v),
        LuaValue::Integer(v) => Value::Number(v.into()),
        LuaValue::Number(v) => serde_json::Number::from_f64(v)
            .map(Value::Number)
            .unwrap_or(Value::Null),
        LuaValue::String(v) => Value::String(
            v.to_str()
                .map_err(|e| anyhow!("lua string: {e}"))?
                .to_string(),
        ),
        LuaValue::Table(t) => {
            let pairs: Vec<(LuaValue, LuaValue)> = t
                .pairs::<LuaValue, LuaValue>()
                .collect::<mlua::Result<Vec<_>>>()
                .map_err(|error| anyhow!("lua table: {error}"))?;
            let is_array = !pairs.is_empty()
                && pairs
                    .iter()
                    .all(|(key, _)| matches!(key, LuaValue::Integer(value) if *value > 0))
                && pairs
                    .iter()
                    .map(|(key, _)| match key {
                        LuaValue::Integer(value) => *value,
                        _ => 0,
                    })
                    .max()
                    == Some(pairs.len() as i64);
            if is_array {
                let mut values = vec![Value::Null; pairs.len()];
                for (key, value) in pairs {
                    if let LuaValue::Integer(index) = key {
                        values[(index - 1) as usize] = lua_to_json(value)?;
                    }
                }
                Value::Array(values)
            } else {
                let mut map = serde_json::Map::new();
                for (key, value) in pairs {
                    let key = match key {
                        LuaValue::String(key) => key
                            .to_str()
                            .map_err(|e| anyhow!("lua key: {e}"))?
                            .to_string(),
                        LuaValue::Integer(key) => key.to_string(),
                        _ => return Err(anyhow!("lua table key must be string or integer")),
                    };
                    map.insert(key, lua_to_json(value)?);
                }
                Value::Object(map)
            }
        }
        _ => Value::Null,
    })
}
