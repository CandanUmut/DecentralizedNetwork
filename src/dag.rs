//! The TimeCoin DAG ("tangle"): contribution records attach to current tips,
//! are validated, and balances are derived by folding over the whole DAG.
//!
//! v2 ledger rules (see docs/ROADMAP.md, Stage 1):
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

/// Clock skew tolerated before a transaction is considered future-dated and
/// parked until its time arrives.
const FUTURE_SKEW_SECS: u64 = 300;

const SECS_PER_DAY: u64 = 86_400;

/// Community economy parameters. They are baked into the genesis
/// transactions, so every node of a community enforces the same rules —
/// and different rules mean a different (disjoint) network by construction.
#[derive(Debug, Clone)]
pub struct Params {
    pub network: String,
    /// How much TimeCoin any one member's *thanks* can create per day.
    /// The scarcity that makes the coin mean something. 0 = unlimited
    /// (useful for tests and toy networks).
    pub daily_allowance: u64,
}

pub fn day_of(timestamp: u64) -> u64 {
    timestamp / SECS_PER_DAY
}

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
    /// Signer withdraws their vouch for `receiver`. For each
    /// (voucher, vouchee) pair the latest statement wins.
    Revoke,
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
            TxKind::Revoke => b"revoke",
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
            TxKind::Vouch | TxKind::Revoke | TxKind::Profile => {
                if self.amount != 0 {
                    bail!("vouch/revoke/profile transactions carry no amount");
                }
                if self.seq != 0 {
                    bail!("vouch/revoke/profile transactions carry no sequence number");
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
            TxKind::Vouch | TxKind::Revoke => {
                if signer_wallet != self.sender {
                    bail!("vouch/revoke must be signed by the vouching wallet");
                }
                if signer_wallet == self.receiver {
                    bail!("cannot vouch for/revoke yourself");
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

/// The two genesis transactions every node of a community computes
/// identically, so all DAGs of that community share the same roots — and
/// communities with different names *or different economy rules* share
/// nothing by construction.
pub fn genesis(params: &Params) -> Vec<Transaction> {
    let memo = format!("genesis:{}:allowance={}", params.network, params.daily_allowance);
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
    /// Ids of transfers applied by the fold.
    pub applied_transfers: HashSet<String>,
    /// Ids of rewards that counted (within allowance/caps/trust).
    pub counted_rewards: HashSet<String>,
}

/// In-memory DAG with append-only on-disk persistence (JSON lines).
pub struct Dag {
    params: Params,
    transactions: HashMap<String, Transaction>,
    /// Ids referenced as a parent by at least one accepted transaction.
    referenced: HashSet<String>,
    /// Valid transactions waiting for missing parents, by missing parent id.
    orphans: HashMap<String, Vec<Transaction>>,
    orphan_count: usize,
    /// Future-dated transactions parked until their claimed time arrives —
    /// you cannot pre-farm tomorrow's allowance today.
    parked: Vec<Transaction>,
    log_path: Option<PathBuf>,
}

impl Dag {
    pub fn new(params: &Params) -> Self {
        let mut dag = Self {
            params: params.clone(),
            transactions: HashMap::new(),
            referenced: HashSet::new(),
            orphans: HashMap::new(),
            orphan_count: 0,
            parked: Vec::new(),
            log_path: None,
        };
        for g in genesis(params) {
            dag.accept(g);
        }
        dag
    }

    pub fn params(&self) -> &Params {
        &self.params
    }

    /// Load from `dir/dag.jsonl`, creating a fresh DAG (genesis only) if the
    /// file doesn't exist. Every stored transaction is re-validated on load.
    pub fn load(dir: &Path, params: &Params) -> Result<Self> {
        let mut dag = Self::new(params);
        let path = dir.join("dag.jsonl");
        if path.exists() {
            let data = std::fs::read_to_string(&path)
                .with_context(|| format!("reading {}", path.display()))?;
            for line in data.lines().filter(|l| !l.trim().is_empty()) {
                let tx: Transaction = serde_json::from_str(line).context("corrupt dag.jsonl line")?;
                // Stored transactions were accepted before; don't re-park.
                if let Err(e) = dag.insert(tx.clone(), u64::MAX) {
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

    /// Insert a transaction (local or from the network). `now` is the local
    /// clock in unix seconds.
    ///
    /// Returns `Ok(newly_accepted)`: the transaction itself plus any orphans
    /// it unblocked. A structurally valid transaction with unknown parents is
    /// parked as an orphan; a future-dated one is parked until its claimed
    /// time arrives (see [`Dag::release_due`]). Both return an empty vec.
    pub fn insert(&mut self, tx: Transaction, now: u64) -> Result<Vec<Transaction>> {
        if self.contains(&tx.id) {
            return Ok(vec![]);
        }
        if !tx.is_genesis() {
            tx.validate()?;
        } else if !genesis(&self.params).iter().any(|g| g.id == tx.id) {
            bail!("unknown genesis transaction (different --network or economy rules?)");
        }

        // A claimed future time would let someone farm tomorrow's allowance
        // today; park it until the network's clocks agree it exists.
        if tx.timestamp > now.saturating_add(FUTURE_SKEW_SECS) {
            if self.parked.len() >= MAX_ORPHANS {
                bail!("future-dated pool full");
            }
            self.parked.push(tx);
            return Ok(vec![]);
        }

        if let Some(missing) = tx.parents.iter().find(|p| !self.contains(p)) {
            if self.orphan_count >= MAX_ORPHANS {
                bail!("orphan pool full");
            }
            self.orphan_count += 1;
            self.orphans.entry(missing.clone()).or_default().push(tx);
            return Ok(vec![]);
        }
        self.check_parent_times(&tx)?;

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
                } else if self.check_parent_times(&w).is_ok() {
                    queue.push(w.id.clone());
                    accepted.push(w.clone());
                    self.accept(w);
                }
            }
        }
        Ok(accepted)
    }

    /// Time flows forward along the DAG: a transaction may not claim a time
    /// earlier than any of its parents. Blocks trivial backdating.
    fn check_parent_times(&self, tx: &Transaction) -> Result<()> {
        let max_parent_ts = tx
            .parents
            .iter()
            .filter_map(|p| self.transactions.get(p))
            .map(|p| p.timestamp)
            .max()
            .unwrap_or(0);
        if tx.timestamp < max_parent_ts {
            bail!("timestamp earlier than a parent's — time flows forward along the DAG");
        }
        Ok(())
    }

    /// Accept parked future-dated transactions whose time has arrived.
    /// Returns the newly accepted transactions.
    pub fn release_due(&mut self, now: u64) -> Vec<Transaction> {
        let due: Vec<Transaction> = {
            let (due, keep): (Vec<_>, Vec<_>) = std::mem::take(&mut self.parked)
                .into_iter()
                .partition(|tx| tx.timestamp <= now.saturating_add(FUTURE_SKEW_SECS));
            self.parked = keep;
            due
        };
        let mut accepted = Vec::new();
        for tx in due {
            if let Ok(mut a) = self.insert(tx, now) {
                accepted.append(&mut a);
            }
        }
        accepted
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
        self.ledger_view(None, 0)
    }

    /// Like [`Dag::ledger`], but when `trusted` is given, only rewards minted
    /// by a wallet in the trusted set (plus genesis) are counted — "balances
    /// as seen by me, counting only coins vouched-for people created".
    /// `mint_cap` > 0 additionally limits how much total mint any single
    /// attester's rewards contribute (blast-radius limit for a compromised
    /// or colluding vouched wallet); rewards are considered in (timestamp,
    /// id) order so every rule below is a pure function of the set.
    ///
    /// Community economy (always on, raw and trusted views alike): each
    /// attester's rewards count only up to `daily_allowance` TC per claimed
    /// day. Combined with parent-time monotonicity and future-parking at
    /// insert, thanks-per-day is the scarcity of the coin.
    ///
    /// In trusted views a member's rewards additionally count only from the
    /// moment a trusted wallet first vouched for them — a newcomer cannot
    /// arrive with a "backdated history" of self-assigned generosity.
    pub fn ledger_view(&self, trusted: Option<&BTreeMap<String, u32>>, mint_cap: u64) -> LedgerState {
        let mut balances: BTreeMap<String, i64> = BTreeMap::new();
        let mut counted_rewards: HashSet<String> = HashSet::new();
        let mut rewards: Vec<&Transaction> = self
            .transactions
            .values()
            .filter(|t| t.kind == TxKind::Reward)
            .collect();
        rewards.sort_by(|a, b| (a.timestamp, &a.id).cmp(&(b.timestamp, &b.id)));

        // Membership anchor for trusted views: earliest vouch for a wallet
        // by anyone in the trusted set.
        let anchors: HashMap<&str, u64> = match trusted {
            None => HashMap::new(),
            Some(set) => {
                let mut anchors: HashMap<&str, u64> = HashMap::new();
                for tx in self.transactions.values().filter(|t| t.kind == TxKind::Vouch) {
                    if set.contains_key(&tx.sender) {
                        anchors
                            .entry(tx.receiver.as_str())
                            .and_modify(|t| *t = (*t).min(tx.timestamp))
                            .or_insert(tx.timestamp);
                    }
                }
                anchors
            }
        };

        let mut minted_by: HashMap<&str, u64> = HashMap::new();
        let mut minted_by_day: HashMap<(&str, u64), u64> = HashMap::new();
        for tx in rewards {
            // Genesis marker: an empty public key is only insertable when the
            // id matches the network's known genesis transactions.
            let genesis = tx.public_key.is_empty();
            if let Some(set) = trusted {
                if !genesis && !set.contains_key(&tx.sender) {
                    continue;
                }
                // Viewer (depth 0) is anchored at 0; others at first vouch.
                if !genesis && set.get(&tx.sender) != Some(&0) {
                    match anchors.get(tx.sender.as_str()) {
                        Some(anchor) if tx.timestamp >= *anchor => {}
                        _ => continue,
                    }
                }
            }
            if !genesis {
                if self.params.daily_allowance > 0 {
                    let day = minted_by_day
                        .entry((tx.sender.as_str(), day_of(tx.timestamp)))
                        .or_default();
                    if *day + tx.amount > self.params.daily_allowance {
                        continue;
                    }
                    *day += tx.amount;
                }
                if mint_cap > 0 {
                    let so_far = minted_by.entry(tx.sender.as_str()).or_default();
                    if *so_far + tx.amount > mint_cap {
                        continue;
                    }
                    *so_far += tx.amount;
                }
            }
            counted_rewards.insert(tx.id.clone());
            *balances.entry(tx.receiver.clone()).or_default() += tx.amount as i64;
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
        LedgerState { balances, applied_transfers: applied, counted_rewards }
    }

    /// How much of today's giving allowance a wallet has used (counted
    /// rewards it attested with today's date).
    pub fn given_on_day(&self, wallet: &str, day: u64) -> u64 {
        let counted = self.ledger().counted_rewards;
        self.transactions
            .values()
            .filter(|t| {
                t.kind == TxKind::Reward
                    && t.sender == wallet
                    && day_of(t.timestamp) == day
                    && counted.contains(&t.id)
            })
            .map(|t| t.amount)
            .sum()
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
        // For each (voucher, vouchee) pair, the latest vouch/revoke wins.
        let mut latest: HashMap<(&str, &str), &Transaction> = HashMap::new();
        for tx in self
            .transactions
            .values()
            .filter(|t| matches!(t.kind, TxKind::Vouch | TxKind::Revoke))
        {
            latest
                .entry((tx.sender.as_str(), tx.receiver.as_str()))
                .and_modify(|cur| {
                    if (tx.timestamp, &tx.id) > (cur.timestamp, &cur.id) {
                        *cur = tx;
                    }
                })
                .or_insert(tx);
        }
        let mut edges: HashMap<&str, Vec<&str>> = HashMap::new();
        for ((voucher, vouchee), tx) in latest {
            if tx.kind == TxKind::Vouch {
                edges.entry(voucher).or_default().push(vouchee);
            }
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

    const NOW: u64 = 1_800_000_000;

    fn params(daily_allowance: u64) -> Params {
        Params { network: "test".into(), daily_allowance }
    }

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
            10,
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
            20,
        )
        .unwrap()
    }

    #[test]
    fn genesis_is_deterministic_and_network_scoped() {
        let a = genesis(&Params { network: "main".into(), daily_allowance: 100 }).iter().map(|g| g.id.clone()).collect::<Vec<_>>();
        let b = genesis(&Params { network: "main".into(), daily_allowance: 100 }).iter().map(|g| g.id.clone()).collect::<Vec<_>>();
        let c = genesis(&Params { network: "koop".into(), daily_allowance: 100 }).iter().map(|g| g.id.clone()).collect::<Vec<_>>();
        assert_eq!(a, b);
        assert_ne!(a, c);
    }

    #[test]
    fn cannot_mint_to_yourself() {
        let key = keypair();
        let dag = Dag::new(&params(0));
        let selfish = reward_to(&dag, &key, &wallet_of(&key), 1000);
        assert!(selfish.validate().is_err());
        let mut dag = dag;
        assert!(dag.insert(selfish, NOW).is_err());
    }

    #[test]
    fn attested_reward_then_transfer_updates_balances() {
        let alice = keypair();
        let bob = keypair();
        let mut dag = Dag::new(&params(0));

        // Bob attests Alice contributed.
        dag.insert(reward_to(&dag, &bob, &wallet_of(&alice), 100), NOW).unwrap();
        assert_eq!(dag.balance(&wallet_of(&alice)), 100);

        // Alice pays Bob 40.
        dag.insert(transfer(&dag, &alice, &wallet_of(&bob), 40, 0), NOW).unwrap();
        assert_eq!(dag.balance(&wallet_of(&alice)), 60);
        assert_eq!(dag.balance(&wallet_of(&bob)), 40);
    }

    #[test]
    fn transfer_must_be_signed_by_sender() {
        let key = keypair();
        let dag = Dag::new(&params(0));
        let t = Transaction::create(
            TxKind::Transfer,
            &key,
            "0xsomeoneelse".into(),
            "0xfriend".into(),
            5,
            0,
            String::new(),
            dag.tips(),
            20,
        )
        .unwrap();
        assert!(t.validate().is_err());
    }

    #[test]
    fn tampered_amount_fails_validation() {
        let key = keypair();
        let dag = Dag::new(&params(0));
        let mut r = reward_to(&dag, &key, "0xw", 10);
        r.amount = 1000;
        assert!(r.validate().is_err());
    }

    #[test]
    fn overdraft_never_applies_network_wide() {
        let alice = keypair();
        let bob = keypair();
        let mut dag = Dag::new(&params(0));
        dag.insert(reward_to(&dag, &bob, &wallet_of(&alice), 10), NOW).unwrap();

        // A modified client signs a structurally valid transfer of 1000.
        let overdraft = transfer(&dag, &alice, &wallet_of(&bob), 1000, 0);
        dag.insert(overdraft.clone(), NOW).unwrap(); // accepted into the DAG…

        let ledger = dag.ledger();
        assert!(!ledger.applied_transfers.contains(&overdraft.id)); // …but never applied
        assert_eq!(dag.balance(&wallet_of(&alice)), 10);
        assert!(dag.balances().values().all(|b| *b >= 0));
    }

    #[test]
    fn double_spend_resolves_identically_regardless_of_arrival_order() {
        let alice = keypair();
        let bob = keypair();
        let mut base = Dag::new(&params(0));
        base.insert(reward_to(&base, &bob, &wallet_of(&alice), 100), NOW).unwrap();

        // Alice signs two conflicting seq-0 transfers spending the same 100.
        let spend_x = transfer(&base, &alice, "0xmerchant_x", 100, 0);
        let spend_y = transfer(&base, &alice, "0xmerchant_y", 100, 0);

        let mut a = Dag::new(&params(0));
        for tx in base.all().cloned().collect::<Vec<_>>() {
            let _ = a.insert(tx, NOW);
        }
        let mut b = Dag::new(&params(0));
        for tx in base.all().cloned().collect::<Vec<_>>() {
            let _ = b.insert(tx, NOW);
        }

        a.insert(spend_x.clone(), NOW).unwrap();
        a.insert(spend_y.clone(), NOW).unwrap();
        b.insert(spend_y.clone(), NOW).unwrap();
        b.insert(spend_x.clone(), NOW).unwrap();

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
        let mut dag = Dag::new(&params(0));
        // attester funds alice; alice pays bob; bob pays carol.
        dag.insert(reward_to(&dag, &attester, &wallet_of(&alice), 50), NOW).unwrap();
        dag.insert(transfer(&dag, &alice, &wallet_of(&bob), 50, 0), NOW).unwrap();
        dag.insert(transfer(&dag, &bob, &wallet_of(&carol), 30, 0), NOW).unwrap();

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
            5,
        )
        .unwrap()
    }

    #[test]
    fn cannot_vouch_for_yourself() {
        let key = keypair();
        let dag = Dag::new(&params(0));
        let v = vouch(&dag, &key, &wallet_of(&key));
        assert!(v.validate().is_err());
    }

    #[test]
    fn trust_bfs_respects_depth() {
        let a = keypair();
        let b = keypair();
        let c = keypair();
        let mut dag = Dag::new(&params(0));
        dag.insert(vouch(&dag, &a, &wallet_of(&b)), NOW).unwrap(); // a → b
        dag.insert(vouch(&dag, &b, &wallet_of(&c)), NOW).unwrap(); // b → c

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
        let mut dag = Dag::new(&params(0));
        dag.insert(vouch(&dag, &me, &wallet_of(&friend)), NOW).unwrap();
        // Friend (trusted) mints 10 to worker; stranger mints 1000 to worker.
        dag.insert(reward_to(&dag, &friend, &wallet_of(&worker), 10), NOW).unwrap();
        dag.insert(reward_to(&dag, &stranger, &wallet_of(&worker), 1000), NOW).unwrap();

        assert_eq!(dag.balance(&wallet_of(&worker)), 1010); // raw view
        let trusted = dag.trusted_set(&wallet_of(&me), 3);
        let view = dag.ledger_view(Some(&trusted), 0);
        assert_eq!(view.balances.get(&wallet_of(&worker)), Some(&10)); // my view
    }

    #[test]
    fn revoke_cuts_the_trust_edge_and_its_subtree() {
        let me = keypair();
        let friend = keypair();
        let their_friend = keypair();
        let mut dag = Dag::new(&params(0));
        dag.insert(vouch(&dag, &me, &wallet_of(&friend)), NOW).unwrap();
        dag.insert(vouch(&dag, &friend, &wallet_of(&their_friend)), NOW).unwrap();
        assert!(dag.trusted_set(&wallet_of(&me), 3).contains_key(&wallet_of(&their_friend)));

        // I withdraw my vouch (later timestamp wins).
        let revoke = Transaction::create(
            TxKind::Revoke,
            &me,
            wallet_of(&me),
            wallet_of(&friend),
            0,
            0,
            String::new(),
            dag.tips(),
            99,
        )
        .unwrap();
        dag.insert(revoke, NOW).unwrap();
        let trusted = dag.trusted_set(&wallet_of(&me), 3);
        assert!(!trusted.contains_key(&wallet_of(&friend)));
        assert!(!trusted.contains_key(&wallet_of(&their_friend)));
        assert_eq!(trusted.len(), 1); // just me
    }

    #[test]
    fn mint_cap_limits_a_single_attesters_blast_radius() {
        let me = keypair();
        let friend = keypair();
        let mut dag = Dag::new(&params(0));
        dag.insert(vouch(&dag, &me, &wallet_of(&friend)), NOW).unwrap();
        // Trusted friend goes rogue and mints 3 × 400 to an accomplice.
        for ts in [10, 20, 30] {
            let tx = Transaction::create(
                TxKind::Reward,
                &friend,
                wallet_of(&friend),
                "0xaccomplice".into(),
                400,
                0,
                "rogue".into(),
                dag.tips(),
                ts,
            )
            .unwrap();
            dag.insert(tx, NOW).unwrap();
        }
        let trusted = dag.trusted_set(&wallet_of(&me), 3);
        // Uncapped trusted view counts all 1200.
        assert_eq!(
            dag.ledger_view(Some(&trusted), 0).balances.get("0xaccomplice"),
            Some(&1200)
        );
        // With a 1000 cap, only the first two rewards (800) fit.
        assert_eq!(
            dag.ledger_view(Some(&trusted), 1000).balances.get("0xaccomplice"),
            Some(&800)
        );
    }

    #[test]
    fn profile_latest_name_wins_and_is_validated() {
        let key = keypair();
        let mut dag = Dag::new(&params(0));
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
        dag.insert(name(&dag, "umut", 10), NOW).unwrap();
        dag.insert(name(&dag, "umut-v2", 20), NOW).unwrap();
        assert_eq!(dag.names().get(&wallet_of(&key)).map(String::as_str), Some("umut-v2"));

        let too_long = name(&dag, &"x".repeat(33), 30);
        assert!(too_long.validate().is_err());
    }

    #[test]
    fn orphans_resolve_when_parent_arrives() {
        let attester = keypair();
        let mut a = Dag::new(&params(0));
        let r1 = reward_to(&a, &attester, "0xw", 10);
        a.insert(r1.clone(), NOW).unwrap();
        let r2 = reward_to(&a, &attester, "0xw", 20); // parents include r1
        a.insert(r2.clone(), NOW).unwrap();

        // Node B receives child before parent.
        let mut b = Dag::new(&params(0));
        assert!(b.insert(r2.clone(), NOW).unwrap().is_empty());
        assert!(!b.contains(&r2.id));
        assert_eq!(b.missing_parents(), vec![r1.id.clone()]);
        let accepted = b.insert(r1.clone(), NOW).unwrap();
        assert_eq!(accepted.len(), 2);
        assert!(b.contains(&r2.id));
        assert_eq!(b.tips(), a.tips());
        assert_eq!(b.balances(), a.balances());
    }

    #[test]
    fn with_ancestry_returns_reachable_history() {
        let attester = keypair();
        let mut dag = Dag::new(&params(0));
        let r1 = reward_to(&dag, &attester, "0xw", 10);
        dag.insert(r1, NOW).unwrap();
        let r2 = reward_to(&dag, &attester, "0xw", 20);
        dag.insert(r2.clone(), NOW).unwrap();
        // From the single tip, ancestry reaches everything.
        let txs = dag.with_ancestry(&[r2.id.clone()], 100);
        assert_eq!(txs.len(), dag.len());
    }

    #[test]
    fn daily_allowance_caps_minting_per_attester_per_day() {
        let a = keypair();
        let mut dag = Dag::new(&params(50)); // 50 TC/day community
        let day1 = SECS_PER_DAY + 10;
        let mint = |dag: &Dag, ts: u64, amount: u64| {
            Transaction::create(
                TxKind::Reward, &a, wallet_of(&a), "0xw".into(), amount, 0,
                "thanks".into(), dag.tips(), ts,
            ).unwrap()
        };
        dag.insert(mint(&dag, 10, 30), NOW).unwrap();
        dag.insert(mint(&dag, 20, 30), NOW).unwrap(); // over budget: 60 > 50
        assert_eq!(dag.balance("0xw"), 30); // second reward doesn't count
        dag.insert(mint(&dag, day1, 30), NOW).unwrap(); // next day: fresh budget
        assert_eq!(dag.balance("0xw"), 60);
        // The uncounted reward is in the DAG (visible) but worth nothing.
        assert_eq!(dag.ledger().counted_rewards.len(), 2 + 2); // 2 genesis + 2 counted
    }

    #[test]
    fn future_dated_transactions_park_until_due() {
        let a = keypair();
        let mut dag = Dag::new(&params(0));
        let future = NOW + 7 * SECS_PER_DAY;
        let tx = Transaction::create(
            TxKind::Reward, &a, wallet_of(&a), "0xw".into(), 10, 0,
            "from the future".into(), dag.tips(), future,
        ).unwrap();
        assert!(dag.insert(tx.clone(), NOW).unwrap().is_empty());
        assert!(!dag.contains(&tx.id)); // parked, not accepted
        assert!(dag.release_due(NOW).is_empty()); // still not due
        let released = dag.release_due(future + 1);
        assert_eq!(released.len(), 1);
        assert!(dag.contains(&tx.id));
    }

    #[test]
    fn timestamps_flow_forward_along_the_dag() {
        let a = keypair();
        let mut dag = Dag::new(&params(0));
        dag.insert(reward_to(&dag, &a, "0xw", 10), NOW).unwrap(); // ts 10
        let backdated = Transaction::create(
            TxKind::Reward, &a, wallet_of(&a), "0xw".into(), 10, 0,
            "backdated".into(), dag.tips(), 5, // earlier than its parent (10)
        ).unwrap();
        assert!(dag.insert(backdated, NOW).is_err());
    }

    #[test]
    fn trusted_view_ignores_rewards_predating_membership() {
        let me = keypair();
        let newcomer = keypair();
        let mut dag = Dag::new(&params(0));
        // Newcomer arrives with a self-made "history" of generosity (ts 10),
        // then gets vouched in at ts 50.
        dag.insert(reward_to(&dag, &newcomer, "0xcrony", 500), NOW).unwrap(); // ts 10
        let vouch_late = Transaction::create(
            TxKind::Vouch, &me, wallet_of(&me), wallet_of(&newcomer), 0, 0,
            String::new(), dag.tips(), 50,
        ).unwrap();
        dag.insert(vouch_late, NOW).unwrap();
        let after = Transaction::create(
            TxKind::Reward, &newcomer, wallet_of(&newcomer), "0xcrony".into(), 20, 0,
            "real".into(), dag.tips(), 60,
        ).unwrap();
        dag.insert(after, NOW).unwrap();

        let trusted = dag.trusted_set(&wallet_of(&me), 3);
        let view = dag.ledger_view(Some(&trusted), 0);
        // Only the reward made *after* membership counts in my view.
        assert_eq!(view.balances.get("0xcrony"), Some(&20));
        assert_eq!(dag.balance("0xcrony"), 520); // raw view still shows all
    }

    #[test]
    fn persistence_roundtrip() {
        let dir = std::env::temp_dir().join(format!("timecoin-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let attester = keypair();
        {
            let mut dag = Dag::load(&dir, &params(0)).unwrap();
            let r = reward_to(&dag, &attester, "0xw", 10);
            dag.insert(r, NOW).unwrap();
        }
        let dag = Dag::load(&dir, &params(0)).unwrap();
        assert_eq!(dag.balance("0xw"), 10);
        assert_eq!(dag.len(), 3); // 2 genesis + 1 reward
        let _ = std::fs::remove_dir_all(&dir);
    }
}
