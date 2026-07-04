//! The libp2p layer, entirely on upstream released crates (no fork).
//!
//! Protocols (all namespaced by `--network` so communities stay isolated):
//! - gossipsub topic `timecoin/<net>/tx/v2` — new-transaction broadcast
//! - `/timecoin/<net>/sync/1.1.0`  — DAG anti-entropy: tips on connect,
//!   missing ancestry pulled in batched rounds (delta sync)
//! - `/timecoin/<net>/msg/1.0.0`   — direct text messages (free; transport is
//!   already Noise-encrypted and peer-authenticated)
//! - `/timecoin/<net>/blob/1.0.0`  — paid content-addressed storage:
//!   quote → pay on the ledger → put(payment id) → get(hash)

use crate::dag::{wallet_address, Dag, Transaction, TxKind};
use anyhow::{anyhow, Result};
use futures::StreamExt;
use libp2p::{
    autonat, dcutr, gossipsub, identify, kad, mdns, noise, ping, relay,
    request_response::{self, OutboundRequestId, ProtocolSupport, ResponseChannel},
    swarm::{behaviour::toggle::Toggle, NetworkBehaviour, SwarmEvent},
    upnp, yamux, Multiaddr, PeerId, StreamProtocol, Swarm,
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::{HashMap, HashSet};
use std::io::Write as _;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::sync::{mpsc, oneshot};
use tracing::{debug, info, warn};

/// How many transactions a single sync response may carry.
const SYNC_BATCH: usize = 512;
/// Cap on the in-memory message inbox.
const INBOX_CAP: usize = 1000;

// ---------------------------------------------------------------------------
// Wire messages
// ---------------------------------------------------------------------------

/// DAG anti-entropy. `Tips` opens a sync: the responder sends its tip
/// transactions plus recent ancestry; the requester's orphan pool then pulls
/// whatever is still missing via `Get`, one batched round per generation gap.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum SyncRequest {
    Tips,
    Get(Vec<String>),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum SyncResponse {
    Txs(Vec<Transaction>),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MsgRequest {
    pub text: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum MsgResponse {
    Ack,
    Refused { reason: String },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum BlobRequest {
    /// Ask what storing `size` bytes costs (in TimeCoin).
    Quote { size: u64 },
    /// Store `data`; `payment` is the ledger id of a transfer paying the
    /// provider's wallet at least the quoted price (ignored if price is 0).
    Put {
        #[serde(with = "serde_bytes")]
        data: Vec<u8>,
        payment: String,
    },
    /// Fetch a blob by its sha256 hex hash.
    Get { hash: String },
    /// Custody spot-check: return `len` bytes of the blob at `offset`.
    Range { hash: String, offset: u64, len: u32 },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum BlobResponse {
    Price { price: u64 },
    Stored { hash: String },
    Blob {
        #[serde(with = "serde_bytes")]
        data: Vec<u8>,
    },
    RangeData {
        #[serde(with = "serde_bytes")]
        data: Vec<u8>,
    },
    Refused { reason: String },
}

/// A received direct message.
#[derive(Debug, Clone, Serialize)]
pub struct InboxMessage {
    pub from: String,
    pub from_wallet: String,
    pub text: String,
    pub received_at: u64,
}

// ---------------------------------------------------------------------------
// Behaviour
// ---------------------------------------------------------------------------

#[derive(NetworkBehaviour)]
pub struct Behaviour {
    gossipsub: gossipsub::Behaviour,
    mdns: Toggle<mdns::tokio::Behaviour>,
    identify: identify::Behaviour,
    ping: ping::Behaviour,
    autonat: autonat::Behaviour,
    relay_server: relay::Behaviour,
    relay_client: relay::client::Behaviour,
    dcutr: dcutr::Behaviour,
    upnp: upnp::tokio::Behaviour,
    kad: kad::Behaviour<kad::store::MemoryStore>,
    sync: request_response::cbor::Behaviour<SyncRequest, SyncResponse>,
    msg: request_response::cbor::Behaviour<MsgRequest, MsgResponse>,
    blob: request_response::cbor::Behaviour<BlobRequest, BlobResponse>,
}

/// Commands from the API/CLI into the network task (the swarm is owned by a
/// single task; the legacy Arc<Mutex<Swarm>> pattern deadlocked by design).
pub enum Command {
    PublishTx(Transaction),
    Dial(Multiaddr, oneshot::Sender<Result<()>>),
    ListenAddrs(oneshot::Sender<Vec<Multiaddr>>),
    SendMessage(PeerId, String, oneshot::Sender<Result<()>>),
    Blob(PeerId, BlobRequest, oneshot::Sender<Result<BlobResponse>>),
}

/// Node-level configuration the network task needs.
pub struct Config {
    pub network: String,
    pub enable_mdns: bool,
    pub data_dir: PathBuf,
    /// TimeCoin per started 100 KiB for storing a blob. 0 = store for free.
    pub blob_price: u64,
    /// Largest blob this node will store for others.
    pub blob_max_bytes: u64,
    /// When > 0, accept storage payments only from wallets within this many
    /// vouch hops of ours — rigged coin from outside the neighborhood buys
    /// nothing here.
    pub blob_trust_depth: u32,
}

impl Config {
    pub fn quote(&self, size: u64) -> u64 {
        self.blob_price * size.div_ceil(100 * 1024).max(1)
    }
}

pub struct Network {
    pub swarm: Swarm<Behaviour>,
    pub dag: Arc<Mutex<Dag>>,
    pub peers: Arc<Mutex<HashSet<PeerId>>>,
    pub inbox: Arc<Mutex<Vec<InboxMessage>>>,
    config: Config,
    wallet: String,
    commands: mpsc::Receiver<Command>,
    topic: gossipsub::IdentTopic,
    /// Relay to reserve a slot on once we're connected to it.
    relay: Option<(PeerId, Multiaddr)>,
    relay_reserved: bool,
    pending_msgs: HashMap<OutboundRequestId, oneshot::Sender<Result<()>>>,
    pending_blobs: HashMap<OutboundRequestId, oneshot::Sender<Result<BlobResponse>>>,
    /// Ledger transfer ids already redeemed for storage (anti double-redeem).
    used_payments: HashSet<String>,
}

fn proto(network: &str, suffix: &str) -> Result<StreamProtocol> {
    StreamProtocol::try_from_owned(format!("/timecoin/{network}/{suffix}"))
        .map_err(|e| anyhow!("invalid protocol name: {e}"))
}

impl Network {
    pub fn new(
        keypair: &libp2p::identity::Keypair,
        dag: Arc<Mutex<Dag>>,
        peers: Arc<Mutex<HashSet<PeerId>>>,
        commands: mpsc::Receiver<Command>,
        config: Config,
    ) -> Result<Self> {
        let sync_proto = proto(&config.network, "sync/1.1.0")?;
        let msg_proto = proto(&config.network, "msg/1.0.0")?;
        let blob_proto = proto(&config.network, "blob/1.0.0")?;
        let kad_proto = proto(&config.network, "kad/1.0.0")?;
        let identify_proto = format!("/timecoin/{}/1.0.0", config.network);
        let enable_mdns = config.enable_mdns;

        let swarm = libp2p::SwarmBuilder::with_existing_identity(keypair.clone())
            .with_tokio()
            .with_tcp(
                libp2p::tcp::Config::default().nodelay(true),
                noise::Config::new,
                yamux::Config::default,
            )?
            .with_quic()
            .with_dns()?
            .with_relay_client(noise::Config::new, yamux::Config::default)?
            .with_behaviour(|key, relay_client| {
                let local_peer_id = key.public().to_peer_id();

                let gossipsub_config = gossipsub::ConfigBuilder::default()
                    .validation_mode(gossipsub::ValidationMode::Strict)
                    .message_id_fn(|m: &gossipsub::Message| {
                        gossipsub::MessageId::from(Sha256::digest(&m.data).to_vec())
                    })
                    .build()
                    .map_err(|e| anyhow!("gossipsub config: {e}"))?;
                let gossipsub = gossipsub::Behaviour::new(
                    gossipsub::MessageAuthenticity::Signed(key.clone()),
                    gossipsub_config,
                )
                .map_err(|e| anyhow!("gossipsub: {e}"))?;

                let mdns = if enable_mdns {
                    Some(mdns::tokio::Behaviour::new(
                        mdns::Config::default(),
                        local_peer_id,
                    )?)
                } else {
                    None
                }
                .into();

                let identify = identify::Behaviour::new(identify::Config::new(
                    identify_proto.clone(),
                    key.public(),
                ));

                let rr = |p: StreamProtocol| ([(p, ProtocolSupport::Full)], Default::default());

                // Peer discovery beyond the bootstrap list: a DHT under our
                // own protocol id. Server mode so every node contributes
                // routing; the table doubles as the address book for dialing
                // peers-of-peers by id (messages, blobs).
                let mut kad = kad::Behaviour::with_config(
                    local_peer_id,
                    kad::store::MemoryStore::new(local_peer_id),
                    kad::Config::new(kad_proto.clone()),
                );
                kad.set_mode(Some(kad::Mode::Server));

                Ok(Behaviour {
                    gossipsub,
                    mdns,
                    identify,
                    ping: ping::Behaviour::default(),
                    autonat: autonat::Behaviour::new(local_peer_id, autonat::Config::default()),
                    relay_server: relay::Behaviour::new(local_peer_id, relay::Config::default()),
                    relay_client,
                    dcutr: dcutr::Behaviour::new(local_peer_id),
                    upnp: upnp::tokio::Behaviour::default(),
                    kad,
                    sync: {
                        let (p, c) = rr(sync_proto.clone());
                        request_response::cbor::Behaviour::new(p, c)
                    },
                    msg: {
                        let (p, c) = rr(msg_proto.clone());
                        request_response::cbor::Behaviour::new(p, c)
                    },
                    blob: {
                        let (p, c) = rr(blob_proto.clone());
                        request_response::cbor::Behaviour::new(p, c)
                    },
                })
            })?
            .with_swarm_config(|c| c.with_idle_connection_timeout(Duration::from_secs(120)))
            .build();

        let topic = gossipsub::IdentTopic::new(format!("timecoin/{}/tx/v2", config.network));
        let wallet = wallet_address(&keypair.public().to_peer_id());
        let used_payments = load_used_payments(&config.data_dir);
        let mut network = Self {
            swarm,
            dag,
            peers,
            inbox: Arc::new(Mutex::new(Vec::new())),
            config,
            wallet,
            commands,
            topic: topic.clone(),
            relay: None,
            relay_reserved: false,
            pending_msgs: HashMap::new(),
            pending_blobs: HashMap::new(),
            used_payments,
        };
        network
            .swarm
            .behaviour_mut()
            .gossipsub
            .subscribe(&topic)
            .map_err(|e| anyhow!("subscribe: {e}"))?;
        Ok(network)
    }

    pub fn listen(&mut self, port: u16) -> Result<()> {
        self.swarm
            .listen_on(format!("/ip4/0.0.0.0/tcp/{port}").parse()?)?;
        self.swarm
            .listen_on(format!("/ip4/0.0.0.0/udp/{port}/quic-v1").parse()?)?;
        Ok(())
    }

    /// Dial bootstrap peers and, if a relay is given, dial it too. The circuit
    /// reservation is requested once the relay connection is established (a
    /// concurrent listen-and-dial to the same peer aborts the reservation).
    pub fn bootstrap(&mut self, bootstrap: &[Multiaddr], relay: Option<&Multiaddr>) -> Result<()> {
        if let Some(relay_addr) = relay {
            let peer_id = relay_addr
                .iter()
                .find_map(|p| match p {
                    libp2p::multiaddr::Protocol::P2p(peer) => Some(peer),
                    _ => None,
                })
                .ok_or_else(|| {
                    anyhow!("--relay address must end in /p2p/<relay-peer-id>: {relay_addr}")
                })?;
            self.relay = Some((peer_id, relay_addr.clone()));
        }
        for addr in bootstrap.iter().chain(relay) {
            match self.swarm.dial(addr.clone()) {
                Ok(()) => info!("dialing peer {addr}"),
                Err(e) => warn!("failed to dial {addr}: {e}"),
            }
        }
        Ok(())
    }

    /// Once connected to the configured relay, ask it for a reservation so we
    /// stay reachable at `<relay>/p2p-circuit/p2p/<us>`.
    fn maybe_reserve_relay(&mut self, connected_peer: PeerId) {
        if self.relay_reserved {
            return;
        }
        let Some((relay_peer, relay_addr)) = &self.relay else { return };
        if *relay_peer != connected_peer {
            return;
        }
        let circuit = relay_addr
            .clone()
            .with(libp2p::multiaddr::Protocol::P2pCircuit);
        match self.swarm.listen_on(circuit) {
            Ok(_) => {
                info!("requesting relay reservation via {relay_addr}");
                self.relay_reserved = true;
            }
            Err(e) => warn!("failed to listen via relay {relay_addr}: {e}"),
        }
    }

    pub async fn run(mut self) {
        // Refresh DHT routing periodically so the mesh heals as peers churn.
        let mut kad_refresh = tokio::time::interval(Duration::from_secs(300));
        kad_refresh.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        loop {
            tokio::select! {
                command = self.commands.recv() => match command {
                    Some(c) => self.handle_command(c),
                    None => return,
                },
                event = self.swarm.select_next_some() => self.handle_event(event),
                _ = kad_refresh.tick() => {
                    if self.swarm.behaviour_mut().kad.bootstrap().is_err() {
                        debug!("kad bootstrap skipped: no known peers yet");
                    }
                }
            }
        }
    }

    fn handle_command(&mut self, command: Command) {
        match command {
            Command::PublishTx(tx) => self.publish_tx(&tx),
            Command::Dial(addr, reply) => {
                let result = self.swarm.dial(addr).map_err(|e| anyhow!(e));
                let _ = reply.send(result);
            }
            Command::ListenAddrs(reply) => {
                let addrs = self.swarm.listeners().cloned().collect();
                let _ = reply.send(addrs);
            }
            Command::SendMessage(peer, text, reply) => {
                let id = self
                    .swarm
                    .behaviour_mut()
                    .msg
                    .send_request(&peer, MsgRequest { text });
                self.pending_msgs.insert(id, reply);
            }
            Command::Blob(peer, request, reply) => {
                let id = self.swarm.behaviour_mut().blob.send_request(&peer, request);
                self.pending_blobs.insert(id, reply);
            }
        }
    }

    fn publish_tx(&mut self, tx: &Transaction) {
        let data = match serde_json::to_vec(tx) {
            Ok(d) => d,
            Err(e) => {
                warn!("failed to serialize transaction {}: {e}", tx.id);
                return;
            }
        };
        match self.swarm.behaviour_mut().gossipsub.publish(self.topic.clone(), data) {
            Ok(_) => debug!("published transaction {}", tx.id),
            // InsufficientPeers just means we're alone; the sync protocol
            // delivers it when someone connects.
            Err(e) => debug!("gossip publish for {}: {e}", tx.id),
        }
    }

    /// Import transactions from the network; returns ids of missing parents
    /// (orphans we should request from the source peer).
    fn import_txs(&mut self, txs: Vec<Transaction>, source: &str) -> Vec<String> {
        let mut dag = self.dag.lock().unwrap();
        for tx in txs {
            let id = tx.id.clone();
            match dag.insert(tx) {
                Ok(accepted) if !accepted.is_empty() => {
                    info!(
                        "accepted {} transaction(s) from {source} (dag size {})",
                        accepted.len(),
                        dag.len()
                    );
                }
                Ok(_) => debug!("transaction {id} parked or already known"),
                Err(e) => warn!("rejected transaction {id} from {source}: {e}"),
            }
        }
        dag.missing_parents()
    }

    fn request_missing(&mut self, peer: PeerId, missing: Vec<String>) {
        if !missing.is_empty() {
            self.swarm
                .behaviour_mut()
                .sync
                .send_request(&peer, SyncRequest::Get(missing));
        }
    }

    fn handle_sync_request(
        &mut self,
        request: SyncRequest,
        channel: ResponseChannel<SyncResponse>,
    ) {
        let response = {
            let dag = self.dag.lock().unwrap();
            match request {
                SyncRequest::Tips => SyncResponse::Txs(dag.with_ancestry(&dag.tips(), SYNC_BATCH)),
                SyncRequest::Get(ids) => SyncResponse::Txs(dag.with_ancestry(&ids, SYNC_BATCH)),
            }
        };
        if self
            .swarm
            .behaviour_mut()
            .sync
            .send_response(channel, response)
            .is_err()
        {
            debug!("sync peer hung up before response was sent");
        }
    }

    // -- blob provider side -------------------------------------------------

    fn handle_blob_request(&mut self, peer: PeerId, request: BlobRequest) -> BlobResponse {
        match request {
            BlobRequest::Quote { size } => {
                if size > self.config.blob_max_bytes {
                    return BlobResponse::Refused {
                        reason: format!(
                            "blob too large: {size} > {} bytes",
                            self.config.blob_max_bytes
                        ),
                    };
                }
                // Refuse untrusted requesters at quote time, before they
                // burn a payment that Put would reject anyway.
                if self.config.blob_trust_depth > 0 {
                    let requester = wallet_address(&peer);
                    let trusted = self
                        .dag
                        .lock()
                        .unwrap()
                        .trusted_set(&self.wallet, self.config.blob_trust_depth);
                    if !trusted.contains_key(&requester) {
                        return BlobResponse::Refused {
                            reason: format!(
                                "you are outside this node's trust neighborhood (depth {}) — ask the operator to vouch for you",
                                self.config.blob_trust_depth
                            ),
                        };
                    }
                }
                BlobResponse::Price { price: self.config.quote(size) }
            }
            BlobRequest::Put { data, payment } => self.handle_blob_put(peer, data, payment),
            BlobRequest::Get { hash } => {
                if !valid_hash(&hash) {
                    return BlobResponse::Refused { reason: "invalid hash".into() };
                }
                match std::fs::read(self.blob_dir().join(&hash)) {
                    Ok(data) => BlobResponse::Blob { data },
                    Err(_) => BlobResponse::Refused { reason: "not found".into() },
                }
            }
            BlobRequest::Range { hash, offset, len } => {
                if !valid_hash(&hash) || len > 4096 {
                    return BlobResponse::Refused { reason: "invalid range request".into() };
                }
                match std::fs::read(self.blob_dir().join(&hash)) {
                    Ok(data) => {
                        let start = offset as usize;
                        let end = start.saturating_add(len as usize);
                        if end > data.len() {
                            BlobResponse::Refused { reason: "range out of bounds".into() }
                        } else {
                            BlobResponse::RangeData { data: data[start..end].to_vec() }
                        }
                    }
                    Err(_) => BlobResponse::Refused { reason: "not found".into() },
                }
            }
        }
    }

    fn handle_blob_put(&mut self, peer: PeerId, data: Vec<u8>, payment: String) -> BlobResponse {
        let size = data.len() as u64;
        if size > self.config.blob_max_bytes {
            return BlobResponse::Refused {
                reason: format!("blob too large: {size} > {} bytes", self.config.blob_max_bytes),
            };
        }
        let price = self.config.quote(size);
        if price > 0 {
            if let Err(reason) = self.verify_payment(&payment, price) {
                return BlobResponse::Refused { reason };
            }
        }
        let hash = hex::encode(Sha256::digest(&data));
        let dir = self.blob_dir();
        if let Err(e) = std::fs::create_dir_all(&dir).and_then(|_| std::fs::write(dir.join(&hash), &data)) {
            warn!("failed to store blob {hash}: {e}");
            return BlobResponse::Refused { reason: "storage failure".into() };
        }
        if price > 0 {
            self.used_payments.insert(payment.clone());
            persist_used_payment(&self.config.data_dir, &payment, &hash);
        }
        info!(
            "stored {size}-byte blob {hash} for {peer} (paid {price} TC via {})",
            if price > 0 { payment.as_str() } else { "free tier" }
        );
        BlobResponse::Stored { hash }
    }

    /// A payment is good if it's a transfer to our wallet, of at least
    /// `price`, that the deterministic ledger fold actually applied (so a
    /// double-spend loser can't buy storage), and not already redeemed.
    fn verify_payment(&self, payment: &str, price: u64) -> std::result::Result<(), String> {
        if self.used_payments.contains(payment) {
            return Err("payment already redeemed".into());
        }
        let dag = self.dag.lock().unwrap();
        let Some(tx) = dag.get(payment) else {
            return Err("payment not found (may still be propagating — retry)".into());
        };
        if tx.kind != TxKind::Transfer || tx.receiver != self.wallet {
            return Err("payment is not a transfer to this node's wallet".into());
        }
        if tx.amount < price {
            return Err(format!("payment {} < price {price}", tx.amount));
        }
        // Trust gate: the payer must be inside our vouch neighborhood, and
        // the payment must be funded when counting only trusted mints —
        // coin rigged up outside the neighborhood buys nothing here.
        let (ledger, gated) = if self.config.blob_trust_depth > 0 {
            let trusted = dag.trusted_set(&self.wallet, self.config.blob_trust_depth);
            if !trusted.contains_key(&tx.sender) {
                return Err(format!(
                    "payer is outside this node's trust neighborhood (depth {}) — ask the operator to vouch for you",
                    self.config.blob_trust_depth
                ));
            }
            (dag.ledger_view(Some(&trusted), 0), true)
        } else {
            (dag.ledger(), false)
        };
        if !ledger.applied_transfers.contains(payment) {
            return Err(if gated {
                "payment not applied in this node's trusted ledger view — retry, or the coins aren't trusted here".into()
            } else {
                "payment not (yet) applied by the ledger — retry".into()
            });
        }
        Ok(())
    }

    fn blob_dir(&self) -> PathBuf {
        self.config.data_dir.join("blobs")
    }

    // -- event loop ----------------------------------------------------------

    fn handle_event(&mut self, event: SwarmEvent<BehaviourEvent>) {
        match event {
            SwarmEvent::NewListenAddr { address, .. } => {
                let peer_id = *self.swarm.local_peer_id();
                info!("listening on {address}/p2p/{peer_id}");
            }
            SwarmEvent::ConnectionEstablished { peer_id, endpoint, .. } => {
                info!("connected to {peer_id} via {}", endpoint.get_remote_address());
                self.peers.lock().unwrap().insert(peer_id);
                self.swarm
                    .behaviour_mut()
                    .gossipsub
                    .add_explicit_peer(&peer_id);
                self.maybe_reserve_relay(peer_id);
                // Anti-entropy: compare DAGs with every newly connected peer.
                self.swarm
                    .behaviour_mut()
                    .sync
                    .send_request(&peer_id, SyncRequest::Tips);
                // Learn this peer's neighbors so we can reach peers-of-peers.
                let _ = self.swarm.behaviour_mut().kad.bootstrap();
            }
            SwarmEvent::ConnectionClosed { peer_id, num_established, .. } => {
                if num_established == 0 {
                    info!("disconnected from {peer_id}");
                    self.peers.lock().unwrap().remove(&peer_id);
                }
            }
            SwarmEvent::Behaviour(BehaviourEvent::Mdns(mdns::Event::Discovered(list))) => {
                for (peer_id, addr) in list {
                    debug!("mDNS discovered {peer_id} at {addr}");
                    self.swarm.behaviour_mut().kad.add_address(&peer_id, addr.clone());
                    if let Err(e) = self.swarm.dial(addr.clone()) {
                        debug!("mDNS dial {addr}: {e}");
                    }
                }
            }
            SwarmEvent::Behaviour(BehaviourEvent::Kad(kad::Event::RoutingUpdated {
                peer, ..
            })) => {
                debug!("kad routing table gained {peer}");
            }
            SwarmEvent::Behaviour(BehaviourEvent::Kad(event)) => {
                debug!("kad: {event:?}");
            }
            SwarmEvent::Behaviour(BehaviourEvent::Gossipsub(gossipsub::Event::Message {
                propagation_source,
                message,
                ..
            })) => match serde_json::from_slice::<Transaction>(&message.data) {
                Ok(tx) => {
                    let missing = self.import_txs(vec![tx], &propagation_source.to_string());
                    self.request_missing(propagation_source, missing);
                }
                Err(e) => warn!("undecodable gossip message from {propagation_source}: {e}"),
            },
            SwarmEvent::Behaviour(BehaviourEvent::Sync(request_response::Event::Message {
                peer,
                message,
                ..
            })) => match message {
                request_response::Message::Request { request, channel, .. } => {
                    self.handle_sync_request(request, channel);
                }
                request_response::Message::Response { response, .. } => {
                    let SyncResponse::Txs(txs) = response;
                    let missing = self.import_txs(txs, &peer.to_string());
                    self.request_missing(peer, missing);
                }
            },
            SwarmEvent::Behaviour(BehaviourEvent::Msg(event)) => self.handle_msg_event(event),
            SwarmEvent::Behaviour(BehaviourEvent::Blob(event)) => self.handle_blob_event(event),
            SwarmEvent::Behaviour(BehaviourEvent::Identify(identify::Event::Received {
                peer_id,
                info,
                ..
            })) => {
                debug!("identify from {peer_id}: {}", info.protocol_version);
                // Remember the peer's addresses (swarm book + DHT routing
                // table) so direct requests can dial it later and other
                // peers can discover it through us.
                for addr in info.listen_addrs {
                    self.swarm.add_peer_address(peer_id, addr.clone());
                    self.swarm.behaviour_mut().kad.add_address(&peer_id, addr);
                }
            }
            SwarmEvent::Behaviour(BehaviourEvent::Autonat(event)) => {
                debug!("autonat: {event:?}");
            }
            SwarmEvent::Behaviour(BehaviourEvent::Upnp(event)) => match event {
                upnp::Event::NewExternalAddr(addr) => info!("UPnP external address: {addr}"),
                upnp::Event::GatewayNotFound => {
                    info!("UPnP: no gateway found (fine on cloud/docker networks)")
                }
                other => debug!("upnp: {other:?}"),
            },
            SwarmEvent::Behaviour(BehaviourEvent::Dcutr(event)) => {
                info!("dcutr hole punching: {event:?}");
            }
            SwarmEvent::Behaviour(BehaviourEvent::RelayClient(event)) => {
                info!("relay client: {event:?}");
            }
            other => debug!("swarm event: {other:?}"),
        }
    }

    fn handle_msg_event(&mut self, event: request_response::Event<MsgRequest, MsgResponse>) {
        match event {
            request_response::Event::Message { peer, message, .. } => match message {
                request_response::Message::Request { request, channel, .. } => {
                    let response = {
                        let mut inbox = self.inbox.lock().unwrap();
                        if inbox.len() >= INBOX_CAP {
                            MsgResponse::Refused { reason: "inbox full".into() }
                        } else {
                            inbox.push(InboxMessage {
                                from: peer.to_string(),
                                from_wallet: wallet_address(&peer),
                                text: request.text,
                                received_at: now(),
                            });
                            MsgResponse::Ack
                        }
                    };
                    let _ = self.swarm.behaviour_mut().msg.send_response(channel, response);
                }
                request_response::Message::Response { request_id, response } => {
                    if let Some(reply) = self.pending_msgs.remove(&request_id) {
                        let _ = reply.send(match response {
                            MsgResponse::Ack => Ok(()),
                            MsgResponse::Refused { reason } => {
                                Err(anyhow!("peer refused message: {reason}"))
                            }
                        });
                    }
                }
            },
            request_response::Event::OutboundFailure { request_id, error, .. } => {
                if let Some(reply) = self.pending_msgs.remove(&request_id) {
                    let _ = reply.send(Err(anyhow!("delivery failed: {error}")));
                }
            }
            _ => {}
        }
    }

    fn handle_blob_event(&mut self, event: request_response::Event<BlobRequest, BlobResponse>) {
        match event {
            request_response::Event::Message { peer, message, .. } => match message {
                request_response::Message::Request { request, channel, .. } => {
                    let response = self.handle_blob_request(peer, request);
                    let _ = self.swarm.behaviour_mut().blob.send_response(channel, response);
                }
                request_response::Message::Response { request_id, response } => {
                    if let Some(reply) = self.pending_blobs.remove(&request_id) {
                        let _ = reply.send(Ok(response));
                    }
                }
            },
            request_response::Event::OutboundFailure { request_id, error, .. } => {
                if let Some(reply) = self.pending_blobs.remove(&request_id) {
                    let _ = reply.send(Err(anyhow!("blob request failed: {error}")));
                }
            }
            _ => {}
        }
    }
}

fn valid_hash(hash: &str) -> bool {
    hash.len() == 64 && hash.chars().all(|c| c.is_ascii_hexdigit())
}

fn now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

fn payments_path(data_dir: &std::path::Path) -> PathBuf {
    data_dir.join("blob_payments.jsonl")
}

fn load_used_payments(data_dir: &std::path::Path) -> HashSet<String> {
    let mut used = HashSet::new();
    if let Ok(data) = std::fs::read_to_string(payments_path(data_dir)) {
        for line in data.lines().filter(|l| !l.trim().is_empty()) {
            if let Ok(v) = serde_json::from_str::<serde_json::Value>(line) {
                if let Some(p) = v["payment"].as_str() {
                    used.insert(p.to_string());
                }
            }
        }
    }
    used
}

fn persist_used_payment(data_dir: &std::path::Path, payment: &str, hash: &str) {
    let line = serde_json::json!({ "payment": payment, "hash": hash });
    let result = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(payments_path(data_dir))
        .and_then(|mut f| writeln!(f, "{line}"));
    if let Err(e) = result {
        warn!("failed to persist redeemed payment {payment}: {e}");
    }
}
