use axum::{
    extract::{Extension, Path},
    response::Html,
    Json,
};
use axum::{response::IntoResponse};
use axum::extract::Json as MsgJson;

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

    if path.is_empty() {
        return Json(json!({"error": "Path is required"}));
    }

    println!("Received request to add file. Path: {}, Content: {:?}", path, content_base64);

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
                let cid_result = ipfs_handler.lock().await.add_file(&temp_path, None).await;
                let _ = tokio::fs::remove_file(&temp_path).await;
                cid_result
            }
            Err(e) => Err(anyhow::anyhow!(format!("Failed to write temporary file: {}", e))),
        }
    } else {
        println!("Adding file directly from path: {}", path);
        ipfs_handler.lock().await.add_file(path, None).await
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

    // Fetch the file
    let content = ipfs_handler.lock().await.get_file(&cid).await;
    match content { 
        Ok(data) => {
            println!("Fetched content length: {}", data.len());
            // Save the content locally for debugging (optional)
            let save_path = format!("/tmp/{}", cid);
            match tokio::fs::write(&save_path, &data).await {
                Ok(_) => println!("Content saved to: {}", save_path),
                Err(e) => eprintln!("Failed to save content: {}", e),
            }
            Json(json!({"status": "Content fetched successfully", "saved_to": save_path}))
        }
        Err(e) => {
            eprintln!("Failed to fetch content: {}", e);
            Json(json!({"error": format!("Failed to fetch content: {}", e)}))
        }
    }
}
