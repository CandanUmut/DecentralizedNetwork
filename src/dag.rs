//! The TimeCoin DAG ("tangle"): contribution records attach to current tips,
//! are validated, and balances are derived by folding over the whole DAG.
//!
//! v2 ledger rules (see ROADMAP.md, Stage 1):
//! - Rewards are *attestations*: the signer vouches that someone else provided
//!   value. A reward whose signer is its own recipient is invalid — you cannot
//!   mint to yourself. The minter's wallet is recorded, so every coin's origin
//!   is attributable.
//! - Transfers carry a per-sender sequence number. Balances are a
//!   deterministic fold over the transaction *set* (see [`Dag::ledger`]), so
//!   double-spends resolve identically on every node and network-wide
//!   balances can never go negative, even against modified clients.

use anyhow::{anyhow, bail, Context, Result};
use libp2p::identity::{Keypair, PublicKey};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, HashMap, HashSet};
use std::io::Write;
use std::path::{Path, PathBuf};

/// Cap on structurally-valid transactions parked while their parents are
/// missing, so a malicious peer can't exhaust memory with orphans.
const MAX_ORPHANS: usize = 10_000;

/// Derive a wallet address from a peer id, exactly as the legacy code did:
/// `0x` + hex(sha256(base58 peer id)).
pub fn wallet_address(peer_id: &libp2p::PeerId) -> String {
    format!("0x{}", hex::encode(Sha256::digest(peer_id.to_base58().as_bytes())))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TxKind {
    /// Mints `amount` to `receiver`, signed by an attester (`sender`) who
    /// vouches the receiver contributed. Signer's wallet must differ from
    /// `receiver`.
    Reward,
    /// Moves `amount` from `sender` to `receiver`. `seq` is the sender's
    /// transfer sequence number (0, 1, 2, …).
    Transfer,
    /// Signer (`sender`) states they know and trust the wallet in
    /// `receiver`. No balance effect; feeds the web-of-trust views.
    Vouch,
    /// Signer sets their own display name (in `memo`). Non-unique label —
    /// the wallet is the identity, the name is decoration. Latest wins.
    Profile,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Transaction {
    pub id: String,
    pub kind: TxKind,
    pub sender: String,
    pub receiver: String,
    pub amount: u64,
    /// Per-sender transfer sequence number; 0 for rewards.
    #[serde(default)]
    pub seq: u64,
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

/// Everything covered by the transaction hash/signature, in canonical order.
struct SigningFields<'a> {
    kind: TxKind,
    sender: &'a str,
    receiver: &'a str,
    amount: u64,
    seq: u64,
    memo: &'a str,
    parents: &'a [String],
    timestamp: u64,
    public_key: &'a [u8],
}

impl SigningFields<'_> {
    /// Canonical bytes covered by the hash and the signature. Field order,
    /// length prefixes, and sorted parents make this deterministic across
    /// nodes — the legacy code hashed `format!("{:?}", HashSet)`, whose
    /// iteration order is random per process.
    fn bytes(&self) -> Vec<u8> {
        let mut out = Vec::new();
        let mut put = |bytes: &[u8]| {
            out.extend_from_slice(&(bytes.len() as u64).to_be_bytes());
            out.extend_from_slice(bytes);
        };
        put(b"timecoin-tx-v2");
        put(match self.kind {
            TxKind::Reward => b"reward",
            TxKind::Transfer => b"transfer",
            TxKind::Vouch => b"vouch",
            TxKind::Profile => b"profile",
        });
        put(self.sender.as_bytes());
        put(self.receiver.as_bytes());
        put(&self.amount.to_be_bytes());
        put(&self.seq.to_be_bytes());
        put(self.memo.as_bytes());
        for p in self.parents {
            put(p.as_bytes());
        }
        put(&self.timestamp.to_be_bytes());
        put(self.public_key);
        out
    }
}

impl Transaction {
    fn signing_bytes(&self) -> Vec<u8> {
        SigningFields {
            kind: self.kind,
            sender: &self.sender,
            receiver: &self.receiver,
            amount: self.amount,
            seq: self.seq,
            memo: &self.memo,
            parents: &self.parents,
            timestamp: self.timestamp,
            public_key: &self.public_key,
        }
        .bytes()
    }

    #[allow(clippy::too_many_arguments)] // one arg per hashed field, by design
    pub fn create(
        kind: TxKind,
        keypair: &Keypair,
        sender: String,
        receiver: String,
        amount: u64,
        seq: u64,
        memo: String,
        mut parents: Vec<String>,
        timestamp: u64,
    ) -> Result<Self> {
        parents.sort();
        parents.dedup();
        let public_key = keypair.public().encode_protobuf();
        let mut tx = Self {
            id: String::new(),
            kind,
            sender,
            receiver,
            amount,
            seq,
            memo,
            parents,
            timestamp,
            public_key,
            signature: vec![],
        };
        let bytes = tx.signing_bytes();
        tx.id = hex::encode(Sha256::digest(&bytes));
        tx.signature = keypair.sign(&bytes).context("signing transaction")?;
        Ok(tx)
    }

    /// Structural validation: id matches content, signature verifies, and the
    /// signer is authorized (transfers: signer owns the sending wallet;
    /// rewards: signer is vouching for someone *else*). Deterministic — every
    /// node reaches the same verdict — which is what makes the DAG converge.
    pub fn validate(&self) -> Result<()> {
        match self.kind {
            TxKind::Reward | TxKind::Transfer => {
                if self.amount == 0 {
                    bail!("amount must be positive");
                }
            }
            TxKind::Vouch | TxKind::Profile => {
                if self.amount != 0 {
                    bail!("vouch/profile transactions carry no amount");
                }
                if self.seq != 0 {
                    bail!("vouch/profile transactions carry no sequence number");
                }
            }
        }
        let mut sorted = self.parents.clone();
        sorted.sort();
        if sorted != self.parents {
            bail!("parents not in canonical (sorted) order");
        }
        let bytes = self.signing_bytes();
        if hex::encode(Sha256::digest(&bytes)) != self.id {
            bail!("transaction id does not match contents");
        }
        let key = PublicKey::try_decode_protobuf(&self.public_key)
            .map_err(|e| anyhow!("bad public key: {e}"))?;
        if !key.verify(&bytes, &self.signature) {
            bail!("signature verification failed");
        }
        let signer_wallet = wallet_address(&key.to_peer_id());
        match self.kind {
            TxKind::Transfer => {
                if signer_wallet != self.sender {
                    bail!("transfer not signed by the sender's key");
                }
                if self.sender == self.receiver {
                    bail!("transfer to self is pointless");
                }
            }
            TxKind::Reward => {
                if signer_wallet != self.sender {
                    bail!("reward attester must record their own wallet as sender");
                }
                if signer_wallet == self.receiver {
                    bail!("cannot mint a reward to yourself — rewards are attestations by others");
                }
                if self.seq != 0 {
                    bail!("rewards carry no sequence number");
                }
            }
            TxKind::Vouch => {
                if signer_wallet != self.sender {
                    bail!("vouch must be signed by the vouching wallet");
                }
                if signer_wallet == self.receiver {
                    bail!("cannot vouch for yourself");
                }
            }
            TxKind::Profile => {
                if signer_wallet != self.sender || self.sender != self.receiver {
                    bail!("profile must be self-signed and self-addressed");
                }
                if self.memo.is_empty() || self.memo.len() > 32 || !self.memo.chars().all(|c| !c.is_control()) {
                    bail!("display name must be 1..=32 printable characters");
                }
            }
        }
        Ok(())
    }

    pub fn is_genesis(&self) -> bool {
        self.parents.is_empty() && self.public_key.is_empty()
    }
}

/// The two genesis transactions every node of a named network computes
/// identically, so all DAGs of that network share the same roots — and DAGs
/// of *different* networks share nothing.
pub fn genesis(network: &str) -> Vec<Transaction> {
    let memo = format!("genesis:{network}");
    let g = |receiver: &str, parents: Vec<String>| {
        let mut tx = Transaction {
            id: String::new(),
            kind: TxKind::Reward,
            sender: "System".into(),
            receiver: receiver.into(),
            amount: 1,
            seq: 0,
            memo: memo.clone(),
            parents,
            timestamp: 0,
            public_key: vec![],
            signature: vec![],
        };
        tx.id = hex::encode(Sha256::digest(tx.signing_bytes()));
        tx
    };
    let g1 = g("SystemReceiver1", vec![]);
    // Second genesis references the first, as in the legacy bootstrap.
    let g2 = g("SystemReceiver2", vec![g1.id.clone()]);
    vec![g1, g2]
}

/// The balances and the set of transfers that actually took effect.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LedgerState {
    pub balances: BTreeMap<String, i64>,
    /// Ids of transfers applied by the fold. Rewards always apply.
    pub applied_transfers: HashSet<String>,
}

/// In-memory DAG with append-only on-disk persistence (JSON lines).
pub struct Dag {
    network: String,
    transactions: HashMap<String, Transaction>,
    /// Ids referenced as a parent by at least one accepted transaction.
    referenced: HashSet<String>,
    /// Valid transactions waiting for missing parents, by missing parent id.
    orphans: HashMap<String, Vec<Transaction>>,
    orphan_count: usize,
    log_path: Option<PathBuf>,
}

impl Dag {
    pub fn new(network: &str) -> Self {
        let mut dag = Self {
            network: network.to_string(),
            transactions: HashMap::new(),
            referenced: HashSet::new(),
            orphans: HashMap::new(),
            orphan_count: 0,
            log_path: None,
        };
        for g in genesis(network) {
            dag.accept(g);
        }
        dag
    }

    /// Load from `dir/dag.jsonl`, creating a fresh DAG (genesis only) if the
    /// file doesn't exist. Every stored transaction is re-validated on load.
    pub fn load(dir: &Path, network: &str) -> Result<Self> {
        let mut dag = Self::new(network);
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

    /// The requested transactions plus missing-side ancestry, breadth-first,
    /// capped — lets a syncing peer pull whole generations per round trip.
    pub fn with_ancestry(&self, ids: &[String], cap: usize) -> Vec<Transaction> {
        let mut out = Vec::new();
        let mut seen: HashSet<&str> = HashSet::new();
        let mut queue: Vec<&str> = ids.iter().map(|s| s.as_str()).collect();
        while let Some(id) = queue.pop() {
            if out.len() >= cap || !seen.insert(id) {
                continue;
            }
            if let Some(tx) = self.transactions.get(id) {
                out.push(tx.clone());
                queue.extend(tx.parents.iter().map(|p| p.as_str()));
            }
        }
        out
    }

    /// Missing parent ids currently blocking orphans — what to ask peers for.
    pub fn missing_parents(&self) -> Vec<String> {
        self.orphans.keys().cloned().collect()
    }

    /// The next sequence number this wallet should use for a transfer:
    /// one past the highest it has ever signed (applied or not), so an honest
    /// client never conflicts with itself.
    pub fn next_seq(&self, wallet: &str) -> u64 {
        self.transactions
            .values()
            .filter(|tx| tx.kind == TxKind::Transfer && tx.sender == wallet)
            .map(|tx| tx.seq + 1)
            .max()
            .unwrap_or(0)
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
        } else if !genesis(&self.network).iter().any(|g| g.id == tx.id) {
            bail!("unknown genesis transaction (different --network?)");
        }

        if let Some(missing) = tx.parents.iter().find(|p| !self.contains(p)) {
            if self.orphan_count >= MAX_ORPHANS {
                bail!("orphan pool full");
            }
            self.orphan_count += 1;
            self.orphans.entry(missing.clone()).or_default().push(tx);
            return Ok(vec![]);
        }

        let mut accepted = vec![tx.clone()];
        self.accept(tx);

        // Unblock orphans transitively.
        let mut queue: Vec<String> = vec![accepted[0].id.clone()];
        while let Some(id) = queue.pop() {
            let Some(waiting) = self.orphans.remove(&id) else { continue };
            self.orphan_count -= waiting.len();
            for w in waiting {
                if self.contains(&w.id) {
                    continue;
                }
                if let Some(missing) = w.parents.iter().find(|p| !self.contains(p)) {
                    let missing = missing.clone();
                    self.orphan_count += 1;
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

    /// Deterministic ledger fold — a pure function of the transaction *set*,
    /// so every node that has the same DAG computes the same balances no
    /// matter what order transactions arrived in.
    ///
    /// 1. All rewards apply (they only add; order is irrelevant).
    /// 2. For each (sender, seq) the winner is the lowest tx id — a
    ///    double-spend's loser is dead on every node identically.
    /// 3. Winners apply in fixpoint passes ordered by (sender, seq, id): a
    ///    transfer applies only when its seq is the sender's next *and* the
    ///    sender's balance covers it. Passes repeat until nothing changes,
    ///    so transfers funded by other transfers settle regardless of
    ///    ordering. An unfunded transfer (modified client) simply never
    ///    applies — balances cannot go negative.
    pub fn ledger(&self) -> LedgerState {
        self.ledger_view(None)
    }

    /// Like [`Dag::ledger`], but when `trusted` is given, only rewards minted
    /// by a wallet in the trusted set (plus genesis) are counted — "balances
    /// as seen by me, counting only coins vouched-for people created".
    /// Transfers behave identically; they just can't be funded by untrusted
    /// mints. Deterministic for a given (DAG, trusted set).
    pub fn ledger_view(&self, trusted: Option<&BTreeMap<String, u32>>) -> LedgerState {
        let mut balances: BTreeMap<String, i64> = BTreeMap::new();
        for tx in self.transactions.values() {
            if tx.kind == TxKind::Reward {
                let counted = match trusted {
                    None => true,
                    Some(set) => tx.is_genesis() || set.contains_key(&tx.sender),
                };
                if counted {
                    *balances.entry(tx.receiver.clone()).or_default() += tx.amount as i64;
                }
            }
        }

        // Winner per (sender, seq): lowest id.
        let mut winners: BTreeMap<(String, u64), &Transaction> = BTreeMap::new();
        for tx in self.transactions.values().filter(|t| t.kind == TxKind::Transfer) {
            winners
                .entry((tx.sender.clone(), tx.seq))
                .and_modify(|cur| {
                    if tx.id < cur.id {
                        *cur = tx;
                    }
                })
                .or_insert(tx);
        }

        let mut applied: HashSet<String> = HashSet::new();
        let mut next_seq: HashMap<&str, u64> = HashMap::new();
        loop {
            let mut progress = false;
            for ((sender, seq), tx) in &winners {
                if applied.contains(&tx.id) {
                    continue;
                }
                if *seq != *next_seq.get(sender.as_str()).unwrap_or(&0) {
                    continue;
                }
                let balance = balances.get(sender).copied().unwrap_or(0);
                if balance >= tx.amount as i64 {
                    *balances.entry(tx.sender.clone()).or_default() -= tx.amount as i64;
                    *balances.entry(tx.receiver.clone()).or_default() += tx.amount as i64;
                    next_seq.insert(sender.as_str(), seq + 1);
                    applied.insert(tx.id.clone());
                    progress = true;
                }
            }
            if !progress {
                break;
            }
        }
        LedgerState { balances, applied_transfers: applied }
    }

    pub fn balances(&self) -> BTreeMap<String, i64> {
        self.ledger().balances
    }

    pub fn balance(&self, wallet: &str) -> i64 {
        self.balances().get(wallet).copied().unwrap_or(0)
    }

    /// Wallets the viewer trusts, with their vouch distance: the viewer at
    /// depth 0, everyone the viewer vouched for at 1, everyone *they*
    /// vouched for at 2, … up to `max_depth` hops (BFS over vouch edges).
    pub fn trusted_set(&self, viewer: &str, max_depth: u32) -> BTreeMap<String, u32> {
        let mut edges: HashMap<&str, Vec<&str>> = HashMap::new();
        for tx in self.transactions.values().filter(|t| t.kind == TxKind::Vouch) {
            edges.entry(tx.sender.as_str()).or_default().push(tx.receiver.as_str());
        }
        let mut trusted: BTreeMap<String, u32> = BTreeMap::new();
        trusted.insert(viewer.to_string(), 0);
        let mut frontier: Vec<&str> = vec![viewer];
        for depth in 1..=max_depth {
            let mut next = Vec::new();
            for wallet in frontier {
                for vouched in edges.get(wallet).into_iter().flatten() {
                    if !trusted.contains_key(*vouched) {
                        trusted.insert(vouched.to_string(), depth);
                        next.push(*vouched);
                    }
                }
            }
            if next.is_empty() {
                break;
            }
            frontier = next;
        }
        trusted
    }

    /// Latest self-declared display name per wallet (labels, not identities:
    /// names are not unique and prove nothing — the wallet is the identity).
    pub fn names(&self) -> BTreeMap<String, String> {
        let mut latest: BTreeMap<String, &Transaction> = BTreeMap::new();
        for tx in self.transactions.values().filter(|t| t.kind == TxKind::Profile) {
            latest
                .entry(tx.sender.clone())
                .and_modify(|cur| {
                    if (tx.timestamp, &tx.id) > (cur.timestamp, &cur.id) {
                        *cur = tx;
                    }
                })
                .or_insert(tx);
        }
        latest.into_iter().map(|(w, tx)| (w, tx.memo.clone())).collect()
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

    const NET: &str = "test";

    fn keypair() -> Keypair {
        Keypair::generate_ed25519()
    }

    fn wallet_of(key: &Keypair) -> String {
        wallet_address(&key.public().to_peer_id())
    }

    /// `attester` vouches that `wallet` contributed.
    fn reward_to(dag: &Dag, attester: &Keypair, wallet: &str, amount: u64) -> Transaction {
        Transaction::create(
            TxKind::Reward,
            attester,
            wallet_of(attester),
            wallet.to_string(),
            amount,
            0,
            "test contribution".into(),
            dag.tips(),
            1,
        )
        .unwrap()
    }

    fn transfer(
        dag: &Dag,
        key: &Keypair,
        to: &str,
        amount: u64,
        seq: u64,
    ) -> Transaction {
        Transaction::create(
            TxKind::Transfer,
            key,
            wallet_of(key),
            to.to_string(),
            amount,
            seq,
            String::new(),
            dag.tips(),
            2,
        )
        .unwrap()
    }

    #[test]
    fn genesis_is_deterministic_and_network_scoped() {
        let a = genesis("main").iter().map(|g| g.id.clone()).collect::<Vec<_>>();
        let b = genesis("main").iter().map(|g| g.id.clone()).collect::<Vec<_>>();
        let c = genesis("koop").iter().map(|g| g.id.clone()).collect::<Vec<_>>();
        assert_eq!(a, b);
        assert_ne!(a, c);
    }

    #[test]
    fn cannot_mint_to_yourself() {
        let key = keypair();
        let dag = Dag::new(NET);
        let selfish = reward_to(&dag, &key, &wallet_of(&key), 1000);
        assert!(selfish.validate().is_err());
        let mut dag = dag;
        assert!(dag.insert(selfish).is_err());
    }

    #[test]
    fn attested_reward_then_transfer_updates_balances() {
        let alice = keypair();
        let bob = keypair();
        let mut dag = Dag::new(NET);

        // Bob attests Alice contributed.
        dag.insert(reward_to(&dag, &bob, &wallet_of(&alice), 100)).unwrap();
        assert_eq!(dag.balance(&wallet_of(&alice)), 100);

        // Alice pays Bob 40.
        dag.insert(transfer(&dag, &alice, &wallet_of(&bob), 40, 0)).unwrap();
        assert_eq!(dag.balance(&wallet_of(&alice)), 60);
        assert_eq!(dag.balance(&wallet_of(&bob)), 40);
    }

    #[test]
    fn transfer_must_be_signed_by_sender() {
        let key = keypair();
        let dag = Dag::new(NET);
        let t = Transaction::create(
            TxKind::Transfer,
            &key,
            "0xsomeoneelse".into(),
            "0xfriend".into(),
            5,
            0,
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
        let dag = Dag::new(NET);
        let mut r = reward_to(&dag, &key, "0xw", 10);
        r.amount = 1000;
        assert!(r.validate().is_err());
    }

    #[test]
    fn overdraft_never_applies_network_wide() {
        let alice = keypair();
        let bob = keypair();
        let mut dag = Dag::new(NET);
        dag.insert(reward_to(&dag, &bob, &wallet_of(&alice), 10)).unwrap();

        // A modified client signs a structurally valid transfer of 1000.
        let overdraft = transfer(&dag, &alice, &wallet_of(&bob), 1000, 0);
        dag.insert(overdraft.clone()).unwrap(); // accepted into the DAG…

        let ledger = dag.ledger();
        assert!(!ledger.applied_transfers.contains(&overdraft.id)); // …but never applied
        assert_eq!(dag.balance(&wallet_of(&alice)), 10);
        assert!(dag.balances().values().all(|b| *b >= 0));
    }

    #[test]
    fn double_spend_resolves_identically_regardless_of_arrival_order() {
        let alice = keypair();
        let bob = keypair();
        let mut base = Dag::new(NET);
        base.insert(reward_to(&base, &bob, &wallet_of(&alice), 100)).unwrap();

        // Alice signs two conflicting seq-0 transfers spending the same 100.
        let spend_x = transfer(&base, &alice, "0xmerchant_x", 100, 0);
        let spend_y = transfer(&base, &alice, "0xmerchant_y", 100, 0);

        let mut a = Dag::new(NET);
        for tx in base.all().cloned().collect::<Vec<_>>() {
            let _ = a.insert(tx);
        }
        let mut b = Dag::new(NET);
        for tx in base.all().cloned().collect::<Vec<_>>() {
            let _ = b.insert(tx);
        }

        a.insert(spend_x.clone()).unwrap();
        a.insert(spend_y.clone()).unwrap();
        b.insert(spend_y.clone()).unwrap();
        b.insert(spend_x.clone()).unwrap();

        assert_eq!(a.balances(), b.balances());
        let winner = if spend_x.id < spend_y.id { &spend_x } else { &spend_y };
        assert!(a.ledger().applied_transfers.contains(&winner.id));
        assert_eq!(a.ledger().applied_transfers.len(), 1);
        // Exactly one merchant got paid, on both nodes.
        let paid: i64 = a.balances().get("0xmerchant_x").copied().unwrap_or(0)
            + a.balances().get("0xmerchant_y").copied().unwrap_or(0);
        assert_eq!(paid, 100);
    }

    #[test]
    fn chained_transfers_settle_regardless_of_fold_input_order() {
        let alice = keypair();
        let bob = keypair();
        let carol = keypair();
        let attester = keypair();
        let mut dag = Dag::new(NET);
        // attester funds alice; alice pays bob; bob pays carol.
        dag.insert(reward_to(&dag, &attester, &wallet_of(&alice), 50)).unwrap();
        dag.insert(transfer(&dag, &alice, &wallet_of(&bob), 50, 0)).unwrap();
        dag.insert(transfer(&dag, &bob, &wallet_of(&carol), 30, 0)).unwrap();

        assert_eq!(dag.balance(&wallet_of(&alice)), 0);
        assert_eq!(dag.balance(&wallet_of(&bob)), 20);
        assert_eq!(dag.balance(&wallet_of(&carol)), 30);
    }

    fn vouch(dag: &Dag, voucher: &Keypair, for_wallet: &str) -> Transaction {
        Transaction::create(
            TxKind::Vouch,
            voucher,
            wallet_of(voucher),
            for_wallet.to_string(),
            0,
            0,
            String::new(),
            dag.tips(),
            3,
        )
        .unwrap()
    }

    #[test]
    fn cannot_vouch_for_yourself() {
        let key = keypair();
        let dag = Dag::new(NET);
        let v = vouch(&dag, &key, &wallet_of(&key));
        assert!(v.validate().is_err());
    }

    #[test]
    fn trust_bfs_respects_depth() {
        let a = keypair();
        let b = keypair();
        let c = keypair();
        let mut dag = Dag::new(NET);
        dag.insert(vouch(&dag, &a, &wallet_of(&b))).unwrap(); // a → b
        dag.insert(vouch(&dag, &b, &wallet_of(&c))).unwrap(); // b → c

        let depth1 = dag.trusted_set(&wallet_of(&a), 1);
        assert!(depth1.contains_key(&wallet_of(&b)));
        assert!(!depth1.contains_key(&wallet_of(&c)));

        let depth2 = dag.trusted_set(&wallet_of(&a), 2);
        assert_eq!(depth2.get(&wallet_of(&c)), Some(&2));
        // Trust is directional: b never vouched for a.
        assert!(!dag.trusted_set(&wallet_of(&c), 3).contains_key(&wallet_of(&a)));
    }

    #[test]
    fn trusted_balances_ignore_unvouched_minters() {
        let me = keypair();
        let friend = keypair();
        let stranger = keypair();
        let worker = keypair();
        let mut dag = Dag::new(NET);
        dag.insert(vouch(&dag, &me, &wallet_of(&friend))).unwrap();
        // Friend (trusted) mints 10 to worker; stranger mints 1000 to worker.
        dag.insert(reward_to(&dag, &friend, &wallet_of(&worker), 10)).unwrap();
        dag.insert(reward_to(&dag, &stranger, &wallet_of(&worker), 1000)).unwrap();

        assert_eq!(dag.balance(&wallet_of(&worker)), 1010); // raw view
        let trusted = dag.trusted_set(&wallet_of(&me), 3);
        let view = dag.ledger_view(Some(&trusted));
        assert_eq!(view.balances.get(&wallet_of(&worker)), Some(&10)); // my view
    }

    #[test]
    fn profile_latest_name_wins_and_is_validated() {
        let key = keypair();
        let mut dag = Dag::new(NET);
        let name = |dag: &Dag, text: &str, ts: u64| {
            Transaction::create(
                TxKind::Profile,
                &key,
                wallet_of(&key),
                wallet_of(&key),
                0,
                0,
                text.into(),
                dag.tips(),
                ts,
            )
            .unwrap()
        };
        dag.insert(name(&dag, "umut", 10)).unwrap();
        dag.insert(name(&dag, "umut-v2", 20)).unwrap();
        assert_eq!(dag.names().get(&wallet_of(&key)).map(String::as_str), Some("umut-v2"));

        let too_long = name(&dag, &"x".repeat(33), 30);
        assert!(too_long.validate().is_err());
    }

    #[test]
    fn orphans_resolve_when_parent_arrives() {
        let attester = keypair();
        let mut a = Dag::new(NET);
        let r1 = reward_to(&a, &attester, "0xw", 10);
        a.insert(r1.clone()).unwrap();
        let r2 = reward_to(&a, &attester, "0xw", 20); // parents include r1
        a.insert(r2.clone()).unwrap();

        // Node B receives child before parent.
        let mut b = Dag::new(NET);
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
    fn with_ancestry_returns_reachable_history() {
        let attester = keypair();
        let mut dag = Dag::new(NET);
        let r1 = reward_to(&dag, &attester, "0xw", 10);
        dag.insert(r1).unwrap();
        let r2 = reward_to(&dag, &attester, "0xw", 20);
        dag.insert(r2.clone()).unwrap();
        // From the single tip, ancestry reaches everything.
        let txs = dag.with_ancestry(&[r2.id.clone()], 100);
        assert_eq!(txs.len(), dag.len());
    }

    #[test]
    fn persistence_roundtrip() {
        let dir = std::env::temp_dir().join(format!("timecoin-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let attester = keypair();
        {
            let mut dag = Dag::load(&dir, NET).unwrap();
            let r = reward_to(&dag, &attester, "0xw", 10);
            dag.insert(r).unwrap();
        }
        let dag = Dag::load(&dir, NET).unwrap();
        assert_eq!(dag.balance("0xw"), 10);
        assert_eq!(dag.len(), 3); // 2 genesis + 1 reward
        let _ = std::fs::remove_dir_all(&dir);
    }
}
