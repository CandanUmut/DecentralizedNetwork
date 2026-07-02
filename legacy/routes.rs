use axum::{
    extract::{Extension, Path},
    response::Html,
    Json,
    routing::post,
    Router,
};

use axum::handler::Handler;
use rsa::RsaPublicKey;
use rsa::RsaPrivateKey;
use axum::{response::IntoResponse};
use axum::extract::Json as MsgJson;
use crate::timecoin::peer_manager::PeerManager;
use crate::timecoin::peer_id::IPFSPeerId;
use libp2p::PeerId;
use tokio::sync::mpsc;
use libp2p::Multiaddr;
use serde_json::json;
use serde_json::Value;
use std::sync::Arc;
use tokio::sync::Mutex;
use base64;
use crate::IPFSHandler;
use crate::{MyBehaviour, MyBehaviourEvent};
use libp2p::swarm::{Swarm, SwarmEvent};
use std::str::FromStr;
use tokio::sync::mpsc::Receiver;
use std::fs;
use axum::response::Response;
use axum::http::StatusCode;
use crate::myipfs::Metadata;

pub async fn root() -> impl IntoResponse {
    // Define the path to the HTML file
    let file_path = "C:\\Users\\umtcn\\MyProjectsNetwork\\libp2p\\libp2p\\examples\\ipfs-kad\\test_dir\\subdir\\index.html";
    
    match fs::read_to_string(file_path) {
        Ok(contents) => Html(contents), // Serve the file contents as HTML
        Err(err) => {
            eprintln!("Error reading HTML file: {:?}", err);
            Html("<h1>Error: Could not load the requested page</h1>".to_string())
        }
    }
}


pub fn to_axum_result(
    result: Result<serde_json::Value, (StatusCode, serde_json::Value)>,
) -> Result<Json<serde_json::Value>, (StatusCode, Json<serde_json::Value>)> {
    result.map(Json).map_err(|(status, value)| (status, Json(value)))
}


// pub async fn send_timecoin(
//     Json(payload): Json<Value>,
//     Extension(peer_manager): Extension<Arc<Mutex<PeerManager>>>,
// ) -> impl IntoResponse {
//     let sender_id = payload["sender_id"].as_str().unwrap_or_default();
//     let receiver_id = payload["receiver_id"].as_str().unwrap_or_default();
//     let amount = payload["amount"].as_u64().unwrap_or(0);

//     if sender_id.is_empty() || receiver_id.is_empty() || amount == 0 {
//         return Json(json!({ "error": "Invalid input: sender_id, receiver_id, or amount missing" }));
//     }

//     let sender_peer = match IPFSPeerId::from_base58(sender_id) {
//         Ok(peer) => peer,
//         Err(_) => return Json(json!({ "error": "Invalid sender ID" })),
//     };

//     let receiver_peer = match IPFSPeerId::from_base58(receiver_id) {
//         Ok(peer) => peer,
//         Err(_) => return Json(json!({ "error": "Invalid receiver ID" })),
//     };

//     let mut manager = peer_manager.lock().await;

//     let private_key = match RsaPrivateKey::new(&mut rand::rngs::OsRng, 2048) {
//         Ok(key) => key,
//         Err(_) => return Json(json!({ "error": "Failed to generate RSA keys" })),
//     };

//     let public_key = RsaPublicKey::from(&private_key);

//     match manager.send_timecoin_logic(sender_peer, receiver_peer, amount, &private_key, public_key) {
//         Ok(msg) => Json(json!({ "status": "success", "message": msg })),
//         Err((status, err)) => Json(json!({ "error": err })),
//     }
// }

pub async fn send_timecoin(
    Extension(peer_manager): Extension<Arc<Mutex<PeerManager>>>,
    Json(payload): Json<serde_json::Value>,
) -> impl IntoResponse {
    let sender_id = payload["sender_id"].as_str().unwrap_or("");
    let receiver_id = payload["receiver_id"].as_str().unwrap_or("");
    let amount = payload["amount"].as_u64().unwrap_or(0);

    if sender_id.is_empty() || receiver_id.is_empty() || amount == 0 {
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({ "error": "Invalid input: sender_id, receiver_id, or amount missing" })),
        );
    }

    let sender_peer = match IPFSPeerId::from_base58(sender_id) {
        Ok(peer) => peer,
        Err(_) => {
            return (
                StatusCode::BAD_REQUEST,
                Json(json!({ "error": "Invalid sender ID" })),
            );
        }
    };

    let receiver_peer = match IPFSPeerId::from_base58(receiver_id) {
        Ok(peer) => peer,
        Err(_) => {
            return (
                StatusCode::BAD_REQUEST,
                Json(json!({ "error": "Invalid receiver ID" })),
            );
        }
    };

    let private_key = match RsaPrivateKey::new(&mut rand::rngs::OsRng, 2048) {
        Ok(key) => key,
        Err(_) => {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({ "error": "Failed to generate RSA keys" })),
            );
        }
    };

    let public_key = RsaPublicKey::from(&private_key);

    let mut manager = peer_manager.lock().await;

    match manager.send_timecoin_logic(sender_peer, receiver_peer, amount, &private_key, public_key) {
        Ok(success) => (StatusCode::OK, Json(success)),
        Err(error) => (StatusCode::BAD_REQUEST, Json(error)),
    }
}






pub async fn get_balance(
    Extension(peer_manager): Extension<Arc<Mutex<PeerManager>>>,
    Json(payload): Json<serde_json::Value>,
) -> impl IntoResponse {
    // Extract and validate the peer_id
    let peer_id = payload["peer_id"].as_str().unwrap_or("");
    if peer_id.is_empty() {
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({ "error": "Invalid input: peer_id is missing" })),
        );
    }

    // Parse peer ID
    let peer_id = match IPFSPeerId::from_base58(peer_id) {
        Ok(id) => id,
        Err(_) => {
            return (
                StatusCode::BAD_REQUEST,
                Json(json!({ "error": "Invalid peer ID" })),
            );
        }
    };

    // Lock the PeerManager
    let manager = peer_manager.lock().await;

    // Call the `get_balance` logic in PeerManager
    match manager.get_balance(&peer_id) {
        Ok(balance) => (StatusCode::OK, Json(balance)), // Success response
        Err(error) => (StatusCode::BAD_REQUEST, Json(error)), // Error response
    }
}


pub async fn reward_timecoin(
    Extension(peer_manager): Extension<Arc<Mutex<PeerManager>>>,
    Json(payload): Json<serde_json::Value>,
) -> impl IntoResponse {
    // Extract and validate inputs
    let peer_id = payload["peer_id"].as_str().unwrap_or("");
    let amount = payload["amount"].as_u64().unwrap_or(0);

    if peer_id.is_empty() || amount == 0 {
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({ "error": "Invalid input: peer_id or amount missing" })),
        );
    }

    // Parse peer ID
    let peer_id = match IPFSPeerId::from_base58(peer_id) {
        Ok(id) => id,
        Err(_) => {
            return (
                StatusCode::BAD_REQUEST,
                Json(json!({ "error": "Invalid peer ID" })),
            );
        }
    };

    // Lock the PeerManager for safe access
    let mut manager = peer_manager.lock().await;

    // Call the `reward_timecoin` logic in PeerManager
    match manager.reward_timecoin(peer_id, amount) {
        Ok(success) => (StatusCode::OK, Json(success)), // Success response
        Err(error) => (StatusCode::BAD_REQUEST, Json(error)), // Error response
    }
}


pub async fn get_peers(
    Extension(swarm): Extension<Arc<Mutex<Swarm<MyBehaviour>>>>,
) -> Json<Vec<String>> {
    let swarm = swarm.lock().await;
    let peers: Vec<String> = swarm.connected_peers().map(|peer| peer.to_string()).collect();
    Json(peers)
}

pub async fn send_message(
    Json(payload): Json<Value>, // Axum's built-in JSON extractor
    Extension(swarm): Extension<Arc<Mutex<Swarm<MyBehaviour>>>>,
) -> impl IntoResponse {
    let peer_id = payload["peer_id"].as_str().unwrap_or_default();
    let message = payload["message"].as_str().unwrap_or_default();

    if peer_id.is_empty() || message.is_empty() {
        return Json(json!({"error": "peer_id and message are required"}));
    }

    let peer_id = match PeerId::from_str(peer_id) {
        Ok(id) => id,
        Err(_) => return Json(json!({"error": "Invalid peer_id"})),
    };

    let mut swarm = swarm.lock().await;
    let request_id = swarm.behaviour_mut().messaging.send_request(&peer_id, message.as_bytes().to_vec());

    Json(json!({
        "status": "Message sent",
        "peer_id": peer_id.to_string(),
        "message": message,
        "request_id": format!("{:?}", request_id),
    }))
}



pub async fn receive_messages(
    Extension(message_rx): Extension<Arc<Mutex<Receiver<(String, String)>>>>,
) -> Json<Vec<(String, String)>> {
    let mut rx = message_rx.lock().await;
    let mut messages = Vec::new();

    while let Ok(msg) = rx.try_recv() {
        messages.push(msg);
    }

    Json(messages)
}
/// Connect to a specific peer
pub async fn connect_peer(
    MsgJson(payload): MsgJson<Value>, // Use the new MessagingJson alias
    Extension(swarm): Extension<Arc<Mutex<Swarm<MyBehaviour>>>>,
) -> Json<Value> {
    let address = payload["address"].as_str().unwrap_or_default();

    if address.is_empty() {
        return Json(json!({"error": "address is required"}));
    }

    let multiaddr = match address.parse::<Multiaddr>() {
        Ok(addr) => addr,
        Err(_) => return Json(json!({"error": "Invalid address"})),
    };

    let mut swarm = swarm.lock().await;
    match swarm.dial(multiaddr.clone()) {
        Ok(_) => Json(json!({"status": "Dial initiated", "address": address})),
        Err(err) => Json(json!({"error": format!("Failed to dial: {}", err)})),
    }
}

pub async fn add_file(
    Extension(ipfs_handler): Extension<Arc<Mutex<IPFSHandler>>>,
    Json(payload): Json<serde_json::Value>,
) -> Json<serde_json::Value> {
    let path = payload["path"].as_str().unwrap_or("");
    let content_base64 = payload["content"].as_str();

    // Parse metadata fields
    let tags = payload["metadata"]["tags"]
        .as_array()
        .map(|tags| {
            tags.iter()
                .filter_map(|tag| tag.as_str().map(|s| s.to_string()))
                .collect::<Vec<_>>()
        })
        .unwrap_or_else(|| vec![]);
    let category = payload["metadata"]["category"]
        .as_str()
        .unwrap_or("Uncategorized")
        .to_string();
    let description = payload["metadata"]["description"]
        .as_str()
        .map(|s| s.to_string());
    let timestamp = chrono::Utc::now().timestamp() as u64;

    // Construct metadata
    let metadata = Metadata {
        tags,
        category,
        timestamp,
        description,
    };

    if path.is_empty() {
        return Json(json!({"error": "Path is required"}));
    }

    println!(
        "Received request to add file. Path: {}, Content: {:?}, Metadata: {:?}",
        path, content_base64, metadata
    );

    let result = if let Some(base64_content) = content_base64 {
        let content_bytes = match base64::decode(base64_content) {
            Ok(bytes) => bytes,
            Err(e) => {
                return Json(json!({
                    "error": format!("Failed to decode Base64 content: {}", e)
                }));
            }
        };

        let temp_path = format!("/tmp/{}", path);
        match tokio::fs::write(&temp_path, &content_bytes).await {
            Ok(_) => {
                println!("Temporary file written successfully at: {}", temp_path);
                let cid_result = ipfs_handler
                    .lock()
                    .await
                    .add_file(&temp_path, None, Some(metadata))
                    .await;
                let _ = tokio::fs::remove_file(&temp_path).await;
                cid_result
            }
            Err(e) => Err(anyhow::anyhow!(format!("Failed to write temporary file: {}", e))),
        }
    } else {
        println!("Adding file directly from path: {}", path);
        ipfs_handler.lock().await.add_file(path, None, Some(metadata)).await
    };

    match result {
        Ok(cid) => {
            println!("File added successfully. CID: {}", cid);

            // Pin the file using the CID
            let pin_result = ipfs_handler.lock().await.pin_file(&cid).await;
            match pin_result {
                Ok(_) => Json(json!({
                    "cid": cid,
                    "status": "File added and pinned"
                })),
                Err(e) => Json(json!({
                    "cid": cid,
                    "warning": format!("File added but failed to pin: {}", e)
                })),
            }
        }
        Err(e) => {
            println!("Failed to add file: {}", e);
            Json(json!({"error": format!("Failed to add file: {}", e)}))
        }
    }
}


pub async fn add_zip(
    Extension(ipfs_handler): Extension<Arc<Mutex<IPFSHandler>>>,
    Json(payload): Json<serde_json::Value>,
) -> Json<serde_json::Value> {
    let path = payload["path"].as_str().unwrap_or("");

    if path.is_empty() {
        return Json(json!({"error": "Path is required"}));
    }

    println!("Received .zip file for processing: {}", path);

    match ipfs_handler.lock().await.add_zip(path).await {
        Ok(cid) => Json(json!({"cid": cid, "status": "Folder added and pinned"})),
        Err(e) => Json(json!({"error": format!("Failed to process .zip file: {}", e)})),
    }
}



pub async fn add_folder(
    Extension(ipfs_handler): Extension<Arc<Mutex<IPFSHandler>>>,
    Json(payload): Json<serde_json::Value>,
) -> Json<serde_json::Value> {
    let path = payload["path"].as_str().unwrap_or("");

    if path.is_empty() {
        return Json(json!({"error": "Path is required"}));
    }

    println!("Received request to add folder. Path: {}", path);

    // Ensure the path is a directory
    let is_dir = match tokio::fs::metadata(path).await {
        Ok(metadata) => metadata.is_dir(),
        Err(_) => false,
    };

    if !is_dir {
        return Json(json!({"error": "The provided path is not a directory"}));
    }

    // Add the directory to IPFS
    let result = ipfs_handler.lock().await.add_folder(path).await;

    match result {
        Ok(cid) => {
            println!("Folder added successfully. CID: {}", cid);
            let pin_result = ipfs_handler.lock().await.pin_file(&cid).await;
            match pin_result {
                Ok(_) => Json(json!({"cid": cid, "status": "Folder added and pinned"})),
                Err(e) => Json(json!({"cid": cid, "warning": format!("Folder added but failed to pin: {}", e)})),
            }
        }
        Err(e) => {
            println!("Failed to add folder: {}", e);
            Json(json!({"error": format!("Failed to add folder: {}", e)}))
        }
    }
}


//Pin The File or Folder
pub async fn pin_file(
    Extension(ipfs_handler): Extension<Arc<Mutex<IPFSHandler>>>,
    Path(cid): Path<String>,
) -> Json<serde_json::Value> {
    match ipfs_handler.lock().await.pin_file(&cid).await {
        Ok(_) => Json(json!({"status": "Pinned successfully", "cid": cid})),
        Err(e) => Json(json!({"error": format!("Failed to pin file: {}", e)})),
    }
}

pub async fn unpin_file(
    Extension(ipfs_handler): Extension<Arc<Mutex<IPFSHandler>>>,
    Path(cid): Path<String>,
) -> Json<serde_json::Value> {
    match ipfs_handler.lock().await.unpin_file(&cid).await {
        Ok(_) => Json(json!({"status": "Unpinned successfully", "cid": cid})),
        Err(e) => Json(json!({"error": format!("Failed to unpin file: {}", e)})),
    }
}


pub async fn fetch_content(
    Extension(ipfs_handler): Extension<Arc<Mutex<IPFSHandler>>>,
    Path(cid): Path<String>,
) -> Json<serde_json::Value> {
    println!("Fetching content for CID: {}", cid);

    // Fetch the file and metadata
    let include_metadata = true; // Always fetch metadata
    match ipfs_handler.lock().await.get_file(&cid, include_metadata).await {
        Ok((file_content, metadata)) => {
            println!("Fetched content length: {}", file_content.len());

            // Save the content locally (optional)
            let save_path = format!("/tmp/{}", cid);
            if let Err(e) = tokio::fs::write(&save_path, &file_content).await {
                eprintln!("Failed to save content: {}", e);
            } else {
                println!("Content saved to: {}", save_path);
            }

            // Prepare the response with file content and metadata
            let response = json!({
                "status": "Content fetched successfully",
                "content_length": file_content.len(),
                "metadata": metadata, // Metadata is serialized automatically
                "saved_to": save_path,
            });
            Json(response)
        }
        Err(e) => {
            eprintln!("Failed to fetch content: {}", e);
            Json(json!({"error": format!("Failed to fetch content: {}", e)}))
        }
    }
}
