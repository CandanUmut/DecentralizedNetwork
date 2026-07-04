//! Local HTTP API + dashboard. Successor of the legacy Axum routes; the
//! messaging and (paid) storage endpoints revive the legacy features on the
//! v2 ledger. See ROADMAP.md Stage 2.

use crate::dag::{wallet_address, Dag, Transaction, TxKind};
use crate::network::{BlobRequest, BlobResponse, Command, InboxMessage};
use axum::{
    extract::{Path, Query, State},
    http::StatusCode,
    response::Html,
    routing::{get, post},
    Json, Router,
};
use base64::Engine as _;
use libp2p::identity::Keypair;
use libp2p::PeerId;
use serde::Deserialize;
use serde_json::{json, Value};
use sha2::Digest as _;
use std::collections::HashSet;
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tokio::sync::{mpsc, oneshot};

#[derive(Clone)]
pub struct AppState {
    pub dag: Arc<Mutex<Dag>>,
    pub peers: Arc<Mutex<HashSet<PeerId>>>,
    pub inbox: Arc<Mutex<Vec<InboxMessage>>>,
    pub commands: mpsc::Sender<Command>,
    pub keypair: Arc<Keypair>,
    pub peer_id: PeerId,
    pub wallet: String,
    pub network: String,
    pub data_dir: std::path::PathBuf,
}

/// A custody probe remembered at store time: the hash of a random slice of
/// the blob, checkable later without keeping the blob around.
#[derive(Debug, Clone, serde::Serialize, Deserialize)]
struct Probe {
    hash: String,
    offset: u64,
    len: u32,
    expected: String,
}

pub fn router(state: AppState) -> Router {
    Router::new()
        .route("/", get(dashboard))
        .route("/status", get(status))
        .route("/peers", get(peers))
        .route("/tips", get(tips))
        .route("/transactions", get(transactions))
        .route("/balances", get(balances))
        .route("/balance", get(my_balance))
        .route("/vouch", post(vouch))
        .route("/revoke", post(revoke))
        .route("/trust", get(trust))
        .route("/names", get(names))
        .route("/name", post(set_name))
        .route("/reward", post(reward))
        .route("/send", post(send))
        .route("/connect", post(connect))
        .route("/message", post(message))
        .route("/messages", get(messages))
        .route("/store", post(store))
        .route("/fetch/{hash}", get(fetch))
        .route("/verify/{hash}", get(verify))
        .route("/join-qr.svg", get(join_qr))
        .with_state(state)
}

fn now() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default().as_secs()
}

fn b64() -> base64::engine::GeneralPurpose {
    base64::engine::general_purpose::STANDARD
}

/// Accept a receiver given as a 0x wallet or as a peer id.
fn parse_wallet(s: &str) -> Result<String, String> {
    if s.starts_with("0x") {
        Ok(s.to_string())
    } else {
        s.parse::<PeerId>()
            .map(|p| wallet_address(&p))
            .map_err(|_| "must be a 0x wallet or a peer id".to_string())
    }
}

fn bad(msg: impl Into<String>) -> (StatusCode, Json<Value>) {
    (StatusCode::BAD_REQUEST, Json(json!({"error": msg.into()})))
}

fn internal(msg: impl Into<String>) -> (StatusCode, Json<Value>) {
    (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({"error": msg.into()})))
}

async fn dashboard() -> Html<&'static str> {
    Html(include_str!("dashboard.html"))
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
                    .map(|a| {
                        let suffix = format!("/p2p/{}", s.peer_id);
                        if a.to_string().ends_with(&suffix) {
                            a.to_string()
                        } else {
                            format!("{a}{suffix}")
                        }
                    })
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default()
    };
    Json(json!({
        "network": s.network,
        "peer_id": s.peer_id.to_string(),
        "wallet": s.wallet,
        "balance": my_balance,
        "connected_peers": peers,
        "peer_count": peers.len(),
        "dag_transactions": tx_count,
        "dag_tips": tip_count,
        "inbox_count": s.inbox.lock().unwrap().len(),
        "listen_addrs": listen_addrs,
    }))
}

async fn peers(State(s): State<AppState>) -> Json<Vec<String>> {
    Json(s.peers.lock().unwrap().iter().map(|p| p.to_string()).collect())
}

async fn tips(State(s): State<AppState>) -> Json<Vec<String>> {
    Json(s.dag.lock().unwrap().tips())
}

/// All transactions, oldest first, each flagged with whether the ledger fold
/// applied it (rewards always apply; a transfer may be pending/dead).
async fn transactions(State(s): State<AppState>) -> Json<Vec<Value>> {
    let dag = s.dag.lock().unwrap();
    let ledger = dag.ledger();
    let mut txs: Vec<&Transaction> = dag.all().collect();
    txs.sort_by(|a, b| a.timestamp.cmp(&b.timestamp).then(a.id.cmp(&b.id)));
    Json(
        txs.into_iter()
            .map(|tx| {
                let applied =
                    tx.kind == TxKind::Reward || ledger.applied_transfers.contains(&tx.id);
                let mut v = serde_json::to_value(tx).unwrap_or_default();
                v["applied"] = json!(applied);
                v
            })
            .collect(),
    )
}

#[derive(Deserialize)]
struct TrustQuery {
    /// When true, count only rewards minted by wallets within `depth` vouch
    /// hops of this node's wallet.
    #[serde(default)]
    trusted: bool,
    #[serde(default = "default_depth")]
    depth: u32,
    /// In trusted views, cap how much total mint a single attester's rewards
    /// can contribute (0 = uncapped). Blast-radius limit for a rogue voucher.
    #[serde(default)]
    mint_cap: u64,
}

fn default_depth() -> u32 {
    3
}

async fn balances(State(s): State<AppState>, Query(q): Query<TrustQuery>) -> Json<Value> {
    let dag = s.dag.lock().unwrap();
    if q.trusted {
        let trusted = dag.trusted_set(&s.wallet, q.depth);
        Json(json!(dag.ledger_view(Some(&trusted), q.mint_cap).balances))
    } else {
        Json(json!(dag.balances()))
    }
}

async fn my_balance(State(s): State<AppState>, Query(q): Query<TrustQuery>) -> Json<Value> {
    let dag = s.dag.lock().unwrap();
    let balance = if q.trusted {
        let trusted = dag.trusted_set(&s.wallet, q.depth);
        dag.ledger_view(Some(&trusted), q.mint_cap)
            .balances
            .get(&s.wallet)
            .copied()
            .unwrap_or(0)
    } else {
        dag.balance(&s.wallet)
    };
    Json(json!({ "wallet": s.wallet, "balance": balance, "trusted_view": q.trusted }))
}

#[derive(Deserialize)]
struct VouchRequest {
    /// Wallet (0x…) or peer id being vouched for.
    to: String,
}

/// State on the ledger that this node's owner knows and trusts a wallet.
async fn vouch(
    State(s): State<AppState>,
    Json(req): Json<VouchRequest>,
) -> (StatusCode, Json<Value>) {
    let receiver = match parse_wallet(&req.to) {
        Ok(w) => w,
        Err(e) => return bad(format!("to: {e}")),
    };
    if receiver == s.wallet {
        return bad("cannot vouch for yourself");
    }
    submit(&s, TxKind::Vouch, receiver, 0, 0, String::new()).await
}

/// Withdraw a previous vouch (latest statement wins on the ledger).
async fn revoke(
    State(s): State<AppState>,
    Json(req): Json<VouchRequest>,
) -> (StatusCode, Json<Value>) {
    let receiver = match parse_wallet(&req.to) {
        Ok(w) => w,
        Err(e) => return bad(format!("to: {e}")),
    };
    if receiver == s.wallet {
        return bad("cannot revoke yourself");
    }
    submit(&s, TxKind::Revoke, receiver, 0, 0, String::new()).await
}

/// This node's trust neighborhood: wallets within `depth` vouch hops.
async fn trust(State(s): State<AppState>, Query(q): Query<TrustQuery>) -> Json<Value> {
    let trusted = s.dag.lock().unwrap().trusted_set(&s.wallet, q.depth);
    Json(json!({ "viewer": s.wallet, "depth": q.depth, "trusted": trusted }))
}

async fn names(State(s): State<AppState>) -> Json<Value> {
    Json(json!(s.dag.lock().unwrap().names()))
}

#[derive(Deserialize)]
struct NameRequest {
    name: String,
}

/// Set this wallet's display name (a label, not an identity — names are not
/// unique and prove nothing).
async fn set_name(
    State(s): State<AppState>,
    Json(req): Json<NameRequest>,
) -> (StatusCode, Json<Value>) {
    submit(&s, TxKind::Profile, s.wallet.clone(), 0, 0, req.name).await
}

#[derive(Deserialize)]
struct RewardRequest {
    /// Contributor being attested: wallet (0x…) or peer id. Must not be
    /// this node's own wallet — rewards are attestations by others.
    to: String,
    amount: u64,
    /// What the contribution was.
    memo: Option<String>,
}

/// Attest a contribution: mints `amount` TimeCoin to someone else's wallet,
/// signed by this node (which is thereby on record as the voucher).
async fn reward(
    State(s): State<AppState>,
    Json(req): Json<RewardRequest>,
) -> (StatusCode, Json<Value>) {
    let receiver = match parse_wallet(&req.to) {
        Ok(w) => w,
        Err(e) => return bad(format!("to: {e}")),
    };
    if receiver == s.wallet {
        return bad("cannot mint a reward to yourself — ask the peer you helped to run this");
    }
    submit(&s, TxKind::Reward, receiver, req.amount, 0, req.memo.unwrap_or_default()).await
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
    let receiver = match parse_wallet(&req.to) {
        Ok(w) => w,
        Err(e) => return bad(format!("to: {e}")),
    };
    pay(&s, receiver, req.amount, req.memo.unwrap_or_default()).await
}

/// Create, locally insert, and gossip a transfer; refuses local overdrafts.
/// (A modified client skipping this check gains nothing: unfunded transfers
/// are never applied by any node's ledger fold.)
async fn pay(
    s: &AppState,
    receiver: String,
    amount: u64,
    memo: String,
) -> (StatusCode, Json<Value>) {
    let seq = {
        let dag = s.dag.lock().unwrap();
        let balance = dag.balance(&s.wallet);
        if balance < amount as i64 {
            return bad(format!("insufficient balance: have {balance}, need {amount}"));
        }
        dag.next_seq(&s.wallet)
    };
    submit(s, TxKind::Transfer, receiver, amount, seq, memo).await
}

async fn submit(
    s: &AppState,
    kind: TxKind,
    receiver: String,
    amount: u64,
    seq: u64,
    memo: String,
) -> (StatusCode, Json<Value>) {
    let tx = {
        let mut dag = s.dag.lock().unwrap();
        let tips = dag.tips();
        let tx = match Transaction::create(
            kind,
            &s.keypair,
            s.wallet.clone(),
            receiver,
            amount,
            seq,
            memo,
            tips,
            now(),
        ) {
            Ok(tx) => tx,
            Err(e) => return internal(format!("failed to create transaction: {e}")),
        };
        if let Err(e) = dag.insert(tx.clone()) {
            return bad(format!("transaction rejected: {e}"));
        }
        tx
    };
    if s.commands.send(Command::PublishTx(tx.clone())).await.is_err() {
        return internal("network task is not running");
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
        Err(e) => return bad(format!("invalid multiaddr: {e}")),
    };
    let (reply, rx) = oneshot::channel();
    if s.commands.send(Command::Dial(addr, reply)).await.is_err() {
        return internal("network task is not running");
    }
    match rx.await {
        Ok(Ok(())) => (StatusCode::OK, Json(json!({"status": "dialing", "address": req.address}))),
        Ok(Err(e)) => bad(format!("dial failed: {e}")),
        Err(_) => internal("network task dropped the request"),
    }
}

#[derive(Deserialize)]
struct MessageRequest {
    peer_id: String,
    text: String,
}

/// Send a direct message over the encrypted, peer-authenticated stream.
async fn message(
    State(s): State<AppState>,
    Json(req): Json<MessageRequest>,
) -> (StatusCode, Json<Value>) {
    let peer: PeerId = match req.peer_id.parse() {
        Ok(p) => p,
        Err(_) => return bad("invalid peer_id"),
    };
    if req.text.is_empty() || req.text.len() > 64 * 1024 {
        return bad("text must be 1..65536 bytes");
    }
    let (reply, rx) = oneshot::channel();
    if s.commands.send(Command::SendMessage(peer, req.text.clone(), reply)).await.is_err() {
        return internal("network task is not running");
    }
    match rx.await {
        Ok(Ok(())) => (StatusCode::OK, Json(json!({"status": "delivered", "to": req.peer_id}))),
        Ok(Err(e)) => bad(e.to_string()),
        Err(_) => internal("network task dropped the request"),
    }
}

async fn messages(State(s): State<AppState>) -> Json<Vec<InboxMessage>> {
    Json(s.inbox.lock().unwrap().clone())
}

#[derive(Deserialize)]
struct StoreRequest {
    /// Provider peer id.
    peer_id: String,
    /// Content, either as text or base64 (exactly one).
    text: Option<String>,
    data_base64: Option<String>,
}

/// Pay a peer to store a blob: quote → transfer on the ledger → put.
async fn store(
    State(s): State<AppState>,
    Json(req): Json<StoreRequest>,
) -> (StatusCode, Json<Value>) {
    let peer: PeerId = match req.peer_id.parse() {
        Ok(p) => p,
        Err(_) => return bad("invalid peer_id"),
    };
    let data: Vec<u8> = match (&req.text, &req.data_base64) {
        (Some(t), None) => t.clone().into_bytes(),
        (None, Some(b)) => match b64().decode(b) {
            Ok(d) => d,
            Err(e) => return bad(format!("bad base64: {e}")),
        },
        _ => return bad("provide exactly one of text or data_base64"),
    };
    if data.is_empty() {
        return bad("empty blob");
    }

    // 1. Quote.
    let price = match blob_roundtrip(&s, peer, BlobRequest::Quote { size: data.len() as u64 }).await
    {
        Ok(BlobResponse::Price { price }) => price,
        Ok(BlobResponse::Refused { reason }) => return bad(format!("provider refused: {reason}")),
        Ok(other) => return internal(format!("unexpected quote response: {other:?}")),
        Err(e) => return bad(e),
    };

    // 2. Pay on the ledger (skipped for free storage).
    let payment_id = if price > 0 {
        let provider_wallet = wallet_address(&peer);
        let (code, body) = pay(&s, provider_wallet, price, format!("storage of {} bytes", data.len())).await;
        if code != StatusCode::OK {
            return (code, body);
        }
        body.0["transaction"]["id"].as_str().unwrap_or_default().to_string()
    } else {
        String::new()
    };

    // 3. Put, retrying briefly while the payment gossips to the provider.
    let mut last_refusal = String::new();
    for _ in 0..10 {
        match blob_roundtrip(&s, peer, BlobRequest::Put { data: data.clone(), payment: payment_id.clone() })
            .await
        {
            Ok(BlobResponse::Stored { hash }) => {
                let probes = save_probes(&s, &hash, &data);
                return (
                    StatusCode::OK,
                    Json(json!({
                        "status": "stored",
                        "provider": req.peer_id,
                        "hash": hash,
                        "price": price,
                        "payment": payment_id,
                        "custody_probes_saved": probes,
                    })),
                )
            }
            Ok(BlobResponse::Refused { reason }) if reason.contains("retry") => {
                last_refusal = reason;
                tokio::time::sleep(Duration::from_millis(400)).await;
            }
            Ok(BlobResponse::Refused { reason }) => return bad(format!("provider refused: {reason}")),
            Ok(other) => return internal(format!("unexpected put response: {other:?}")),
            Err(e) => return bad(e),
        }
    }
    bad(format!("provider kept refusing: {last_refusal} (payment {payment_id} was still made — retry /store later without paying again is NOT possible yet; see ROADMAP escrow)"))
}

#[derive(Deserialize)]
struct FetchQuery {
    peer: String,
}

/// Fetch a blob by hash from a provider; verifies the content hash locally.
async fn fetch(
    State(s): State<AppState>,
    Path(hash): Path<String>,
    Query(q): Query<FetchQuery>,
) -> (StatusCode, Json<Value>) {
    let peer: PeerId = match q.peer.parse() {
        Ok(p) => p,
        Err(_) => return bad("invalid peer id"),
    };
    match blob_roundtrip(&s, peer, BlobRequest::Get { hash: hash.clone() }).await {
        Ok(BlobResponse::Blob { data }) => {
            let actual = hex::encode(sha2::Sha256::digest(&data));
            if actual != hash {
                return bad(format!("provider returned wrong content (hash {actual})"));
            }
            (
                StatusCode::OK,
                Json(json!({
                    "hash": hash,
                    "size": data.len(),
                    "text": String::from_utf8(data.clone()).ok(),
                    "data_base64": b64().encode(&data),
                })),
            )
        }
        Ok(BlobResponse::Refused { reason }) => bad(format!("provider refused: {reason}")),
        Ok(other) => internal(format!("unexpected fetch response: {other:?}")),
        Err(e) => bad(e),
    }
}

/// Remember hashes of a few random slices so we can later spot-check that a
/// provider still holds the blob, without keeping the blob ourselves.
fn save_probes(s: &AppState, hash: &str, data: &[u8]) -> usize {
    use rand::Rng;
    let mut rng = rand::thread_rng();
    let mut probes = Vec::new();
    let max_len = data.len().min(64);
    for _ in 0..4 {
        let len = rng.gen_range(1..=max_len) as u32;
        let offset = rng.gen_range(0..=(data.len() - len as usize)) as u64;
        let slice = &data[offset as usize..offset as usize + len as usize];
        probes.push(Probe {
            hash: hash.to_string(),
            offset,
            len,
            expected: hex::encode(sha2::Sha256::digest(slice)),
        });
    }
    let path = s.data_dir.join("probes.jsonl");
    let mut written = 0;
    if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(&path) {
        use std::io::Write as _;
        for p in &probes {
            let ok = serde_json::to_writer(&mut f, p).is_ok() && f.write_all(b"\n").is_ok();
            if ok {
                written += 1;
            }
        }
    }
    written
}

/// Spot-check that a provider still holds a blob: ask for one remembered
/// random slice and compare its hash. A provider that deleted the data (or
/// stored only its hash) cannot answer.
async fn verify(
    State(s): State<AppState>,
    Path(hash): Path<String>,
    Query(q): Query<FetchQuery>,
) -> (StatusCode, Json<Value>) {
    let peer: PeerId = match q.peer.parse() {
        Ok(p) => p,
        Err(_) => return bad("invalid peer id"),
    };
    let probes: Vec<Probe> = std::fs::read_to_string(s.data_dir.join("probes.jsonl"))
        .unwrap_or_default()
        .lines()
        .filter_map(|l| serde_json::from_str(l).ok())
        .filter(|p: &Probe| p.hash == hash)
        .collect();
    let Some(probe) = probes.get(now() as usize % probes.len().max(1)).cloned() else {
        return bad("no custody probes saved for this hash (was it stored from this node?)");
    };
    match blob_roundtrip(
        &s,
        peer,
        BlobRequest::Range { hash: hash.clone(), offset: probe.offset, len: probe.len },
    )
    .await
    {
        Ok(BlobResponse::RangeData { data }) => {
            let actual = hex::encode(sha2::Sha256::digest(&data));
            let ok = actual == probe.expected && data.len() == probe.len as usize;
            (
                if ok { StatusCode::OK } else { StatusCode::BAD_GATEWAY },
                Json(json!({
                    "hash": hash,
                    "verified": ok,
                    "probe": { "offset": probe.offset, "len": probe.len },
                })),
            )
        }
        Ok(BlobResponse::Refused { reason }) => (
            StatusCode::BAD_GATEWAY,
            Json(json!({ "hash": hash, "verified": false, "provider_said": reason })),
        ),
        Ok(other) => internal(format!("unexpected verify response: {other:?}")),
        Err(e) => bad(e),
    }
}

/// The join file as a scannable QR (SVG), for phones.
async fn join_qr(State(s): State<AppState>) -> axum::response::Response {
    use axum::response::IntoResponse;
    let listen_addrs = {
        let (reply, rx) = oneshot::channel();
        let _ = s.commands.send(Command::ListenAddrs(reply)).await;
        rx.await.unwrap_or_default()
    };
    let bootstrap: Vec<String> = listen_addrs
        .into_iter()
        .map(|a| format!("{a}/p2p/{}", s.peer_id))
        .filter(|a| !a.contains("/127.0.0.1/") && !a.contains("/::1/"))
        .collect();
    let join = json!({ "network": s.network, "bootstrap": bootstrap }).to_string();
    match qrcode::QrCode::new(join.as_bytes()) {
        Ok(code) => {
            let svg = code
                .render::<qrcode::render::svg::Color>()
                .min_dimensions(240, 240)
                .build();
            ([("content-type", "image/svg+xml")], svg).into_response()
        }
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, format!("QR failed: {e}")).into_response(),
    }
}

async fn blob_roundtrip(
    s: &AppState,
    peer: PeerId,
    request: BlobRequest,
) -> Result<BlobResponse, String> {
    let (reply, rx) = oneshot::channel();
    s.commands
        .send(Command::Blob(peer, request, reply))
        .await
        .map_err(|_| "network task is not running".to_string())?;
    match rx.await {
        Ok(Ok(response)) => Ok(response),
        Ok(Err(e)) => Err(e.to_string()),
        Err(_) => Err("network task dropped the request".into()),
    }
}
