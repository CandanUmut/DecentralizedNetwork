mod api;
mod dag;
mod identity;
mod network;

use anyhow::{Context, Result};
use clap::{Parser, Subcommand};
use libp2p::Multiaddr;
use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use tokio::sync::mpsc;
use tracing_subscriber::EnvFilter;

#[derive(Parser, Debug)]
#[command(name = "timecoin-node", about = "Minimal P2P node with a DAG-based TimeCoin ledger")]
struct Cli {
    #[command(subcommand)]
    command: CliCommand,
}

#[derive(Subcommand, Debug)]
enum CliCommand {
    /// Run a node.
    Run {
        /// Directory for the node key, DAG, and stored blobs.
        #[arg(long, default_value = "./data")]
        data_dir: PathBuf,
        /// Named network (community) to join. Different names = disjoint
        /// ledgers, topics, and protocols.
        #[arg(long, default_value = "main")]
        network: String,
        /// libp2p listen port (TCP and QUIC). 0 picks a random port.
        #[arg(long, default_value_t = 9000)]
        port: u16,
        /// HTTP API listen address.
        #[arg(long, default_value = "127.0.0.1:3000")]
        api: String,
        /// Multiaddr of a peer to connect to on startup (repeatable).
        #[arg(long)]
        bootstrap: Vec<Multiaddr>,
        /// Multiaddr of a relay to reserve a slot on (for NATed nodes).
        #[arg(long)]
        relay: Option<Multiaddr>,
        /// Address this node is publicly reachable at (for relays/servers,
        /// e.g. /ip4/203.0.113.7/tcp/9000). Otherwise AutoNAT discovers it.
        #[arg(long)]
        external_address: Vec<Multiaddr>,
        /// Disable local-network mDNS discovery.
        #[arg(long)]
        no_mdns: bool,
        /// Price in TimeCoin per started 100 KiB for storing blobs for other
        /// peers. 0 stores for free.
        #[arg(long, default_value_t = 1)]
        blob_price: u64,
        /// Largest blob (KiB) this node stores for others.
        #[arg(long, default_value_t = 512)]
        blob_max_kib: u64,
    },
    /// Show a running node's status (peers, DAG tips, balance).
    Status {
        #[arg(long, default_value = "127.0.0.1:3000")]
        api: String,
    },
    /// Attest a contribution: mint TimeCoin to *someone else's* wallet.
    Reward {
        #[arg(long, default_value = "127.0.0.1:3000")]
        api: String,
        /// Wallet (0x…) or peer id of the contributor being rewarded.
        #[arg(long)]
        to: String,
        #[arg(long)]
        amount: u64,
        /// Description of the contribution.
        #[arg(long, default_value = "")]
        memo: String,
    },
    /// Send TimeCoin from the running node's wallet.
    Send {
        #[arg(long, default_value = "127.0.0.1:3000")]
        api: String,
        /// Receiver wallet (0x…) or peer id.
        #[arg(long)]
        to: String,
        #[arg(long)]
        amount: u64,
    },
    /// Show all balances known to a running node.
    Balances {
        #[arg(long, default_value = "127.0.0.1:3000")]
        api: String,
    },
    /// Send a direct encrypted message to a connected peer.
    Message {
        #[arg(long, default_value = "127.0.0.1:3000")]
        api: String,
        /// Recipient peer id.
        #[arg(long)]
        to: String,
        #[arg(long)]
        text: String,
    },
    /// Show messages this node has received.
    Inbox {
        #[arg(long, default_value = "127.0.0.1:3000")]
        api: String,
    },
    /// Pay a peer to store data; prints the content hash.
    Store {
        #[arg(long, default_value = "127.0.0.1:3000")]
        api: String,
        /// Provider peer id.
        #[arg(long)]
        peer: String,
        /// Text to store (for files, use the HTTP API with data_base64).
        #[arg(long)]
        text: String,
    },
    /// Fetch a stored blob from a peer by hash.
    Fetch {
        #[arg(long, default_value = "127.0.0.1:3000")]
        api: String,
        #[arg(long)]
        peer: String,
        #[arg(long)]
        hash: String,
    },
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")),
        )
        .init();

    match Cli::parse().command {
        CliCommand::Run {
            data_dir,
            network,
            port,
            api,
            bootstrap,
            relay,
            external_address,
            no_mdns,
            blob_price,
            blob_max_kib,
        } => {
            let config = network::Config {
                network,
                enable_mdns: !no_mdns,
                data_dir: data_dir.clone(),
                blob_price,
                blob_max_bytes: blob_max_kib * 1024,
            };
            run(data_dir, port, api, bootstrap, relay, external_address, config).await
        }
        CliCommand::Status { api } => print_json(&get(&api, "/status")?),
        CliCommand::Balances { api } => print_json(&get(&api, "/balances")?),
        CliCommand::Inbox { api } => print_json(&get(&api, "/messages")?),
        CliCommand::Reward { api, to, amount, memo } => print_json(&post(
            &api,
            "/reward",
            serde_json::json!({"to": to, "amount": amount, "memo": memo}),
        )?),
        CliCommand::Send { api, to, amount } => print_json(&post(
            &api,
            "/send",
            serde_json::json!({"to": to, "amount": amount}),
        )?),
        CliCommand::Message { api, to, text } => print_json(&post(
            &api,
            "/message",
            serde_json::json!({"peer_id": to, "text": text}),
        )?),
        CliCommand::Store { api, peer, text } => print_json(&post(
            &api,
            "/store",
            serde_json::json!({"peer_id": peer, "text": text}),
        )?),
        CliCommand::Fetch { api, peer, hash } => {
            print_json(&get(&api, &format!("/fetch/{hash}?peer={peer}"))?)
        }
    }
}

#[allow(clippy::too_many_arguments)] // CLI surface, one arg per flag group
async fn run(
    data_dir: PathBuf,
    port: u16,
    api_addr: String,
    bootstrap: Vec<Multiaddr>,
    relay: Option<Multiaddr>,
    external_addresses: Vec<Multiaddr>,
    config: network::Config,
) -> Result<()> {
    let keypair = identity::load_or_generate(&data_dir)?;
    let peer_id = keypair.public().to_peer_id();
    let wallet = dag::wallet_address(&peer_id);
    println!("network : {}", config.network);
    println!("peer id : {peer_id}");
    println!("wallet  : {wallet}");

    let dag = Arc::new(Mutex::new(dag::Dag::load(&data_dir, &config.network)?));
    let peers = Arc::new(Mutex::new(HashSet::new()));
    let (command_tx, command_rx) = mpsc::channel(64);

    let mut net = network::Network::new(&keypair, dag.clone(), peers.clone(), command_rx, config)?;
    let inbox = net.inbox.clone();
    net.listen(port)?;
    for addr in external_addresses {
        net.swarm.add_external_address(addr);
    }
    net.bootstrap(&bootstrap, relay.as_ref())?;
    let network_task = tokio::spawn(net.run());

    let state = api::AppState {
        dag,
        peers,
        inbox,
        commands: command_tx,
        keypair: Arc::new(keypair),
        peer_id,
        wallet,
    };
    let listener = tokio::net::TcpListener::bind(&api_addr)
        .await
        .with_context(|| format!("binding HTTP API on {api_addr}"))?;
    println!("HTTP API + dashboard: http://{api_addr}");
    let api_task = tokio::spawn(async move {
        axum::serve(listener, api::router(state)).await
    });

    tokio::select! {
        r = network_task => anyhow::bail!("network task exited: {r:?}"),
        r = api_task => anyhow::bail!("API task exited: {r:?}"),
        _ = tokio::signal::ctrl_c() => {
            println!("shutting down");
            Ok(())
        }
    }
}

fn get(api: &str, path: &str) -> Result<serde_json::Value> {
    Ok(ureq::get(&format!("http://{api}{path}"))
        .call()
        .with_context(|| format!("is a node running with --api {api}?"))?
        .into_json()?)
}

fn post(api: &str, path: &str, body: serde_json::Value) -> Result<serde_json::Value> {
    match ureq::post(&format!("http://{api}{path}")).send_json(body) {
        Ok(resp) => Ok(resp.into_json()?),
        // Surface the node's JSON error body instead of a bare HTTP status.
        Err(ureq::Error::Status(_, resp)) => Ok(resp.into_json()?),
        Err(e) => Err(e).with_context(|| format!("is a node running with --api {api}?")),
    }
}

fn print_json(value: &serde_json::Value) -> Result<()> {
    println!("{}", serde_json::to_string_pretty(value)?);
    Ok(())
}
