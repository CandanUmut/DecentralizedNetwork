//! The TimeCoin DAG ("tangle"): contribution records attach to current tips,
//! are validated, and balances are derived by folding over the whole DAG.
//!
//! Ported from `legacy/timecoin/` with the design kept and the bugs fixed:
//! deterministic hashing (sorted parents, canonical byte encoding), Ed25519
//! signatures with the node's persistent key instead of throwaway RSA keys,
//! and balances computed from the DAG instead of mutating address strings.

use anyhow::{anyhow, bail, Context, Result};
use libp2p::identity::{Keypair, PublicKey};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, HashMap, HashSet};
use std::io::Write;
use std::path::{Path, PathBuf};

/// Derive a wallet address from a peer id, exactly as the legacy code did:
/// `0x` + hex(sha256(base58 peer id)).
pub fn wallet_address(peer_id: &libp2p::PeerId) -> String {
    format!("0x{}", hex::encode(Sha256::digest(peer_id.to_base58().as_bytes())))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TxKind {
    /// Mints `amount` to `receiver` for a contribution. `sender` is the
    /// wallet of the node that recorded the contribution.
    Reward,
    /// Moves `amount` from `sender` to `receiver`.
    Transfer,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Transaction {
    pub id: String,
    pub kind: TxKind,
    pub sender: String,
    pub receiver: String,
    pub amount: u64,
    /// Free-text description of the contribution (for rewards).
    pub memo: String,
    /// Parent transaction ids, sorted — attachment points in the DAG.
    pub parents: Vec<String>,
    pub timestamp: u64,
    /// Protobuf-encoded libp2p public key of the signer. Empty for genesis.
    #[serde(with = "hex_bytes")]
    pub public_key: Vec<u8>,
    #[serde(with = "hex_bytes")]
    pub signature: Vec<u8>,
}

mod hex_bytes {
    use serde::{Deserialize, Deserializer, Serializer};
    pub fn serialize<S: Serializer>(v: &[u8], s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(&hex::encode(v))
    }
    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<Vec<u8>, D::Error> {
        let s = String::deserialize(d)?;
        hex::decode(&s).map_err(serde::de::Error::custom)
    }
}

impl Transaction {
    /// Canonical bytes covered by the hash and the signature. Field order,
    /// length prefixes, and sorted parents make this deterministic across
    /// nodes — the legacy code hashed `format!("{:?}", HashSet)`, whose
    /// iteration order is random per process.
    #[allow(clippy::too_many_arguments)] // one arg per hashed field, by design
    fn signing_bytes(
        kind: TxKind,
        sender: &str,
        receiver: &str,
        amount: u64,
        memo: &str,
        parents: &[String],
        timestamp: u64,
        public_key: &[u8],
    ) -> Vec<u8> {
        let mut out = Vec::new();
        let mut put = |bytes: &[u8]| {
            out.extend_from_slice(&(bytes.len() as u64).to_be_bytes());
            out.extend_from_slice(bytes);
        };
        put(b"timecoin-tx-v1");
        put(match kind {
            TxKind::Reward => b"reward",
            TxKind::Transfer => b"transfer",
        });
        put(sender.as_bytes());
        put(receiver.as_bytes());
        put(&amount.to_be_bytes());
        put(memo.as_bytes());
        for p in parents {
            put(p.as_bytes());
        }
        put(&timestamp.to_be_bytes());
        put(public_key);
        out
    }

    #[allow(clippy::too_many_arguments)]
    pub fn create(
        kind: TxKind,
        keypair: &Keypair,
        sender: String,
        receiver: String,
        amount: u64,
        memo: String,
        mut parents: Vec<String>,
        timestamp: u64,
    ) -> Result<Self> {
        parents.sort();
        parents.dedup();
        let public_key = keypair.public().encode_protobuf();
        let bytes = Self::signing_bytes(
            kind, &sender, &receiver, amount, &memo, &parents, timestamp, &public_key,
        );
        let id = hex::encode(Sha256::digest(&bytes));
        let signature = keypair.sign(&bytes).context("signing transaction")?;
        Ok(Self {
            id,
            kind,
            sender,
            receiver,
            amount,
            memo,
            parents,
            timestamp,
            public_key,
            signature,
        })
    }

    /// Structural validation: id matches content, signature verifies, and for
    /// transfers the sender wallet must belong to the signing key. This is
    /// deterministic — every node reaches the same verdict — which is what
    /// makes the DAG converge.
    pub fn validate(&self) -> Result<()> {
        if self.amount == 0 {
            bail!("amount must be positive");
        }
        let mut sorted = self.parents.clone();
        sorted.sort();
        if sorted != self.parents {
            bail!("parents not in canonical (sorted) order");
        }
        let bytes = Self::signing_bytes(
            self.kind,
            &self.sender,
            &self.receiver,
            self.amount,
            &self.memo,
            &self.parents,
            self.timestamp,
            &self.public_key,
        );
        if hex::encode(Sha256::digest(&bytes)) != self.id {
            bail!("transaction id does not match contents");
        }
        let key = PublicKey::try_decode_protobuf(&self.public_key)
            .map_err(|e| anyhow!("bad public key: {e}"))?;
        if !key.verify(&bytes, &self.signature) {
            bail!("signature verification failed");
        }
        if self.kind == TxKind::Transfer && wallet_address(&key.to_peer_id()) != self.sender {
            bail!("transfer not signed by the sender's key");
        }
        Ok(())
    }

    pub fn is_genesis(&self) -> bool {
        self.parents.is_empty() && self.public_key.is_empty()
    }
}

/// The two genesis transactions every node computes identically, so all DAGs
/// share the same roots (the legacy bootstrap generated them with random RSA
/// keys, so no two nodes ever agreed on genesis).
pub fn genesis() -> Vec<Transaction> {
    let g = |receiver: &str, parents: Vec<String>| {
        let bytes = Transaction::signing_bytes(
            TxKind::Reward,
            "System",
            receiver,
            1,
            "genesis",
            &parents,
            0,
            &[],
        );
        Transaction {
            id: hex::encode(Sha256::digest(&bytes)),
            kind: TxKind::Reward,
            sender: "System".into(),
            receiver: receiver.into(),
            amount: 1,
            memo: "genesis".into(),
            parents,
            timestamp: 0,
            public_key: vec![],
            signature: vec![],
        }
    };
    let g1 = g("SystemReceiver1", vec![]);
    let mut g2 = g("SystemReceiver2", vec![]);
    // Second genesis references the first, as in the legacy bootstrap.
    g2.parents = vec![g1.id.clone()];
    let bytes = Transaction::signing_bytes(
        TxKind::Reward,
        "System",
        "SystemReceiver2",
        1,
        "genesis",
        &g2.parents,
        0,
        &[],
    );
    g2.id = hex::encode(Sha256::digest(&bytes));
    vec![g1, g2]
}

/// In-memory DAG with append-only on-disk persistence (JSON lines).
pub struct Dag {
    transactions: HashMap<String, Transaction>,
    /// Ids referenced as a parent by at least one accepted transaction.
    referenced: HashSet<String>,
    /// Valid transactions waiting for missing parents, by missing parent id.
    orphans: HashMap<String, Vec<Transaction>>,
    log_path: Option<PathBuf>,
}

impl Dag {
    pub fn new() -> Self {
        let mut dag = Self {
            transactions: HashMap::new(),
            referenced: HashSet::new(),
            orphans: HashMap::new(),
            log_path: None,
        };
        for g in genesis() {
            dag.accept(g);
        }
        dag
    }

    /// Load from `dir/dag.jsonl`, creating a fresh DAG (genesis only) if the
    /// file doesn't exist. Every stored transaction is re-validated on load.
    pub fn load(dir: &Path) -> Result<Self> {
        let mut dag = Self::new();
        let path = dir.join("dag.jsonl");
        if path.exists() {
            let data = std::fs::read_to_string(&path)
                .with_context(|| format!("reading {}", path.display()))?;
            for line in data.lines().filter(|l| !l.trim().is_empty()) {
                let tx: Transaction = serde_json::from_str(line).context("corrupt dag.jsonl line")?;
                // Genesis rows are already present; skip duplicates silently.
                if let Err(e) = dag.insert(tx.clone()) {
                    if !dag.contains(&tx.id) {
                        tracing::warn!("dropping invalid stored transaction {}: {e}", tx.id);
                    }
                }
            }
        }
        dag.log_path = Some(path);
        Ok(dag)
    }

    pub fn contains(&self, id: &str) -> bool {
        self.transactions.contains_key(id)
    }

    pub fn len(&self) -> usize {
        self.transactions.len()
    }

    pub fn get(&self, id: &str) -> Option<&Transaction> {
        self.transactions.get(id)
    }

    pub fn all(&self) -> impl Iterator<Item = &Transaction> {
        self.transactions.values()
    }

    pub fn all_ids(&self) -> Vec<String> {
        self.transactions.keys().cloned().collect()
    }

    /// Current tips: accepted transactions no one references yet.
    pub fn tips(&self) -> Vec<String> {
        let mut tips: Vec<String> = self
            .transactions
            .keys()
            .filter(|id| !self.referenced.contains(*id))
            .cloned()
            .collect();
        tips.sort();
        tips
    }

    /// Missing parent ids currently blocking orphans — what to ask peers for.
    pub fn missing_parents(&self) -> Vec<String> {
        self.orphans.keys().cloned().collect()
    }

    fn accept(&mut self, tx: Transaction) {
        for p in &tx.parents {
            self.referenced.insert(p.clone());
        }
        self.transactions.insert(tx.id.clone(), tx.clone());
        if let Some(path) = &self.log_path {
            if let Err(e) = append_line(path, &tx) {
                tracing::error!("failed to persist transaction {}: {e}", tx.id);
            }
        }
    }

    /// Insert a transaction (local or from the network).
    ///
    /// Returns `Ok(newly_accepted)`: the transaction itself plus any orphans
    /// it unblocked. A structurally valid transaction with unknown parents is
    /// parked as an orphan and returns an empty vec.
    pub fn insert(&mut self, tx: Transaction) -> Result<Vec<Transaction>> {
        if self.contains(&tx.id) {
            return Ok(vec![]);
        }
        if !tx.is_genesis() {
            tx.validate()?;
        } else if !genesis().iter().any(|g| g.id == tx.id) {
            bail!("unknown genesis transaction");
        }

        if let Some(missing) = tx.parents.iter().find(|p| !self.contains(p)) {
            self.orphans.entry(missing.clone()).or_default().push(tx);
            return Ok(vec![]);
        }

        let mut accepted = vec![tx.clone()];
        self.accept(tx);

        // Unblock orphans transitively.
        let mut queue: Vec<String> = vec![accepted[0].id.clone()];
        while let Some(id) = queue.pop() {
            let Some(waiting) = self.orphans.remove(&id) else { continue };
            for w in waiting {
                if self.contains(&w.id) {
                    continue;
                }
                if let Some(missing) = w.parents.iter().find(|p| !self.contains(p)) {
                    let missing = missing.clone();
                    self.orphans.entry(missing).or_default().push(w);
                } else {
                    queue.push(w.id.clone());
                    accepted.push(w.clone());
                    self.accept(w);
                }
            }
        }
        Ok(accepted)
    }

    /// Balances derived by folding over the DAG: rewards mint, transfers move.
    /// A BTreeMap so output ordering is stable everywhere.
    pub fn balances(&self) -> BTreeMap<String, i64> {
        let mut balances: BTreeMap<String, i64> = BTreeMap::new();
        for tx in self.transactions.values() {
            match tx.kind {
                TxKind::Reward => {
                    *balances.entry(tx.receiver.clone()).or_default() += tx.amount as i64;
                }
                TxKind::Transfer => {
                    *balances.entry(tx.sender.clone()).or_default() -= tx.amount as i64;
                    *balances.entry(tx.receiver.clone()).or_default() += tx.amount as i64;
                }
            }
        }
        balances
    }

    pub fn balance(&self, wallet: &str) -> i64 {
        self.balances().get(wallet).copied().unwrap_or(0)
    }
}

fn append_line(path: &Path, tx: &Transaction) -> Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let mut file = std::fs::OpenOptions::new().create(true).append(true).open(path)?;
    serde_json::to_writer(&mut file, tx)?;
    file.write_all(b"\n")?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn keypair() -> Keypair {
        Keypair::generate_ed25519()
    }

    fn reward_to(dag: &Dag, key: &Keypair, wallet: &str, amount: u64) -> Transaction {
        Transaction::create(
            TxKind::Reward,
            key,
            wallet_address(&key.public().to_peer_id()),
            wallet.to_string(),
            amount,
            "test contribution".into(),
            dag.tips(),
            1,
        )
        .unwrap()
    }

    #[test]
    fn genesis_is_deterministic() {
        assert_eq!(
            genesis().iter().map(|g| g.id.clone()).collect::<Vec<_>>(),
            genesis().iter().map(|g| g.id.clone()).collect::<Vec<_>>()
        );
    }

    #[test]
    fn reward_then_transfer_updates_balances() {
        let key = keypair();
        let wallet = wallet_address(&key.public().to_peer_id());
        let mut dag = Dag::new();

        let r = reward_to(&dag, &key, &wallet, 100);
        dag.insert(r).unwrap();
        assert_eq!(dag.balance(&wallet), 100);

        let t = Transaction::create(
            TxKind::Transfer,
            &key,
            wallet.clone(),
            "0xfriend".into(),
            40,
            String::new(),
            dag.tips(),
            2,
        )
        .unwrap();
        dag.insert(t).unwrap();
        assert_eq!(dag.balance(&wallet), 60);
        assert_eq!(dag.balance("0xfriend"), 40);
    }

    #[test]
    fn transfer_must_be_signed_by_sender() {
        let key = keypair();
        let dag = Dag::new();
        let t = Transaction::create(
            TxKind::Transfer,
            &key,
            "0xsomeoneelse".into(),
            "0xfriend".into(),
            5,
            String::new(),
            dag.tips(),
            2,
        )
        .unwrap();
        assert!(t.validate().is_err());
    }

    #[test]
    fn tampered_amount_fails_validation() {
        let key = keypair();
        let dag = Dag::new();
        let mut r = reward_to(&dag, &key, "0xw", 10);
        r.amount = 1000;
        assert!(r.validate().is_err());
    }

    #[test]
    fn orphans_resolve_when_parent_arrives() {
        let key = keypair();
        let mut a = Dag::new();
        let r1 = reward_to(&a, &key, "0xw", 10);
        a.insert(r1.clone()).unwrap();
        let r2 = reward_to(&a, &key, "0xw", 20); // parents include r1
        a.insert(r2.clone()).unwrap();

        // Node B receives child before parent.
        let mut b = Dag::new();
        assert!(b.insert(r2.clone()).unwrap().is_empty());
        assert!(!b.contains(&r2.id));
        assert_eq!(b.missing_parents(), vec![r1.id.clone()]);
        let accepted = b.insert(r1.clone()).unwrap();
        assert_eq!(accepted.len(), 2);
        assert!(b.contains(&r2.id));
        assert_eq!(b.tips(), a.tips());
        assert_eq!(b.balances(), a.balances());
    }

    #[test]
    fn two_dags_converge_regardless_of_order() {
        let k1 = keypair();
        let k2 = keypair();
        let mut a = Dag::new();
        let tx1 = reward_to(&a, &k1, "0xalice", 50);
        a.insert(tx1.clone()).unwrap();
        let tx2 = reward_to(&a, &k2, "0xbob", 30);
        a.insert(tx2.clone()).unwrap();

        let mut b = Dag::new();
        b.insert(tx2.clone()).unwrap();
        b.insert(tx1.clone()).unwrap();

        assert_eq!(a.balances(), b.balances());
        assert_eq!(a.tips(), b.tips());
        assert_eq!(a.len(), b.len());
    }

    #[test]
    fn persistence_roundtrip() {
        let dir = std::env::temp_dir().join(format!("timecoin-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let key = keypair();
        {
            let mut dag = Dag::load(&dir).unwrap();
            let r = reward_to(&dag, &key, "0xw", 10);
            dag.insert(r).unwrap();
        }
        let dag = Dag::load(&dir).unwrap();
        assert_eq!(dag.balance("0xw"), 10);
        assert_eq!(dag.len(), 3); // 2 genesis + 1 reward
        let _ = std::fs::remove_dir_all(&dir);
    }
}
