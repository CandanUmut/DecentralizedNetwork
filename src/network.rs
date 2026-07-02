//! The libp2p layer, entirely on upstream released crates (no fork).
//!
//! What the legacy fork-era code did and where it went:
//! - custom `/messaging/1.0.0` request/response  -> `/timecoin/sync/1.0.0`
//!   (request_response with a CBOR codec, used for DAG anti-entropy)
//! - hand-rolled keep-alive ConnectionHandler    -> `with_idle_connection_timeout`
//! - hand-assembled TCP/WS/QUIC OrTransport      -> `SwarmBuilder`
//! - STUN/TURN/UPnP/WebRTC side stack            -> autonat + relay + dcutr + upnp
//! - public IPFS DHT bootnodes                   -> explicit `--bootstrap` peers + mDNS

use crate::dag::{Dag, Transaction};
use anyhow::{anyhow, Result};
use futures::StreamExt;
use libp2p::{
    autonat, dcutr, gossipsub, identify, mdns, noise, ping, relay,
    request_response::{self, ProtocolSupport, ResponseChannel},
    swarm::{NetworkBehaviour, SwarmEvent},
    upnp, yamux, Multiaddr, PeerId, StreamProtocol, Swarm,
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::HashSet;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::sync::{mpsc, oneshot};
use tracing::{debug, info, warn};

pub const TX_TOPIC: &str = "timecoin-tx-v1";

/// DAG anti-entropy protocol: ask a peer for its transaction ids, then fetch
/// the ones we don't have. Also used to pull missing parents of orphans.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum SyncRequest {
    Ids,
    Get(Vec<String>),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum SyncResponse {
    Ids(Vec<String>),
    Txs(Vec<Transaction>),
}

#[derive(NetworkBehaviour)]
pub struct Behaviour {
    gossipsub: gossipsub::Behaviour,
    mdns: mdns::tokio::Behaviour,
    identify: identify::Behaviour,
    ping: ping::Behaviour,
    autonat: autonat::Behaviour,
    relay_server: relay::Behaviour,
    relay_client: relay::client::Behaviour,
    dcutr: dcutr::Behaviour,
    upnp: upnp::tokio::Behaviour,
    sync: request_response::cbor::Behaviour<SyncRequest, SyncResponse>,
}

/// Commands from the API/CLI into the network task (the swarm is owned by a
/// single task; the legacy Arc<Mutex<Swarm>> pattern deadlocked by design).
pub enum Command {
    PublishTx(Transaction),
    Dial(Multiaddr, oneshot::Sender<Result<()>>),
    ListenAddrs(oneshot::Sender<Vec<Multiaddr>>),
}

pub struct Network {
    pub swarm: Swarm<Behaviour>,
    pub dag: Arc<Mutex<Dag>>,
    pub peers: Arc<Mutex<HashSet<PeerId>>>,
    commands: mpsc::Receiver<Command>,
    topic: gossipsub::IdentTopic,
}

impl Network {
    pub fn new(
        keypair: &libp2p::identity::Keypair,
        dag: Arc<Mutex<Dag>>,
        peers: Arc<Mutex<HashSet<PeerId>>>,
        commands: mpsc::Receiver<Command>,
    ) -> Result<Self> {
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

                let mdns =
                    mdns::tokio::Behaviour::new(mdns::Config::default(), local_peer_id)?;

                let identify = identify::Behaviour::new(identify::Config::new(
                    "/timecoin/1.0.0".to_string(),
                    key.public(),
                ));

                let sync = request_response::cbor::Behaviour::new(
                    [(
                        StreamProtocol::new("/timecoin/sync/1.0.0"),
                        ProtocolSupport::Full,
                    )],
                    request_response::Config::default(),
                );

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
                    sync,
                })
            })?
            .with_swarm_config(|c| c.with_idle_connection_timeout(Duration::from_secs(120)))
            .build();

        let topic = gossipsub::IdentTopic::new(TX_TOPIC);
        let mut network = Self {
            swarm,
            dag,
            peers,
            commands,
            topic: topic.clone(),
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

    /// Dial bootstrap peers and, if a relay is given, reserve a slot on it so
    /// NATed peers stay reachable via `<relay>/p2p-circuit/p2p/<us>`.
    pub fn bootstrap(&mut self, bootstrap: &[Multiaddr], relay: Option<&Multiaddr>) {
        for addr in bootstrap {
            match self.swarm.dial(addr.clone()) {
                Ok(()) => info!("dialing bootstrap peer {addr}"),
                Err(e) => warn!("failed to dial bootstrap peer {addr}: {e}"),
            }
        }
        if let Some(relay_addr) = relay {
            let circuit = relay_addr
                .clone()
                .with(libp2p::multiaddr::Protocol::P2pCircuit);
            match self.swarm.listen_on(circuit.clone()) {
                Ok(_) => info!("requesting relay reservation via {relay_addr}"),
                Err(e) => warn!("failed to listen via relay {relay_addr}: {e}"),
            }
        }
    }

    pub async fn run(mut self) {
        loop {
            tokio::select! {
                command = self.commands.recv() => match command {
                    Some(c) => self.handle_command(c),
                    None => return,
                },
                event = self.swarm.select_next_some() => self.handle_event(event),
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
                SyncRequest::Ids => SyncResponse::Ids(dag.all_ids()),
                SyncRequest::Get(ids) => SyncResponse::Txs(
                    ids.iter().filter_map(|id| dag.get(id).cloned()).collect(),
                ),
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
                // Anti-entropy: compare DAGs with every newly connected peer.
                self.swarm
                    .behaviour_mut()
                    .sync
                    .send_request(&peer_id, SyncRequest::Ids);
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
                    if let Err(e) = self.swarm.dial(addr.clone()) {
                        debug!("mDNS dial {addr}: {e}");
                    }
                }
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
                request_response::Message::Response { response, .. } => match response {
                    SyncResponse::Ids(ids) => {
                        let unknown: Vec<String> = {
                            let dag = self.dag.lock().unwrap();
                            ids.into_iter().filter(|id| !dag.contains(id)).collect()
                        };
                        self.request_missing(peer, unknown);
                    }
                    SyncResponse::Txs(txs) => {
                        let missing = self.import_txs(txs, &peer.to_string());
                        self.request_missing(peer, missing);
                    }
                },
            },
            SwarmEvent::Behaviour(BehaviourEvent::Identify(identify::Event::Received {
                peer_id,
                info,
                ..
            })) => {
                debug!("identify from {peer_id}: {}", info.protocol_version);
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
}
