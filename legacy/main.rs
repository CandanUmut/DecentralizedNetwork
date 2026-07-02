// main.rs

#![doc = include_str!("../README.md")]
mod messaging; // This should already be in place
mod webrtc;
mod myipfs;
mod routes;
mod timecoin; // Load the timecoin module
mod messagingroutes;
use axum::handler::Handler;
use messagingroutes::{send_message};
use bs58; // Add this to your imports
use timecoin::peer_id::IPFSPeerId;
use timecoin::peer_manager::PeerManager;
use timecoin::utils::derive_wallet_address;
use axum::{routing::get, routing::post, Router};
use routes::{root, add_file, fetch_content, pin_file, add_folder, unpin_file, add_zip,  receive_messages, connect_peer, get_peers, send_timecoin, reward_timecoin, get_balance};
use messaging::{create_messaging_behavior, handle_request_response_event};
use messaging::Messaging;
use libp2p::request_response::{Behaviour as RequestResponseBehaviour};
use std::{
    num::NonZeroUsize,
    time::{Duration, Instant},
};
use libp2p::swarm::dial_opts::DialOpts;
use libp2p::swarm::{
    handler::ConnectionHandlerEvent, 
    SubstreamProtocol,
};
use libp2p::swarm::handler::ConnectionEvent;
use libp2p::swarm::Config as SwarmConfig;

use libp2p::core::upgrade::DeniedUpgrade;
use ipfs::{Ipfs, MultiaddrWithoutPeerId, MultiaddrWithPeerId, PeerId as IpfsPeerId};
use libp2p::tls::Config as TlsConfig;
use libp2p::websocket::WsConfig;
use myipfs::IPFSHandler;
use tokio::time;
use tokio::sync::{Mutex, mpsc};
use webrtc::{WebRTCHandler, WebRTCEvent};
use turn::auth::*;
use turn::client::{Client as TurnClient, ClientConfig as TurnClientConfig};
use turn::Error as TurnError;
use tokio::net::{TcpStream, UdpSocket};
use util::Conn;
use std::sync::Arc;
use std::net::SocketAddr;
use anyhow::{anyhow, Error};
use stun_client::*;
use tokio::task; // Replace async_std with tokio for compatibility.
use igd::{search_gateway, PortMappingProtocol};
use std::net::{IpAddr, Ipv4Addr};
use libp2p::quic::{tokio::Transport as QuicTransport, Config as QuicConfig};
use libp2p::autonat::v1::OutboundProbeEvent;
use libp2p::autonat::{Behaviour as AutoNatBehaviour, Config as AutoNatConfig, Event as AutoNatEvent};
use libp2p::autonat::NatStatus;
use libp2p::relay::{Behaviour as RelayBehaviour, Config as RelayConfig};
use libp2p::request_response::{Event as RequestResponseEvent, Message as RequestResponseMessage};
use tokio::time::interval;
use libp2p::multiaddr::Protocol;
use libp2p::tcp::tokio::Transport as TokioTcpTransport;
use libp2p::core::{upgrade, transport::Boxed, Transport};
use libp2p::SwarmBuilder;
pub use libp2p_swarm_derive::NetworkBehaviour;
use anyhow::{bail, Result};
use clap::Parser;
use futures::StreamExt;
use libp2p::{
    tcp,
    bytes::BufMut,
    kad::{self, Behaviour, Config, Event, QueryResult, Record, RecordKey as Key},
    mdns::{tokio::Behaviour as Mdns, Event as MdnsEvent},
    dns::tokio::Transport as DnsTransport,
    swarm::{Swarm, SwarmEvent},
    tcp::Config as TcpConfig,
    tcp::Transport as TcpTransport,
    yamux::Config as YamuxConfig,
    Multiaddr,
    
};
use libp2p::ping::{Behaviour as Ping, Config as PingConfig};
use tracing::debug;
use tracing::error;
use libp2p::yamux;
use libp2p::{
    noise,
    identity,
    noise::{Config as NoiseConfig},
    PeerId,
};
use libp2p::swarm::{Executor, ConnectionHandler};
use tracing::{info, warn};
use tracing_subscriber::EnvFilter;
use crate::myipfs::Metadata;


// Define constants
const BOOTNODES: [&str; 18] = [
    "/dnsaddr/sg1.bootstrap.libp2p.io/p2p/QmcZf59bWwK5XFi76CZX8cbJ4BhTzzA3gU1ZjYZcYW3dwt",
    "/dnsaddr/sv15.bootstrap.libp2p.io/p2p/QmNnooDu7bfjPFoTZYxMNLWUQJyrVwtbZg5gBMjTezGAJN",
    "/ip4/149.28.123.35/udp/4001/quic/p2p/12D3KooWKMqYrF5m6YRmhWxyrTi4JCK1TwmqsFQXKXByDvPyGr4W",
    "/ip4/149.28.52.17/tcp/4001/p2p/12D3KooWB12Fb9gwNchjsajP1pqfYD4m27kSi4XfyDH3XWzjkekE",
    "/ip4/149.28.81.65/udp/4001/quic-v1/p2p/12D3KooWPfBPAAu3GxbMdQzUrLmba3soYqNhAiqNA91GCdfF7t8s",
    "/ip4/149.28.98.59/udp/4001/quic/p2p/12D3KooWRmLkuBQAciuvNzgz7Symz5xDuPn4vjjSbivFsNfgxWAC,",
    "/ip4/152.53.65.116/udp/4001/quic-v1/p2p/12D3KooWT3Ye1Fo2bXoA4FrTc9B8TTQjw21RdTHLcT8NvZUjifXc",
    "/ip4/155.138.200.27/udp/4001/quic-v1/p2p/12D3KooW9xKqkV7NpsTN6jfQisUgq8n6anAFWPbBxfS3MPQ7XZFk",
    "/ip4/158.247.196.95/udp/4001/quic-v1/p2p/12D3KooWHshKWe6wTe5SRNDRanQbxdzxnU7drKcPs9UYnRLGSAAp",
    "/ip4/174.136.97.180/udp/4001/quic-v1/p2p/12D3KooWPEDBmt7vm6FNNYuqaA4n2qMUZ6wPK5NcRc8t6KpqgRkV",
    "/ip4/193.201.15.32/udp/4001/quic-v1/p2p/12D3KooWLhqzMebTYX5ae1A2A1Ti4KPbYyvmWQje261yFmDKjDac",
    "/ip4/193.8.130.177/udp/4001/quic-v1/p2p/12D3KooWJt8avPjhXXnAjNwPTR8TNKgBmQmgUmLGJzjnEqpMRQc9",
    "/ip4/207.246.105.66/udp/4001/quic-v1/p2p/12D3KooWKsj1dLgxUY6QqgE313CzNnDSq4XMXfF27ks4JPpeMsB1",
    "/ip4/23.94.100.84/udp/4001/quic-v1/p2p/12D3KooWLJDbyYWq8nuLiKmbvvSxpVDTAz5VBEgoLV4nHFhXMchP",
    "/ip4/38.242.237.4/udp/4001/quic-v1/p2p/12D3KooWRLr5pLMt6A4GGEGN9m5t4kr2njx7coZZaNEgWQrj4P7F",
    "/ip4/51.210.156.186/udp/4001/quic/p2p/12D3KooWEUPHXKj5uZ2MKH8fyF2SRyum3gx2VkvXvmNyhgYBqzgz",
    "/ip4/62.210.172.22/udp/4001/quic/p2p/12D3KooWRVUNn5oZJ517WiA62pGpSdYLbSBLbXMJpYmJDQAumczx",
    "/ip4/65.108.41.22/udp/4001/quic/p2p/12D3KooWKXeG6Ft3HC7qM1EgiDaiC96kYGy1AoohVVnqQMveEtbF",
   
];
const ROUTING_REFRESH_INTERVAL: Duration = Duration::from_secs(60);


const IPFS_PROTO_NAME: libp2p::swarm::StreamProtocol = libp2p::swarm::StreamProtocol::new("/ipfs/kad/1.0.0");











// Define IPFSEvent
#[derive(Debug, Clone)]
pub enum IPFSEvent {
    AddFile {
        path: String,
        metadata: Option<Metadata>,
    },
    DiscoverContent {
        cid: String,
    },
    FetchContent {
        cid: String,
        include_metadata: bool,
    },
}



fn libp2p_to_ipfs_multiaddr_without_peer_id(
    multiaddr: libp2p::Multiaddr,
) -> Result<ipfs::MultiaddrWithoutPeerId, anyhow::Error> {
    // Convert libp2p::Multiaddr to ipfs::Multiaddr manually
    let mut ipfs_multiaddr = ipfs::Multiaddr::empty();

    for component in multiaddr.iter() {
        match component {
            libp2p::multiaddr::Protocol::P2p(_) => {
                // Skip PeerId component as it will be handled separately
            }
            other => {
                let ipfs_protocol = match other {
                    libp2p::multiaddr::Protocol::Ip4(addr) => ipfs::Protocol::Ip4(addr),
                    libp2p::multiaddr::Protocol::Ip6(addr) => ipfs::Protocol::Ip6(addr),
                    libp2p::multiaddr::Protocol::Tcp(port) => ipfs::Protocol::Tcp(port),
                    libp2p::multiaddr::Protocol::Udp(port) => ipfs::Protocol::Udp(port),
                    libp2p::multiaddr::Protocol::Quic => ipfs::Protocol::Quic,
                    libp2p::multiaddr::Protocol::Dns(domain) => ipfs::Protocol::Dns(domain),
                    libp2p::multiaddr::Protocol::Dns4(domain) => ipfs::Protocol::Dns4(domain),
                    libp2p::multiaddr::Protocol::Dns6(domain) => ipfs::Protocol::Dns6(domain),
                    libp2p::multiaddr::Protocol::Ws(domain) => ipfs::Protocol::Ws(domain),
                    libp2p::multiaddr::Protocol::Wss(domain) => ipfs::Protocol::Wss(domain),
                    libp2p::multiaddr::Protocol::Http => ipfs::Protocol::Http,
                    libp2p::multiaddr::Protocol::Https => ipfs::Protocol::Https,
                    libp2p::multiaddr::Protocol::Memory(port) => ipfs::Protocol::Memory(port),
                    _ => {
                        return Err(anyhow::anyhow!(
                            "Unsupported protocol: {:?}",
                            other
                        ));
                    }
                };
                ipfs_multiaddr.push(ipfs_protocol);
            }
        }
    }

    // Use TryFrom to convert to MultiaddrWithoutPeerId
    ipfs_multiaddr
        .try_into()
        .map_err(|e| anyhow::anyhow!("Failed to convert Multiaddr: {:?}", e))
}

fn to_multiaddr_with_peer_id(
    libp2p_multiaddr: libp2p::Multiaddr,
    peer_id: ipfs::PeerId,
) -> Result<ipfs::MultiaddrWithPeerId, anyhow::Error> {
    // Convert libp2p::Multiaddr to ipfs::MultiaddrWithoutPeerId
    let multiaddr_without_peer_id = libp2p_to_ipfs_multiaddr_without_peer_id(libp2p_multiaddr)?;

    // Combine into MultiaddrWithPeerId
    Ok(ipfs::MultiaddrWithPeerId {
        multiaddr: multiaddr_without_peer_id,
        peer_id,
    })
}







#[derive(NetworkBehaviour)]
#[behaviour(out_event = "MyBehaviourEvent")]
pub struct MyBehaviour {
    kademlia: Behaviour<kad::store::MemoryStore>,
    mdns: Mdns,
    messaging: RequestResponseBehaviour<messaging::MessagingCodec>,
    relay: RelayBehaviour,
    autonat: AutoNatBehaviour, // Add AutoNAT
    ping: Ping, // Add Ping behaviour
    
}

/// Unified event type for `MyBehaviour`
#[derive(Debug)]
pub enum MyBehaviourEvent {
    Kademlia(kad::Event),
    Mdns(MdnsEvent),
    Messaging(libp2p::request_response::Event<Vec<u8>, Vec<u8>>),
    Relay(libp2p::relay::Event),
    AutoNat(AutoNatEvent), // Add AutoNAT Event
    Ping(libp2p::ping::Event), // Add PingEvent
    
}

impl From<kad::Event> for MyBehaviourEvent {
    fn from(event: kad::Event) -> Self {
        MyBehaviourEvent::Kademlia(event)
    }
}

impl From<MdnsEvent> for MyBehaviourEvent {
    fn from(event: MdnsEvent) -> Self {
        MyBehaviourEvent::Mdns(event)
    }
}
impl From<libp2p::request_response::Event<Vec<u8>, Vec<u8>>> for MyBehaviourEvent {
    fn from(event: libp2p::request_response::Event<Vec<u8>, Vec<u8>>) -> Self {
        MyBehaviourEvent::Messaging(event)
    }
}
impl From<libp2p::relay::Event> for MyBehaviourEvent {
    fn from(event: libp2p::relay::Event) -> Self {
        MyBehaviourEvent::Relay(event)
    }
}
impl From<AutoNatEvent> for MyBehaviourEvent {
    fn from(event: AutoNatEvent) -> Self {
        MyBehaviourEvent::AutoNat(event)
    }
}

impl From<libp2p::ping::Event> for MyBehaviourEvent {
    fn from(event: libp2p::ping::Event) -> Self {
        MyBehaviourEvent::Ping(event)
    }
}



impl MyBehaviour {
    fn new(local_peer_id: PeerId) -> Result<Self> {
        // Configure Kademlia
        let store = kad::store::MemoryStore::new(local_peer_id);
        let mut kademlia_config = Config::new(IPFS_PROTO_NAME.clone());
        kademlia_config.set_protocol_names(vec![IPFS_PROTO_NAME.clone()]);
        kademlia_config.set_query_timeout(Duration::from_secs(300)); // 5 minutes
        kademlia_config.disjoint_query_paths(true); // Enable better parallelism for queries
        let kademlia = Behaviour::with_config(local_peer_id, store, kademlia_config);

        // Configure mDNS
        let mdns = Mdns::new(Default::default(), local_peer_id)?;
        //Configure Messaging
        let messaging = create_messaging_behavior();
        //Configure Relay
        let relay = RelayBehaviour::new(local_peer_id, RelayConfig::default());
        //Configure Autonat
        let autonat = AutoNatBehaviour::new(local_peer_id, AutoNatConfig::default());
        let ping = Ping::new(PingConfig::new());

        Ok(Self { kademlia, mdns, messaging, relay, autonat ,ping})
    }

    /// Handles events emitted by the mDNS behaviour
    fn handle_mdns_event(&mut self, event: MdnsEvent) {
        match event {
            MdnsEvent::Discovered(list) => {
                info!("mDNS discovered event triggered!");
                for (peer, addr) in list {
                    info!("Discovered peer with mDNS {} at {}", peer, addr);
                    self.kademlia.add_address(&peer, addr);
                }
            }
            MdnsEvent::Expired(list) => {
                info!("mDNS expired event triggered!");
                for (peer, addr) in list {
                    info!("Expired peer {} at {}", peer, addr);
                    self.kademlia.remove_address(&peer, &addr);
                }
            }
        }
    }
    /// Handle dynamic peer addition
    fn handle_peer_addition(&mut self, peer_id: PeerId, addr: Multiaddr) {
        info!("Peer added: {} at {}", peer_id, addr);
        self.kademlia.add_address(&peer_id, addr);
    }

    /// Handle dynamic peer removal
    fn handle_peer_removal(&mut self, peer_id: PeerId) {
        info!("Peer removed: {}", peer_id);
        self.kademlia.remove_peer(&peer_id);
    }

    /// Refresh the Kademlia routing table
    fn refresh_routing_table(&mut self) {
        info!("Refreshing Kademlia routing table...");
        self.kademlia.bootstrap();
    }

}

struct KeepAliveHandler;

impl ConnectionHandler for KeepAliveHandler {
    type FromBehaviour = ();
    type ToBehaviour = ();
    type InboundProtocol = DeniedUpgrade;
    type OutboundProtocol = DeniedUpgrade;
    type InboundOpenInfo = ();
    type OutboundOpenInfo = ();

    fn connection_keep_alive(&self) -> bool {
        true // Enable keep-alive for this connection
    }

    fn poll(
        &mut self,
        _: &mut std::task::Context<'_>,
    ) -> std::task::Poll<
        libp2p::swarm::ConnectionHandlerEvent<Self::OutboundProtocol, Self::OutboundOpenInfo, Self::ToBehaviour>,
    > {
        std::task::Poll::Pending
    }

    fn listen_protocol(&self) -> SubstreamProtocol<Self::InboundProtocol, Self::InboundOpenInfo> {
        SubstreamProtocol::new(DeniedUpgrade, ())
    }

    fn on_behaviour_event(&mut self, _: Self::FromBehaviour) {}

    fn on_connection_event(
        &mut self,
        _: ConnectionEvent<
            Self::InboundProtocol,
            Self::OutboundProtocol,
            Self::InboundOpenInfo,
            Self::OutboundOpenInfo,
        >,
    ) {
    }
}


/// Helper function to parse peer_id
fn parse_peer_id(peer_id: &str) -> Result<libp2p::PeerId, String> {
    // Decode Base58
    match bs58::decode(peer_id).into_vec() {
        Ok(decoded_peer_id) => {
            // Attempt to create PeerId from decoded bytes
            libp2p::PeerId::from_bytes(&decoded_peer_id)
                .map_err(|e| format!("Failed to convert decoded peer_id: {:?}", e))
        }
        Err(e) => {
            // Log Base58 decoding error
            Err(format!("Invalid Base58 encoding for peer_id: {:?}", e))
        }
    }
}


async fn stun_binding() -> Result<(String, u16), Error> {
    // Create a new STUN client bound to a local ephemeral port.
    let mut client = Client::new("0.0.0.0:0", None).await?;
    
    // Send a binding request to a public STUN server.
    let res = client
        .binding_request("stun.l.google.com:19302", None)
        .await?;

    // Parse the response.
    let class = res.get_class();
    match class {
        Class::SuccessResponse => {
            if let Some(xor_mapped_addr) = Attribute::get_xor_mapped_address(&res) {
                println!(
                    "STUN public address: {}:{}",
                    xor_mapped_addr.ip(), xor_mapped_addr.port()
                );
                Ok((xor_mapped_addr.ip().to_string(), xor_mapped_addr.port()))
            } else {
                Err(anyhow!("Failed to parse XOR-MAPPED-ADDRESS"))
            }
        }
        _ => Err(anyhow!(format!("Failed STUN request. Class: {:?}", class))),
    }
}

/// Attempts to resolve the public IP and port using a TURN server.
///
/// # Returns
/// - `Ok((String, u16))` with the resolved public IP and port if successful.
/// - `Err(String)` if an error occurs during the TURN resolution.
async fn turn_binding() -> Result<(String, u16), String> {
    // Static TURN and STUN server details
    let turn_servers = vec![
        ("stun.relay.metered.ca", 80, "stun", None, None),
        ("global.relay.metered.ca", 80, "turn-tls", Some("01d654fea1e4da89242a001d"), Some("bMAoHpLTGkkgGjrv")),
        (
            "global.relay.metered.ca",
            80,
            "turn-tls",
            Some("01d654fea1e4da89242a001d"),
            Some("bMAoHpLTGkkgGjrv"),
        ),
        ("global.relay.metered.ca", 443, "turn-tls", Some("01d654fea1e4da89242a001d"), Some("bMAoHpLTGkkgGjrv")),
        (
            "global.relay.metered.ca",
            443,
            "turn-tls",
            Some("01d654fea1e4da89242a001d"),
            Some("bMAoHpLTGkkgGjrv"),
        ),
    ];

    let mut last_error = String::new();

    // Iterate through the static TURN servers
    for (host, port, protocol, username, credential) in turn_servers {
        let turn_serv_addr = format!("{}:{}", host, port);
        println!(
            "Attempting to connect to TURN server: {} ({})",
            turn_serv_addr,
            protocol
        );

        // Bind a UDP socket
        let socket = match tokio::net::UdpSocket::bind("0.0.0.0:0").await {
            Ok(sock) => sock,
            Err(e) => {
                last_error = format!("Failed to bind UDP socket: {:?}", e);
                println!("{}", last_error);
                continue;
            }
        };
        let conn = Arc::new(socket);

        // Create TURN client configuration for the current server
        let client_config = TurnClientConfig {
            stun_serv_addr: turn_serv_addr.clone(),
            turn_serv_addr: turn_serv_addr.clone(),
            username: username.unwrap_or("").to_string(),
            password: credential.unwrap_or("").to_string(),
            realm: host.to_string(),
            software: String::from("RustLibP2P"),
            rto_in_ms: 0,
            conn: conn.clone(),
            vnet: None,
        };

        // Initialize the TURN client
        match TurnClient::new(client_config).await {
            Ok(client) => {
                // Attempt to allocate a relay address
                match client.allocate().await {
                    Ok(relay_conn) => match relay_conn.local_addr() {
                        Ok(addr) => {
                            println!(
                                "Relay address allocated (UDP): {}:{}",
                                addr.ip(),
                                addr.port()
                            );
                            return Ok((addr.ip().to_string(), addr.port()));
                        }
                        Err(e) => {
                            let error_msg =
                                format!("Failed to retrieve relay address (UDP): {:?}", e);
                            println!("{}", error_msg);
                            last_error = error_msg;
                            continue;
                        }
                    },
                    Err(e) => {
                        let error_msg = format!(
                            "Failed to allocate relay connection for server {} (UDP): {:?}",
                            turn_serv_addr, e
                        );
                        println!("{}", error_msg);
                        last_error = error_msg;
                        continue;
                    }
                }
            }
            Err(e) => {
                let error_msg = format!(
                    "Failed to create TURN client for server {} (UDP): {:?}",
                    turn_serv_addr, e
                );
                println!("{}", error_msg);
                last_error = error_msg;
                continue;
            }
        }
    }

    Err(format!("All TURN servers failed. Last error: {}", last_error))
}


fn setup_upnp_port_mapping(external_port: u16, internal_port: u16) {
    use igd::SearchOptions;

    let options = SearchOptions {
        timeout: Some(std::time::Duration::from_secs(5)), // Correctly wrap Duration in Some
        ..Default::default()
    };

    match igd::search_gateway(options) {
        Ok(gateway) => {
            println!("Found gateway: {:?}", gateway);

            match gateway.get_external_ip() {
                Ok(external_ip) => {
                    println!("External IP address: {}", external_ip);

                    let internal_socket = std::net::SocketAddrV4::new("0.0.0.0".parse().unwrap(), internal_port);

                    match gateway.add_port(
                        igd::PortMappingProtocol::TCP,
                        external_port,
                        internal_socket,
                        0, // Lease duration (0 = infinite)
                        "Decentralized P2P Node",
                    ) {
                        Ok(()) => println!("Port mapping successful!"),
                        Err(err) => println!("Failed to add port mapping: {:?}", err),
                    }
                }
                Err(err) => println!("Failed to retrieve external IP address: {:?}", err),
            }
        }
        Err(err) => println!("No gateway found: {:?}", err),
    }
}


#[tokio::main]
async fn main() -> Result<()> {
// Inner async function containing all logic
    async fn new_function_doing_everything() -> anyhow::Result<()> {
        //Loggers ! 
        env_logger::init();

        // Example log messages to test
        log::info!("Logger initialized!");
        log::debug!("Starting the application...");
        log::warn!("This is a warning message.");
        let messaging = Messaging::new(100); // Buffer size of 100
        // Initialize logging with environment filter
        let peer_manager = Arc::new(Mutex::new(PeerManager::new()));
        {
            let mut manager = peer_manager.lock().await; // Acquire the lock
            manager.bootstrap_transactions();           // Call the method on the PeerManager instance
        }
        
        let _ = tracing_subscriber::fmt()
            .with_env_filter(EnvFilter::from_default_env())
            .try_init();

        // Generate a random keypair for the local peer
        let local_key = identity::Keypair::generate_ed25519();
        let local_peer_id = PeerId::from(local_key.public());

        println!("Local peer id: {}", local_peer_id);
        // STUN Binding to get public IP and port
        // Set up the resolved public address using STUN
        let stun_public_address = match stun_binding().await {
            Ok((ip, port)) => {
                let public_address = format!("/ip4/{}/tcp/{}", ip, port);
                info!("STUN resolved public address: {}", public_address);
                Some(public_address.parse::<Multiaddr>().expect("Invalid public address"))
            }
            Err(e) => {
                eprintln!("Failed to resolve public address via STUN: {:?}", e);
                None
            }
        };

        // Initialize WebRTCHandler
        let (webrtc_handler_instance, mut webrtc_event_rx) = WebRTCHandler::new();
        let webrtc_handler = Arc::new(Mutex::new(webrtc_handler_instance));

        // Clone PeerManager for use in WebRTC
        let peer_manager_clone = Arc::clone(&peer_manager);

        // Clone WebRTCHandler
        let webrtc_handler_clone = Arc::clone(&webrtc_handler);

        tokio::spawn(async move {
            println!("WebRTC event loop started.");
            while let Some(event) = webrtc_event_rx.recv().await {
                match event {
                    WebRTCEvent::PeerConnectionEstablished(peer_id) => {
                        info!("WebRTC Peer connected: {}", peer_id);
        
                        // Attempt to parse `peer_id`
                        match parse_peer_id(&peer_id) {
                            Ok(libp2p_peer_id) => {
                                let ipfs_peer_id = IPFSPeerId::from_libp2p(libp2p_peer_id)
                                    .expect("Failed to convert libp2p::PeerId to IPFSPeerId");
        
                                let base58_peer_id = ipfs_peer_id.to_base58(); // Convert to Base58
        
                                // Derive Wallet Address
                                let wallet_address = derive_wallet_address(&base58_peer_id);
        
                                // Lock the PeerManager and add the mapping
                                let mut manager = peer_manager_clone.lock().await;
                                if !(*manager).is_registered(&ipfs_peer_id) {
                                    manager.add_mapping(ipfs_peer_id.clone(), wallet_address.clone());
                                    println!(
                                        "Peer registered: {} with Wallet: {}",
                                        ipfs_peer_id, wallet_address
                                    );
                                }
                            }
                            Err(err) => {
                                error!("Failed to parse `peer_id`: {}. Error: {}", peer_id, err);
                            }
                        }
                    }
                    WebRTCEvent::PeerConnectionStateChanged(peer_id, state) => {
                        info!("Peer {} connection state changed: {:?}", peer_id, state);
                    }
                    WebRTCEvent::ICECandidate(peer_id, candidate) => {
                        println!("ICE Candidate from {}: {}", peer_id, candidate);
                    }
                }
            }
        });

        // Initialize IPFSHandler
        // Initialize IPFSHandler
        let ipfs_handler = Arc::new(Mutex::new(IPFSHandler::new(peer_manager.clone()).await?));

        let ipfs_addresses = ipfs_handler.lock().await.get_addresses().await?;
        for address in &ipfs_addresses {
            println!("📡 IPFS Node Address: {}", address);
        }
        
        
        // Set up communication for IPFS events
        let (ipfs_event_tx, mut ipfs_event_rx) = mpsc::channel(100);

        // Spawn IPFS Event Loop
        let ipfs_handler_clone = Arc::clone(&ipfs_handler);
        let ipfs_handler_clone1 = Arc::clone(&ipfs_handler);
        let ipfs_handler_clone2 = Arc::clone(&ipfs_handler);
        let ipfs_handler_clone3 = Arc::clone(&ipfs_handler);
        let ipfs_handler_clone4 = Arc::clone(&ipfs_handler);
        let ipfs_handler_clone5 = Arc::clone(&ipfs_handler);
        let ipfs_handler_clone6 = Arc::clone(&ipfs_handler);
        tokio::spawn(async move {
            let mut last_refresh = Instant::now(); // Track the last refresh time

            loop {
                println!("🚀 Starting IPFS Event Loop.");

                if last_refresh.elapsed() > Duration::from_secs(300) {
                    let mut handler = ipfs_handler_clone1.lock().await;
                    if let Err(e) = handler.refresh().await {
                        eprintln!("Error refreshing IPFS node: {:?}", e);
                    }
                    last_refresh = Instant::now();
                }

                match async {
                    while let Some(event) = ipfs_event_rx.recv().await {
                        println!("📬 Received IPFSEvent: {:?}", event);
                        match event {
                            IPFSEvent::AddFile { path, metadata } => {
                                println!("🛠️ Processing AddFile event for path: {}", path);
                                match ipfs_handler_clone2.lock().await.add_file(&path, None, metadata).await {
                                    Ok(cid) => {
                                        println!("✅ File added to IPFS.");
                                        println!("  - CID: {}", cid);
                                    }
                                    Err(e) => {
                                        eprintln!("❌ Failed to add file '{}': {:?}", path, e);
                                    }
                                }
                            }        
                            IPFSEvent::DiscoverContent { cid } => {
                                println!("🔍 Processing DiscoverContent event for CID: {}", cid);
                                if let Err(e) = ipfs_handler_clone3.lock().await.discover_content(&cid).await {
                                    eprintln!("❌ Failed to fetch content for CID '{}': {:?}", cid, e);
                                }
                            }
                            IPFSEvent::FetchContent { cid, include_metadata } => {
                                println!("📂 Processing FetchContent event for CID: {}", cid);
                                match ipfs_handler_clone6.lock().await.get_file(&cid, include_metadata).await {
                                    Ok((file_content, metadata)) => {
                                        println!("✅ Fetched content length: {}", file_content.len());
                                        if let Some(metadata) = metadata {
                                            println!("📋 Metadata: {:?}", metadata);
                                        }
                                    }
                                    Err(e) => {
                                        eprintln!("❌ Failed to fetch content for CID '{}': {:?}", cid, e);
                                    }
                                }
                            }
                        }
                    }                   
                    Ok::<(), ()>(())
                }
                .await
                {
                    Ok(_) => {
                        println!("🚨 IPFS Event Loop completed successfully (this should not happen). Restarting...");
                    }
                    Err(e) => {
                        eprintln!("❌ IPFS Event Loop encountered an error: {:?}", e);
                    }
                }

                // Pause before retrying to avoid tight loops
                tokio::time::sleep(Duration::from_secs(5)).await;
                println!("🔄 Restarting IPFS Event Loop...");
            }
        });
        


        // TURN Binding to allocate a relay address
        let turn_public_address = match turn_binding().await {
            Ok((ip, port)) => {
                let public_address = format!("/ip4/{}/tcp/{}", ip, port);
                println!("TURN allocated relay address: {}", public_address);
                Some(public_address.parse::<Multiaddr>().expect("Invalid public address"))
            }
            Err(e) => {
                eprintln!("Failed to allocate relay address via TURN: {:?}", e);
                None
            }
        };

        setup_upnp_port_mapping(4001, 4001);
        // Set up the transport with Noise protocol for encryption and Yamux for multiplexing
        let noise_config = NoiseConfig::new(&local_key).expect("Failed to create NoiseConfig");

        // Create TCP transport
        // Create TCP transport
        let tcp_transport = TokioTcpTransport::new(libp2p::tcp::Config::default().nodelay(true));
        let tls_config = TlsConfig::new(&local_key).expect("Failed to create TLS config");
        // Add WebSocket (ws) transport on top of TCP
        let ws_transport = WsConfig::new(TokioTcpTransport::new(libp2p::tcp::Config::default()))
        .upgrade(libp2p::core::upgrade::Version::V1Lazy)
        .authenticate(noise_config.clone())
        .multiplex(YamuxConfig::default())
        .boxed();

        // Add DNS resolution for the TCP transport
        let tcp_with_dns = libp2p::dns::tokio::Transport::system(tcp_transport)
            .expect("Failed to initialize DNS transport")
            .upgrade(libp2p::core::upgrade::Version::V1Lazy)
            .authenticate(noise_config.clone())  // Noise protocol for security
            .multiplex(YamuxConfig::default())   // Yamux for multiplexing
            .boxed();

        // Create QUIC transport
        let quic_transport = libp2p::quic::tokio::Transport::new(libp2p::quic::Config::new(&local_key))
            .map(|(peer_id, connection), _| (peer_id, libp2p::core::muxing::StreamMuxerBox::new(connection)))
            .boxed();

        // Combine TCP, WebSocket, and QUIC transports
        let transport = libp2p::core::transport::OrTransport::new(
            libp2p::core::transport::OrTransport::new(tcp_with_dns, ws_transport), // Combine TCP with DNS and WebSocket
            quic_transport,                                                       // Add QUIC
        )
        .map(|either, _| match either {
            futures::future::Either::Left(inner) => match inner {
                futures::future::Either::Left((peer_id, connection)) => (peer_id, connection), // TCP
                futures::future::Either::Right((peer_id, muxer)) => (peer_id, muxer),         // WebSocket
            },
            futures::future::Either::Right((peer_id, muxer)) => (peer_id, muxer),             // QUIC
        })
        .boxed();

        

        
        // Create the combined Kademlia and mDNS behaviour
        // Define behavior
        let behaviour = MyBehaviour::new(local_key.public().to_peer_id()).expect("Failed to create behaviour");

        // Create swarm configuration
        let swarm_config = SwarmConfig::with_tokio_executor();

        // Build the swarm
        let swarm = Arc::new(Mutex::new(Swarm::new(
            transport,                         // Use the custom transport
            behaviour,                         // Attach the custom behavior
            local_key.public().to_peer_id(),   // Specify the local peer ID
            swarm_config,                      // Provide swarm configuration
        )));

        println!("Swarm initialized with all capabilities!");
        // Define listen addresses
        let listen_addresses: Vec<Multiaddr> = vec![
        "/ip4/0.0.0.0/tcp/0".parse::<Multiaddr>()?,   // Bind TCP on any available port
        "/ip6/::/tcp/0".parse::<Multiaddr>()?,       // Bind TCP for IPv6
        "/ip4/0.0.0.0/udp/0/quic".parse::<Multiaddr>()?, // Bind QUIC on any port
        "/ip6/::/udp/0/quic".parse::<Multiaddr>()?,  // Bind QUIC for IPv6
        "/ip4/0.0.0.0/ws".parse::<Multiaddr>()?,     // WebSocket
        "/ip4/0.0.0.0/wss".parse::<Multiaddr>()?,    // Secure WebSocket
        ];


            // Start Axum server in a new task for the Routes
        
        let swarm_clone = Arc::clone(&swarm); // Clone the swarm for messaging APIs
        let message_rx_clone = messaging.receiver.clone();

        let peer_manager_clone = Arc::clone(&peer_manager); // Clone PeerManager for TimeCoin APIs

        tokio::spawn(async move {
            let app = Router::new()
                
                .route("/", get(root)) // Root endpoint
                .route("/add_file", post(add_file)) // Add a file to IPFS
                .route("/add_folder", post(add_folder)) // Add a folder to IPFS
                .route("/add_zip", post(add_zip)) // Add a .zip to IPFS
                .route("/pin/:cid", get(pin_file)) // Pin a file or folder
                .route("/unpin/:cid", get(unpin_file)) // Unpin a file or folder
                .route("/fetch_content/:cid", get(fetch_content)) // Fetch content by CID
                .route("/peers", get(get_peers)) // Get connected peers
                .route("/send_message", post(send_message)) // Send a message to a peer
                .route("/receive_messages", get(receive_messages)) // Get received messages
                .route("/timecoin/send", post(send_timecoin)) // Send TimeCoin
                .route("/timecoin/reward", post(reward_timecoin)) // Reward a peer with TimeCoin
                .route("/timecoin/get_balance/", get(get_balance)) // Get balance for a peer
                
                .layer(axum::Extension(peer_manager_clone)) // Share PeerManager
                .layer(axum::Extension(ipfs_handler_clone4)) // Share the IPFSHandler
                .layer(axum::Extension(swarm_clone)) // Share the Swarm
                .layer(axum::Extension(message_rx_clone)); // Share the message receiver
        
            let addr = SocketAddr::from(([0, 0, 0, 0], 3000));
            println!("Server running on {:?}", addr);
        
            if let Err(e) = axum::Server::bind(&addr)
                .serve(app.into_make_service())
                .await
            {
                error!("Axum server failed: {:?}", e);
            }
        });

        // Listen on the defined addresses
        
        // Add listeners
        for addr in listen_addresses {
            let mut attempts = 3; // Number of retry attempts
            while attempts > 0 {
                let mut swarm = swarm.lock().await; // Lock the swarm
                println!("Attempting to listen on {}", addr);
                match swarm.listen_on(addr.clone()) {
                    Ok(_) => {
                        println!("✅ Listening process started for {}", addr);
                        break; // Break out of the loop if successful
                    }
                    Err(e) => {
                        eprintln!("❌ Failed to start listening on {}: {}", addr, e);
                        attempts -= 1;
                        tokio::time::sleep(Duration::from_secs(2)).await; // Wait before retrying
                    }
                }
            }
            if attempts == 0 {
                eprintln!("❌ Exhausted retries for address: {}", addr);
            }
        }
        

        


        // Add the public address as an external address to the swarm
        if let Some(public_address) = stun_public_address {
            let mut swarm = swarm.lock().await; // Lock the swarm
            swarm.add_external_address(public_address.clone());
            println!("Added external address to the swarm: {}", public_address);
        }

        // Add the public address from TURN to the swarm
        if let Some(public_address) = turn_public_address {
            let mut swarm = swarm.lock().await; // Lock the swarm
            swarm.add_external_address(public_address.clone());
            println!("Added TURN relay address to the swarm: {}", public_address);
        }



        // Add the bootnodes to the Kademlia routing table
        for peer_str in &BOOTNODES {
            // Parse the string into a Multiaddr
            if let Ok(multiaddr) = peer_str.parse::<Multiaddr>() {
                // Extract the PeerId from the Multiaddr
                let peer_id_opt = multiaddr.iter().find_map(|proto| {
                    if let Protocol::P2p(peer_id) = proto {
                        Some(peer_id.clone()) // Directly use the PeerId
                    } else {
                        None
                    }
                });
                
                if let Some(peer_id) = peer_id_opt {
                    let mut swarm = swarm.lock().await; // Lock the swarm
                    // Add the multiaddr to the Kademlia routing table for the extracted PeerId
                    swarm.behaviour_mut().kademlia.add_address(&peer_id, multiaddr.clone());
                    println!("Added bootnode: {}", peer_id);
                } else {
                    println!("No PeerId found in multiaddr: {}", multiaddr);
                }
            } else {
                println!("Invalid multiaddr: {}", peer_str);
            }
        }

        // Dial the bootnodes to join the network
        for peer_str in &BOOTNODES {
            // Parse the string into a Multiaddr
            if let Ok(multiaddr) = peer_str.parse::<Multiaddr>() {
                // Extract the PeerId from the Multiaddr
                let peer_id_opt = multiaddr.iter().find_map(|proto| {
                    if let Protocol::P2p(peer_id) = proto {
                        Some(peer_id.clone()) // Directly use the PeerId
                    } else {
                        None
                    }
                });

                if let Some(peer_id) = peer_id_opt {
                    // Attempt to dial the extracted PeerId
                    let mut swarm = swarm.lock().await; // Lock the swarm
                    match swarm.dial(peer_id.clone()) {
                        Ok(_) => println!("Dialed bootnode: {}", peer_id),
                        Err(e) => println!("Failed to dial {}: {}", peer_id, e),
                    }
                } else {
                    println!("No PeerId found in multiaddr: {}", multiaddr);
                }
            } else {
                println!("Invalid multiaddr: {}", peer_str);
            }
        }

        // Parse CLI arguments
        // let cli_opt = Opt::parse();

        // // Handle initial CLI actions based on user input
        // match cli_opt.argument {
        //     CliArgument::GetPeers { peer_id } => {
        //         let peer_id = peer_id.unwrap_or(PeerId::random());
        //         println!("Searching for the closest peers to {}", peer_id);
        
        //         {
        //             let mut swarm = swarm.lock().await; // Lock the swarm
        //             swarm.behaviour_mut().kademlia.get_closest_peers(peer_id);
        //         }
        
        //         {
        //             let swarm = swarm.lock().await; // Lock the swarm
        //             for addr in swarm.listeners() {
        //                 info!("Active listener: {:?}", addr);
        //             }
        //         }
        //     }
        //     CliArgument::PutPkRecord {} => {
        //         println!("Putting PK record into the DHT");
        
        //         let mut pk_record_key = vec![];
        //         {
        //             let swarm = swarm.lock().await; // Lock the swarm
        //             pk_record_key.put_slice("/pk/".as_bytes());
        //             pk_record_key.put_slice(swarm.local_peer_id().to_bytes().as_slice());
        //         }
        
        //         {
        //             let mut swarm = swarm.lock().await; // Lock the swarm
        //             let pk_record = Record {
        //                 key: Key::new(&pk_record_key),
        //                 value: local_key.public().encode_protobuf(),
        //                 publisher: Some(*swarm.local_peer_id()),
        //                 expires: Some(Instant::now() + Duration::from_secs(60)),
        //             };
        
        //             swarm.behaviour_mut().kademlia.put_record(
        //                 pk_record,
        //                 libp2p::kad::Quorum::N(NonZeroUsize::new(1).unwrap()),
        //             )?;
        //         }
        
        //         {
        //             let swarm = swarm.lock().await; // Lock the swarm
        //             for addr in swarm.listeners() {
        //                 info!("Active listener: {:?}", addr);
        //             }
        //         }
        //     }
        // }

        // let listen_addresses: Vec<Multiaddr> = vec![
        //     "/ip4/0.0.0.0/tcp/4001".parse()?,
        //     "/ip4/0.0.0.0/tcp/5001".parse()?,
        //     "/dnsaddr/bootstrap.libp2p.io".parse()?, // Example for DNS-based multiaddr
        //     "/ip4/127.0.0.1/ws".parse()?,           // Example WebSocket address
        //     "/ip4/127.0.0.1/wss".parse()?,          // Example Secure WebSocket address
        // ];


        // // Listen on the defined addresses
        // for addr in listen_addresses {
        //     swarm.listen_on(addr)?;
        // }




    
        let mut routing_refresh_interval = interval(ROUTING_REFRESH_INTERVAL);
        let mut discovered_ipfs_peers = Vec::new();

        for peer_str in &BOOTNODES {
            if let Ok(multiaddr) = peer_str.parse::<libp2p::Multiaddr>() {
                let peer_id_opt = multiaddr.iter().find_map(|proto| {
                    if let libp2p::multiaddr::Protocol::P2p(peer_id) = proto {
                        Some(peer_id.clone())
                    } else {
                        None
                    }
                });

                if let Some(libp2p_peer_id) = peer_id_opt {
                    if let Ok(ipfs_peer_id) = ipfs::PeerId::from_bytes(&libp2p_peer_id.to_bytes()) {
                        if let Ok(ipfs_multiaddr_without_peer_id) =
                            libp2p_to_ipfs_multiaddr_without_peer_id(multiaddr.clone())
                        {
                            // Convert MultiaddrWithoutPeerId to Multiaddr
                            let ipfs_multiaddr: ipfs::Multiaddr = ipfs_multiaddr_without_peer_id.into();

                            // Push into discovered_ipfs_peers
                            discovered_ipfs_peers.push((ipfs_peer_id, ipfs_multiaddr));
                        } else {
                            eprintln!("Debug: Failing Multiaddr: {:?}", multiaddr);
                        }
                    } else {
                        eprintln!("Debug: Failing Multiaddr: {:?}", multiaddr);
                    }
                }
            }
        }

        // Add discovered peers to the IPFS node
        {
            let mut ipfs_handler = ipfs_handler_clone5.lock().await;
            ipfs_handler.add_peers(discovered_ipfs_peers).await?;
        }


        
        loop {
            tokio::select! {
                // Handle routing table refresh
                
                _ = routing_refresh_interval.tick() => {
                    let local_peer_id;
                    {
                        let swarm = swarm.lock().await; // Lock swarm to access local_peer_id
                        local_peer_id = *swarm.local_peer_id();
                    }
                    println!("Refreshing the routing table for peer: {}", local_peer_id);
        
                    {
                        let mut swarm = swarm.lock().await; // Lock swarm to update behaviour
                        swarm.behaviour_mut().kademlia.get_closest_peers(local_peer_id);
                    }
                }
                event = async {
                    let mut swarm_guard = swarm.lock().await; // Create a longer-lived guard in a separate async block
                    swarm_guard.select_next_some().await
                } => {
                    match event {
                        SwarmEvent::NewListenAddr { address, .. } => {
                            println!("Started listening on {:?}", address);
                
                            let active_listeners: Vec<Multiaddr> = {
                                let swarm_guard = swarm.lock().await; // Re-lock the swarm here
                                swarm_guard.listeners().map(|addr| addr.clone()).collect() // Manually clone each Multiaddr
                            };
                            
                            
                            {
                                
                                let swarm_guard = swarm.lock().await;
                                let active_listeners: Vec<_> = swarm_guard.listeners().collect();
                                for addr in &active_listeners {
                                    println!("Relay server listening at: {}/p2p/{}", addr, local_peer_id);
                                }
                            }
                            
                            
                            
                            // Ensure there's at least one listening address (fallback)
                            // Ensure there's at least one listening address (fallback)
                            if active_listeners.is_empty() {
                                let fallback_addr: Multiaddr = "/ip4/0.0.0.0/tcp/0".parse()?;
                                let mut swarm = swarm.lock().await; // Lock the swarm for fallback listener setup
                                if let Err(err) = swarm.listen_on(fallback_addr.clone()) {
                                    println!("Failed to start fallback listener on {:?}: {:?}", fallback_addr, err);
                                } else {
                                    println!("Started fallback listener on {:?}", fallback_addr);
                                }
                            }
                        }
                        SwarmEvent::Behaviour(MyBehaviourEvent::Kademlia(kad_event)) => {
                            match kad_event {
                                kad::Event::OutboundQueryProgressed {
                                    result: kad::QueryResult::GetClosestPeers(Ok(ok)),
                                    ..
                                } => {
                                    if !ok.peers.is_empty() {
                                        info!("Query finished with closest peers: {:#?}", ok.peers);
                
                                        let mut known_peer_ids = Vec::new();
                                        {
                                            let mut swarm_guard = swarm.lock().await; // Declare mutable swarm_guard to allow mutable operations
                                            
                                            let behaviour = swarm_guard.behaviour_mut(); // This will now work as intended
                                            for bucket in behaviour.kademlia.kbuckets() {
                                                for entry in bucket.iter() {
                                                    known_peer_ids.push(entry.node.key.preimage().clone());
                                                }
                                            }
                                        }
                        
                                        // Perform mutable operations now that the previous borrow is dropped
                                        for peer_info in ok.peers {
                                            let discovered_peer_id = &peer_info.peer_id; // Extract PeerId from PeerInfo
                                            {
                                                let mut swarm = swarm.lock().await; // Lock the swarm to modify Kademlia
                                                swarm
                                                    .behaviour_mut()
                                                    .kademlia
                                                    .add_address(discovered_peer_id, Multiaddr::empty());
                                            }
                        
                                            // Print shared peer info
                                            if let Ok(libp2p_peer_id) = libp2p::PeerId::from_bytes(&discovered_peer_id.to_bytes()) {
                                                let ipfs_peer_id = IPFSPeerId::from_libp2p(libp2p_peer_id)
                                                    .expect("Failed to convert libp2p::PeerId to IPFSPeerId");
                                            
                                                let base58_peer_id = ipfs_peer_id.to_base58(); // Convert to Base58
                                                let wallet_address = derive_wallet_address(&base58_peer_id);
                                            
                                                let mut manager = peer_manager.lock().await;
                                                if !(*manager).is_registered(&ipfs_peer_id) {
                                                    manager.add_mapping(ipfs_peer_id.clone(), wallet_address.clone());
                                                    println!(
                                                        "Peer registered: {} with Wallet: {}",
                                                        ipfs_peer_id, wallet_address
                                                    );
                                                }
                                            }
        
                                            
                        
                                            // Attempt to establish a WebRTC connection with the discovered peer
                                            let webrtc_handler_clone = Arc::clone(&webrtc_handler);
                                            let peer_id_string = discovered_peer_id.to_string();
                                            tokio::spawn(async move {
                                                let mut handler = webrtc_handler_clone.lock().await;
                                                match handler.create_peer_connection(&peer_id_string).await {
                                                    Ok(_) => {
                                                        info!(
                                                            "WebRTC connection initiated with DHT peer: {}",
                                                            peer_id_string
                                                        );
                                                    }
                                                    Err(e) => {
                                                        error!(
                                                            "Failed to establish WebRTC connection with {}: {:?}",
                                                            peer_id_string, e
                                                        );
                                                    }
                                                }
                                            });
                                        }
                                    } else {
                                        println!("Query finished with no closest peers.");
                                    }
                                }
                                // Handle timeout in GetClosestPeers query
                                kad::Event::OutboundQueryProgressed {
                                    result: kad::QueryResult::GetClosestPeers(Err(kad::GetClosestPeersError::Timeout { .. })),
                                    ..
                                } => {
                                    println!("Query for closest peers timed out.");
                                }
                                // Handle successful PutRecord query
                                kad::Event::OutboundQueryProgressed {
                                    result: kad::QueryResult::PutRecord(Ok(_)),
                                    ..
                                } => {
                                    println!("Successfully inserted the PK record into the DHT.");
                                }
                                // Handle failed PutRecord query
                                kad::Event::OutboundQueryProgressed {
                                    result: kad::QueryResult::PutRecord(Err(err)),
                                    ..
                                } => {
                                    error!("Failed to insert the PK record: {:?}", err);
                                }
                                _ => {
                                    debug!("Unhandled Kademlia event: {:?}", kad_event);
                                }
                            }
                        }
                        
                        
                        SwarmEvent::Behaviour(MyBehaviourEvent::AutoNat(autonat_event)) => {
                            match autonat_event {
                                AutoNatEvent::InboundProbe(_) => {
                                    println!("AutoNAT: Received inbound NAT probe.");
                                }
                                AutoNatEvent::OutboundProbe(outbound_probe_event) => {
                                    match outbound_probe_event {
                                        OutboundProbeEvent::Response { peer, probe_id, address } => {
                                            info!(
                                                "Peer {:?} is publicly reachable at address {:?} with probe ID {:?}",
                                                peer, address, probe_id
                                            );
                                        }
                                        OutboundProbeEvent::Request { peer, probe_id } => {
                                            info!(
                                                "AutoNAT: Outbound probe request for peer {:?} with probe ID {:?}",
                                                peer, probe_id
                                            );
                                        }
                                        OutboundProbeEvent::Error { peer, probe_id, error } => {
                                            error!(
                                                "AutoNAT: Probe failed for peer {:?} with probe ID {:?}. Error: {:?}",
                                                peer, probe_id, error
                                            );
                                        }
                                    }
                                }
                                _ => {}
                            }
                        }
                        
                        SwarmEvent::Behaviour(MyBehaviourEvent::Relay(event)) => {
                            println!("Relay event: {:?}", event);
                            match event {
                                libp2p::relay::Event::ReservationReqAccepted { src_peer_id, .. } => {
                                    println!("Relay accepted reservation request from peer: {}", src_peer_id);
                                }
                                libp2p::relay::Event::CircuitReqAccepted { src_peer_id, .. } => {
                                    println!("Relay accepted circuit request from peer: {}", src_peer_id);
                                }
                                _ => {
                                    println!("Unhandled relay event: {:?}", event);
                                }
                            }
                        }
                        SwarmEvent::Behaviour(MyBehaviourEvent::Mdns(mdns_event)) => {
                            match mdns_event {
                                MdnsEvent::Discovered(peers) => {
                                    for (peer_id, addr) in peers {
                                        {
                                            let mut swarm = swarm.lock().await; // Lock `swarm` to modify behaviour
                                            swarm.behaviour_mut().handle_peer_addition(peer_id, addr);
                                        }
                                        // Attempt to parse `peer_id` as `libp2p::PeerId`
                                        if let Ok(libp2p_peer_id) = libp2p::PeerId::from_bytes(&peer_id.to_bytes()) {
                                            let ipfs_peer_id = IPFSPeerId::from_libp2p(libp2p_peer_id)
                                                .expect("Failed to convert libp2p::PeerId to IPFSPeerId");
                                            
                                            let base58_peer_id = ipfs_peer_id.to_base58(); // Convert to Base58
                                            
                                            // Derive Wallet Address
                                            let wallet_address = derive_wallet_address(&base58_peer_id);

                                            // Lock the PeerManager and add the mapping
                                            {
                                                let mut manager = peer_manager.lock().await; // Lock `peer_manager` for mapping
                                                if !(*manager).is_registered(&ipfs_peer_id) {
                                                    manager.add_mapping(ipfs_peer_id.clone(), wallet_address.clone());
                                                    println!(
                                                        "Peer registered: {} with Wallet: {}",
                                                        ipfs_peer_id, wallet_address
                                                    );
                                                }
                                            }
                                        } else {
                                            error!("Failed to parse `peer_id` as libp2p::PeerId");
                                        }
                        
                                        let webrtc_handler_clone = Arc::clone(&webrtc_handler);
                                        let peer_id_string = peer_id.to_string();
                                        tokio::spawn(async move {
                                            let mut handler = webrtc_handler_clone.lock().await; // Lock the mutex
                                            match handler.create_peer_connection(&peer_id_string).await {
                                                Ok(_) => println!("WebRTC connection initiated with peer: {}", peer_id_string),
                                                Err(err) => eprintln!("Failed to create WebRTC connection with {}: {}", peer_id_string, err),
                                            }
                                        });
                                    }
                                }
                                MdnsEvent::Expired(peers) => {
                                    for (peer_id, _) in peers {
                                        let mut swarm = swarm.lock().await; // Lock `swarm` to modify behaviour
                                        swarm.behaviour_mut().handle_peer_removal(peer_id);
                                    }
                                }
                            }
                        }
                        SwarmEvent::ConnectionEstablished { peer_id, endpoint, .. } => {
                            info!("Connected to peer: {} via {:?}", peer_id, endpoint);
                        
                            // Register the peer in the PeerManager
                            let ipfs_peer_id = IPFSPeerId::from_libp2p(peer_id.clone())
                                .expect("Failed to convert libp2p::PeerId to IPFSPeerId");
                        
                            let base58_peer_id = ipfs_peer_id.to_base58(); // Convert to Base58
                        
                            // Derive Wallet Address
                            let wallet_address = derive_wallet_address(&base58_peer_id);
                        
                            // Lock the PeerManager and add the mapping
                            let mut manager = peer_manager.lock().await;
                            if !manager.is_registered(&ipfs_peer_id) {
                                manager.add_mapping(ipfs_peer_id.clone(), wallet_address.clone());
                                println!(
                                    "Peer registered during ConnectionEstablished: {} with Wallet: {}",
                                    ipfs_peer_id, wallet_address
                                );
                            }
                        
                            // Send a test message
                            let test_message = b"Hello, peer!".to_vec();
                            {
                                let mut swarm = swarm.lock().await; // Lock the swarm to send the message
                                let request_id = swarm
                                    .behaviour_mut()
                                    .messaging
                                    .send_request(&peer_id, test_message.clone());
                                info!(
                                    "Sent test message to {}: {:?} with request ID: {:?}",
                                    peer_id,
                                    String::from_utf8_lossy(&test_message),
                                    request_id
                                );
                            }
                        }
                        
                        SwarmEvent::Behaviour(MyBehaviourEvent::Ping(event)) => {
                            match event.result {
                                Ok(rtt) => {
                                    info!("Ping success: Peer {}, RTT: {:?} ms", event.peer, rtt.as_millis());
                                }
                                Err(err) => {
                                    info!("Ping failure: Peer {}, Error: {:?}", event.peer, err);
                                    // Optional: Reconnect or handle the failure.
                                }
                            }
                        }
                        
                        SwarmEvent::Behaviour(MyBehaviourEvent::Messaging(event)) => {
                            match event {
                                RequestResponseEvent::Message {
                                    peer,
                                    message,
                                    connection_id: _,
                                } => match message {
                                    RequestResponseMessage::Request {
                                        request,
                                        channel,
                                        ..
                                    } => {
                                        println!(
                                            "Received message from {:?}: {:?}",
                                            peer,
                                            String::from_utf8_lossy(&request)
                                        );
                        
                                        // Optionally, respond
                                        let response = b"Message received!".to_vec();
                                        {
                                            let mut swarm = swarm.lock().await; // Lock the swarm to send the response
                                            if let Err(err) =
                                                swarm.behaviour_mut().messaging.send_response(channel, response)
                                            {
                                                eprintln!("Failed to send response: {:?}", err);
                                            }
                                        }
                                    }
                                    RequestResponseMessage::Response { response, .. } => {
                                        println!(
                                            "Received response from {:?}: {:?}",
                                            peer,
                                            String::from_utf8_lossy(&response)
                                        );
                                    }
                                },
                                RequestResponseEvent::OutboundFailure {
                                    peer,
                                    request_id,
                                    error,
                                    connection_id: _,
                                } => {
                                    // eprintln!(
                                    //     "Failed to send request {:?} to {:?}: {:?}",
                                    //     request_id, peer, error
                                    // );
                                }
                                RequestResponseEvent::InboundFailure {
                                    peer,
                                    request_id,
                                    error,
                                    connection_id: _,
                                } => {
                                    eprintln!(
                                        "Inbound request {:?} from {:?} failed: {:?}",
                                        request_id, peer, error
                                    );
                                }
                                RequestResponseEvent::ResponseSent {
                                    peer,
                                    request_id,
                                    connection_id: _,
                                } => {
                                    println!(
                                        "Response sent to {:?} for request {:?}",
                                        peer, request_id
                                    );
                                }
                                _ => {
                                    warn!("Unhandled messaging event: {:?}", event);
                                }
                            }
                        }
                        
                        
                        SwarmEvent::ConnectionClosed { peer_id, cause, .. } => {
                            match cause {
                                Some(err) => {
                                    warn!(
                                        "Disconnected from peer: {}. Cause: {:?}",
                                        peer_id, err
                                    );
                                }
                                None => {
                                    warn!(
                                        "Disconnected from peer: {}. No specific cause provided.",
                                        peer_id
                                    );
                                }
                            }
                        
                            // Retry mechanism
                            let swarm_clone = Arc::clone(&swarm); // Clone the swarm to access it in async block
                            let peer_id_clone = peer_id.clone();
                            tokio::spawn(async move {
                                let mut retry_attempts = 0;
                                let max_retries = 5; // Set a maximum retry limit
                            
                                while retry_attempts < max_retries {
                                    tokio::time::sleep(Duration::from_secs(5)).await; // Wait before retrying
                            
                                    let mut swarm_guard = swarm_clone.lock().await;
                                    match swarm_guard.dial(peer_id_clone.clone()) {
                                        Ok(_) => {
                                            info!(
                                                "Reconnection attempt {} to peer {} succeeded.",
                                                retry_attempts + 1,
                                                peer_id_clone
                                            );
                                            break; // Exit the loop on success
                                        }
                                        Err(err) => {
                                            info!(
                                                "Reconnection attempt {} to peer {} failed: {:?}",
                                                retry_attempts + 1,
                                                peer_id_clone,
                                                err
                                            );
                                            retry_attempts += 1;
                                        }
                                    }
                                }
                            
                                if retry_attempts == max_retries {
                                    info!(
                                        "Failed to reconnect to peer {} after {} attempts.",
                                        peer_id_clone, max_retries
                                    );
                                }
                            });
                        }
                        SwarmEvent::Dialing { peer_id, connection_id: _ } => {
                            info!("Dialing peer: {:?}", peer_id);
                        }
                        SwarmEvent::OutgoingConnectionError { peer_id, error, .. } => {
                            let swarm_clone = Arc::clone(&swarm);
                            let peer_id_clone = peer_id.clone(); // Clone once for reuse
                        
                            tokio::spawn(async move {
                                const MAX_RETRIES: u8 = 5; // Set a retry limit
                                let mut retry_attempts = 0;
                        
                                while retry_attempts < MAX_RETRIES {
                                    tokio::time::sleep(Duration::from_secs(5)).await; // Wait before retrying
                        
                                    let mut swarm_guard = swarm_clone.lock().await;
                        
                                    if let Some(peer_id) = peer_id_clone {
                                        let dial_opts = DialOpts::peer_id(peer_id).build(); // Create DialOpts
                                        match swarm_guard.dial(dial_opts) {
                                            Ok(_) => {
                                                info!(
                                                    "Reconnection attempt {} to peer {} succeeded.",
                                                    retry_attempts + 1,
                                                    peer_id
                                                );
                                                break; // Exit the loop on success
                                            }
                                            Err(err) => {
                                                info!(
                                                    "Reconnection attempt {} to peer {} failed: {:?}",
                                                    retry_attempts + 1,
                                                    peer_id,
                                                    err
                                                );
                                            }
                                        }
                                    } else {
                                        info!("Reconnection attempt failed: peer_id is None.");
                                        break;
                                    }
                        
                                    retry_attempts += 1;
                                }
                        
                                if retry_attempts == MAX_RETRIES {
                                    info!(
                                        "Failed to reconnect to peer {:?} after {} attempts.",
                                        peer_id_clone, MAX_RETRIES
                                    );
                                }
                            });
                        }
                        _ => {
                            debug!("Unhandled swarm event: {:?}", event);
                        }
                    }
                }
            }   
        }

}
    new_function_doing_everything().await
}

#[derive(Parser, Debug)]
#[clap(name = "libp2p Kademlia DHT example")]
struct Opt {
    #[clap(subcommand)]
    argument: CliArgument,
}

#[derive(Debug, Parser)]
enum CliArgument {
    /// Get closest peers to a given PeerId
    GetPeers {
        #[clap(long)]
        peer_id: Option<PeerId>,
    },
    /// Put a PK record into the DHT
    PutPkRecord {},
}
