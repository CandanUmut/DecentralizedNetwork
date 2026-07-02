//! Local HTTP API — the successor of the legacy Axum routes, minus the IPFS
//! and messaging endpoints that are out of MVP scope (see SCOPE.md).

use crate::dag::{wallet_address, Dag, Transaction, TxKind};
use crate::network::Command;
use axum::{
    extract::State,
    http::StatusCode,
    routing::{get, post},
    Json, Router,
};
use libp2p::identity::Keypair;
use libp2p::PeerId;
use serde::Deserialize;
use serde_json::{json, Value};
use std::collections::HashSet;
use std::sync::{Arc, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};
use tokio::sync::{mpsc, oneshot};

#[derive(Clone)]
pub struct AppState {
    pub dag: Arc<Mutex<Dag>>,
    pub peers: Arc<Mutex<HashSet<PeerId>>>,
    pub commands: mpsc::Sender<Command>,
    pub keypair: Arc<Keypair>,
    pub peer_id: PeerId,
    pub wallet: String,
}

pub fn router(state: AppState) -> Router {
    Router::new()
        .route("/status", get(status))
        .route("/peers", get(peers))
        .route("/tips", get(tips))
        .route("/transactions", get(transactions))
        .route("/balances", get(balances))
        .route("/balance", get(my_balance))
        .route("/reward", post(reward))
        .route("/send", post(send))
        .route("/connect", post(connect))
        .with_state(state)
}

fn now() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default().as_secs()
}

async fn status(State(s): State<AppState>) -> Json<Value> {
    let (tx_count, tip_count, my_balance) = {
        let dag = s.dag.lock().unwrap();
        (dag.len(), dag.tips().len(), dag.balance(&s.wallet))
    };
    let peers: Vec<String> = s.peers.lock().unwrap().iter().map(|p| p.to_string()).collect();
    let listen_addrs = {
        let (reply, rx) = oneshot::channel();
        let _ = s.commands.send(Command::ListenAddrs(reply)).await;
        rx.await
            .map(|addrs| {
                addrs
                    .into_iter()
                    .map(|a| format!("{a}/p2p/{}", s.peer_id))
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default()
    };
    Json(json!({
        "peer_id": s.peer_id.to_string(),
        "wallet": s.wallet,
        "balance": my_balance,
        "connected_peers": peers,
        "peer_count": peers.len(),
        "dag_transactions": tx_count,
        "dag_tips": tip_count,
        "listen_addrs": listen_addrs,
    }))
}

async fn peers(State(s): State<AppState>) -> Json<Vec<String>> {
    Json(s.peers.lock().unwrap().iter().map(|p| p.to_string()).collect())
}

async fn tips(State(s): State<AppState>) -> Json<Vec<String>> {
    Json(s.dag.lock().unwrap().tips())
}

async fn transactions(State(s): State<AppState>) -> Json<Vec<Transaction>> {
    let dag = s.dag.lock().unwrap();
    let mut txs: Vec<Transaction> = dag.all().cloned().collect();
    txs.sort_by(|a, b| a.timestamp.cmp(&b.timestamp).then(a.id.cmp(&b.id)));
    Json(txs)
}

async fn balances(State(s): State<AppState>) -> Json<Value> {
    Json(json!(s.dag.lock().unwrap().balances()))
}

async fn my_balance(State(s): State<AppState>) -> Json<Value> {
    let balance = s.dag.lock().unwrap().balance(&s.wallet);
    Json(json!({ "wallet": s.wallet, "balance": balance }))
}

#[derive(Deserialize)]
struct RewardRequest {
    /// Wallet to credit; defaults to this node's wallet.
    wallet: Option<String>,
    amount: u64,
    /// What the contribution was.
    memo: Option<String>,
}

/// Record a contribution: mints `amount` TimeCoin to `wallet`, attached to the
/// current DAG tips, signed by this node, and gossiped to the network.
async fn reward(
    State(s): State<AppState>,
    Json(req): Json<RewardRequest>,
) -> (StatusCode, Json<Value>) {
    let receiver = req.wallet.unwrap_or_else(|| s.wallet.clone());
    submit(&s, TxKind::Reward, s.wallet.clone(), receiver, req.amount, req.memo.unwrap_or_default())
        .await
}

#[derive(Deserialize)]
struct SendRequest {
    /// Receiver: a wallet address (0x…) or a peer id (12D3Koo…).
    to: String,
    amount: u64,
    memo: Option<String>,
}

/// Transfer TimeCoin from this node's wallet.
async fn send(State(s): State<AppState>, Json(req): Json<SendRequest>) -> (StatusCode, Json<Value>) {
    let receiver = if req.to.starts_with("0x") {
        req.to.clone()
    } else {
        match req.to.parse::<PeerId>() {
            Ok(peer) => wallet_address(&peer),
            Err(_) => {
                return (
                    StatusCode::BAD_REQUEST,
                    Json(json!({"error": "receiver must be a 0x wallet or a peer id"})),
                )
            }
        }
    };
    // Refuse to create an overdraft locally. (Acceptance elsewhere is
    // signature-based only — see SCOPE.md consensus notes.)
    let balance = s.dag.lock().unwrap().balance(&s.wallet);
    if balance < req.amount as i64 {
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({"error": format!("insufficient balance: have {balance}, need {}", req.amount)})),
        );
    }
    submit(&s, TxKind::Transfer, s.wallet.clone(), receiver, req.amount, req.memo.unwrap_or_default())
        .await
}

async fn submit(
    s: &AppState,
    kind: TxKind,
    sender: String,
    receiver: String,
    amount: u64,
    memo: String,
) -> (StatusCode, Json<Value>) {
    let tx = {
        let mut dag = s.dag.lock().unwrap();
        let tips = dag.tips();
        let tx = match Transaction::create(kind, &s.keypair, sender, receiver, amount, memo, tips, now()) {
            Ok(tx) => tx,
            Err(e) => {
                return (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    Json(json!({"error": format!("failed to create transaction: {e}")})),
                )
            }
        };
        if let Err(e) = dag.insert(tx.clone()) {
            return (
                StatusCode::BAD_REQUEST,
                Json(json!({"error": format!("transaction rejected: {e}")})),
            );
        }
        tx
    };
    if s.commands.send(Command::PublishTx(tx.clone())).await.is_err() {
        return (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({"error": "network task is not running"})),
        );
    }
    (StatusCode::OK, Json(json!({"status": "ok", "transaction": tx})))
}

#[derive(Deserialize)]
struct ConnectRequest {
    address: String,
}

async fn connect(
    State(s): State<AppState>,
    Json(req): Json<ConnectRequest>,
) -> (StatusCode, Json<Value>) {
    let addr = match req.address.parse::<libp2p::Multiaddr>() {
        Ok(a) => a,
        Err(e) => {
            return (
                StatusCode::BAD_REQUEST,
                Json(json!({"error": format!("invalid multiaddr: {e}")})),
            )
        }
    };
    let (reply, rx) = oneshot::channel();
    if s.commands.send(Command::Dial(addr, reply)).await.is_err() {
        return (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({"error": "network task is not running"})),
        );
    }
    match rx.await {
        Ok(Ok(())) => (StatusCode::OK, Json(json!({"status": "dialing", "address": req.address}))),
        Ok(Err(e)) => (StatusCode::BAD_REQUEST, Json(json!({"error": format!("dial failed: {e}")}))),
        Err(_) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({"error": "network task dropped the request"})),
        ),
    }
}
