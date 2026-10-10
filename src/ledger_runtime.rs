//! Experimental in-memory ledger. No RPC, disk commit or network consensus.
use crate::ledger::*;
use crate::model::{WorldEvent, WorldState};
use crate::runtime::{execute_ledger_command, LuaLimits};
use anyhow::{ensure, Result};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ExecutionLimits {
    pub instruction_limit: u64,
    pub memory_limit: usize,
}

pub struct MemoryLedger {
    world: WorldState,
    genesis: GenesisManifest,
    genesis_hash: String,
    tip: BlockHeader,
    blocks: Vec<LedgerBlock>,
    limits: LuaLimits,
}
fn hash(domain: Domain, value: &Value) -> Result<String> {
    Ok(hex::encode(digest(domain, value)?))
}
fn state_digest(world: &WorldState) -> Result<String> {
    let mut projection = world.clone();
    projection.receipts.clear();
    hash(Domain::State, &serde_json::to_value(projection)?)
}
impl MemoryLedger {
    pub fn new(genesis: GenesisManifest) -> Result<Self> {
        let genesis_hash = genesis.hash()?;
        let world: WorldState = serde_json::from_value(genesis.initial_world.clone())?;
        world.validate()?;
        ensure!(
            world.receipts.is_empty() && world.events.is_empty() && world.next_event_id == 1,
            "genesis history must be empty"
        );
        for account in world.accounts.values() {
            let public_key = account
                .public_key
                .as_ref()
                .ok_or_else(|| anyhow::anyhow!("genesis account requires public key"))?;
            let bytes: [u8; 32] = hex::decode(public_key)?
                .try_into()
                .map_err(|_| anyhow::anyhow!("invalid genesis key"))?;
            ensure!(
                hex::encode(bytes) == *public_key,
                "noncanonical genesis key"
            );
            ed25519_dalek::VerifyingKey::from_bytes(&bytes)?;
            ensure!(account.nonce == 0, "genesis nonce must be zero");
        }
        let config: ExecutionLimits = serde_json::from_value(genesis.execution_limits.clone())?;
        ensure!(
            (1_000..=10_000_000).contains(&config.instruction_limit),
            "invalid instruction limit"
        );
        ensure!(
            (1024 * 1024..=64 * 1024 * 1024).contains(&config.memory_limit),
            "invalid memory limit"
        );
        let tip = BlockHeader {
            protocol_version: 1,
            chain_id: genesis.chain_id.clone(),
            genesis_hash: genesis_hash.clone(),
            runtime_rules_version: 1,
            height: 0,
            parent_hash: "0".repeat(64),
            tx_count: 0,
            transactions_digest: hash(Domain::Transactions, &json!([]))?,
            results_digest: hash(Domain::Results, &json!([]))?,
            post_state_digest: state_digest(&world)?,
        };
        let initial = LedgerBlock {
            header: tip.clone(),
            transactions: vec![],
            execution_results: vec![],
        };
        initial.validate_commitments()?;
        Ok(Self {
            world,
            genesis,
            genesis_hash,
            tip,
            blocks: vec![initial],
            limits: LuaLimits {
                instruction_limit: config.instruction_limit,
                memory_limit: config.memory_limit,
            },
        })
    }
    pub fn world(&self) -> &WorldState {
        &self.world
    }
    pub fn blocks(&self) -> &[LedgerBlock] {
        &self.blocks
    }
    pub fn genesis_hash(&self) -> &str {
        &self.genesis_hash
    }
    pub fn genesis(&self) -> &GenesisManifest {
        &self.genesis
    }
    pub fn tip_hash(&self) -> Result<String> {
        self.tip.hash()
    }

    /// Produces a candidate without mutating published state.
    pub fn propose(&self, tx: LedgerTransaction) -> Result<LedgerBlock> {
        let (_, block) = self.execute_candidate(tx)?;
        Ok(block)
    }
    fn execute_candidate(&self, tx: LedgerTransaction) -> Result<(WorldState, LedgerBlock)> {
        let account = self
            .world
            .accounts
            .get(&tx.from)
            .ok_or_else(|| anyhow::anyhow!("account not found"))?;
        tx.verify(
            account.public_key.as_deref().unwrap(),
            &self.genesis.chain_id,
            &self.genesis_hash,
        )?;
        ensure!(tx.nonce == account.nonce, "invalid nonce");
        let entity = self
            .world
            .entities
            .get(&tx.entity_id)
            .ok_or_else(|| anyhow::anyhow!("entity not found"))?;
        ensure!(entity.owner == tx.from, "not entity owner");
        ensure!(
            self.world.head < MAX_INTEGER as u64 && self.world.next_event_id < MAX_INTEGER as u64,
            "execution counter exhausted"
        );
        let tx_id = tx.tx_id()?;
        let mut candidate = self.world.clone();
        let event_start = candidate.events.len();
        let receipt = execute_ledger_command(
            &mut candidate,
            tx.entity_id,
            &tx.action,
            tx.args.clone(),
            self.limits,
        )?;
        ensure!(receipt.ok, "execution failed");
        candidate.accounts.get_mut(&tx.from).unwrap().nonce = account
            .nonce
            .checked_add(1)
            .ok_or_else(|| anyhow::anyhow!("nonce overflow"))?;
        let id = candidate.next_event_id;
        candidate.next_event_id = id
            .checked_add(1)
            .ok_or_else(|| anyhow::anyhow!("event overflow"))?;
        // Distinct kind: legacy transaction_executed requires a legacy receipt.
        candidate.events.push(WorldEvent {
            id,
            head: candidate.head,
            kind: "ledger_transaction_executed".into(),
            entity_id: Some(tx.entity_id),
            data: json!({"tx_id":tx_id,"from":tx.from,"nonce":tx.nonce}),
        });
        candidate.validate()?;
        let events = &candidate.events[event_start..];
        let result = ExecutionResult {
            tx_id: tx_id.clone(),
            from: tx.from.clone(),
            nonce: tx.nonce,
            ok: true,
            messages: receipt.messages,
            result: receipt.result,
            first_event_id: events.first().map(|e| e.id),
            last_event_id: events.last().map(|e| e.id),
            events_digest: hash(Domain::Events, &serde_json::to_value(events)?)?,
        };
        let results = vec![result];
        let header = BlockHeader {
            protocol_version: 1,
            chain_id: self.genesis.chain_id.clone(),
            genesis_hash: self.genesis_hash.clone(),
            runtime_rules_version: 1,
            height: self
                .tip
                .height
                .checked_add(1)
                .ok_or_else(|| anyhow::anyhow!("height overflow"))?,
            parent_hash: self.tip.hash()?,
            tx_count: 1,
            transactions_digest: hash(Domain::Transactions, &json!([tx_id]))?,
            results_digest: hash(Domain::Results, &serde_json::to_value(&results)?)?,
            post_state_digest: state_digest(&candidate)?,
        };
        let block = LedgerBlock {
            header,
            transactions: vec![tx],
            execution_results: results,
        };
        block.validate_commitments()?;
        Ok((candidate, block))
    }
    /// Reexecutes a supplied block; publishes only after all commitments match.
    pub fn apply(&mut self, block: LedgerBlock) -> Result<()> {
        block.validate_commitments()?;
        ensure!(
            Some(block.header.height) == self.tip.height.checked_add(1)
                && block.header.parent_hash == self.tip.hash()?,
            "invalid ancestry"
        );
        let (candidate, expected) = self.execute_candidate(block.transactions[0].clone())?;
        ensure!(
            canonical_json(&serde_json::to_value(&expected)?)?
                == canonical_json(&serde_json::to_value(&block)?)?,
            "replayed block mismatch"
        );
        self.world = candidate;
        self.tip = block.header.clone();
        self.blocks.push(block);
        Ok(())
    }
    /// Exact duplicate success returns the existing block without reexecution.
    pub fn submit(&mut self, tx: LedgerTransaction) -> Result<LedgerBlock> {
        let tx_id = tx.tx_id()?;
        if let Some(block) = self
            .blocks
            .iter()
            .skip(1)
            .find(|b| b.execution_results[0].tx_id == tx_id)
        {
            return Ok(block.clone());
        }
        let block = self.propose(tx)?;
        self.apply(block.clone())?;
        Ok(block)
    }
}
