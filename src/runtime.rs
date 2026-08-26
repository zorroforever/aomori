use crate::model::*;
use anyhow::{anyhow, Result};
use mlua::{Lua, Value as LuaValue};
use serde_json::Value;
use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

#[derive(Debug, Clone, serde::Serialize)]
pub struct Receipt {
    pub ok: bool,
    pub messages: Vec<String>,
    pub result: Value,
    pub state_root: String,
}

#[derive(Clone)]
struct Context {
    state: Arc<Mutex<WorldState>>,
    entity_id: EntityId,
    command: bool,
    messages: Arc<Mutex<Vec<String>>>,
}

pub fn deploy(state: &mut WorldState, name: String, source: String) -> Result<()> {
    if state.contracts.contains_key(&name) {
        return Err(anyhow!("contract already exists"));
    }
    let hash = blake3::hash(source.as_bytes()).to_hex().to_string();
    state.contracts.insert(
        name.clone(),
        Contract {
            name,
            source,
            source_hash: hash,
        },
    );
    state.head += 1;
    Ok(())
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
    } else {
        *state = original;
    }
    let narration = messages.lock().unwrap().clone();
    let result = lua_to_json(result).map_err(|e| anyhow!("lua result: {e}"))?;
    Ok(Receipt {
        ok: true,
        messages: narration,
        result,
        state_root: state.root(),
    })
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
            let mut map = serde_json::Map::new();
            for pair in t.pairs::<String, LuaValue>() {
                let (k, v) = pair.map_err(|e| anyhow!("lua table: {e}"))?;
                map.insert(k, lua_to_json(v)?);
            }
            Value::Object(map)
        }
        _ => Value::Null,
    })
}
