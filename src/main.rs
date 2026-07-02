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
        /// Directory for the node key and the DAG (created if missing).
        #[arg(long, default_value = "./data")]
        data_dir: PathBuf,
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
    },
    /// Show a running node's status (peers, DAG tips, balance).
    Status {
        #[arg(long, default_value = "127.0.0.1:3000")]
        api: String,
    },
    /// Record a contribution: mint TimeCoin to a wallet via a running node.
    Reward {
        #[arg(long, default_value = "127.0.0.1:3000")]
        api: String,
        /// Wallet to credit (defaults to the node's own wallet).
        #[arg(long)]
        wallet: Option<String>,
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
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")),
        )
        .init();

    match Cli::parse().command {
        CliCommand::Run { data_dir, port, api, bootstrap, relay, external_address, no_mdns } => {
            run(data_dir, port, api, bootstrap, relay, external_address, no_mdns).await
        }
        CliCommand::Status { api } => print_json(&get(&api, "/status")?),
        CliCommand::Balances { api } => print_json(&get(&api, "/balances")?),
        CliCommand::Reward { api, wallet, amount, memo } => print_json(&post(
            &api,
            "/reward",
            serde_json::json!({"wallet": wallet, "amount": amount, "memo": memo}),
        )?),
        CliCommand::Send { api, to, amount } => print_json(&post(
            &api,
            "/send",
            serde_json::json!({"to": to, "amount": amount}),
        )?),
    }
}

async fn run(
    data_dir: PathBuf,
    port: u16,
    api_addr: String,
    bootstrap: Vec<Multiaddr>,
    relay: Option<Multiaddr>,
    external_addresses: Vec<Multiaddr>,
    no_mdns: bool,
) -> Result<()> {
    let keypair = identity::load_or_generate(&data_dir)?;
    let peer_id = keypair.public().to_peer_id();
    let wallet = dag::wallet_address(&peer_id);
    println!("peer id : {peer_id}");
    println!("wallet  : {wallet}");

    let dag = Arc::new(Mutex::new(dag::Dag::load(&data_dir)?));
    let peers = Arc::new(Mutex::new(HashSet::new()));
    let (command_tx, command_rx) = mpsc::channel(64);

    let mut net =
        network::Network::new(&keypair, dag.clone(), peers.clone(), command_rx, !no_mdns)?;
    net.listen(port)?;
    for addr in external_addresses {
        net.swarm.add_external_address(addr);
    }
    net.bootstrap(&bootstrap, relay.as_ref())?;
    let network_task = tokio::spawn(net.run());

    let state = api::AppState {
        dag,
        peers,
        commands: command_tx,
        keypair: Arc::new(keypair),
        peer_id,
        wallet,
    };
    let listener = tokio::net::TcpListener::bind(&api_addr)
        .await
        .with_context(|| format!("binding HTTP API on {api_addr}"))?;
    println!("HTTP API: http://{api_addr}");
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
