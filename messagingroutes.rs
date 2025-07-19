use axum::{
    extract::{Extension, Path},
    response::Html,
    Json,
    http::StatusCode,
};
use axum::{response::IntoResponse};

use axum::debug_handler; // Make sure to add this line
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

// pub use self::{send_message};

// pub async fn get_peers(
//     Extension(swarm): Extension<Arc<Mutex<Swarm<MyBehaviour>>>>,
// ) -> Json<Vec<String>> {
//     let swarm = swarm.lock().await;
//     let peers: Vec<String> = swarm.connected_peers().map(|peer| peer.to_string()).collect();
//     Json(peers)
// }


#[debug_handler]
pub async fn send_message(
    
    Extension(swarm): Extension<Arc<Mutex<Swarm<MyBehaviour>>>>, 
    Json(payload): Json<Value>, 
) -> impl IntoResponse {
    // Extract and validate peer_id and message
    let peer_id_str = payload.get("peer_id").and_then(|v| v.as_str()).unwrap_or("");
    let message = payload.get("message").and_then(|v| v.as_str()).unwrap_or("");

    if peer_id_str.is_empty() || message.is_empty() {
        return (StatusCode::BAD_REQUEST, Json(json!({ "error": "peer_id and message are required" })));
    }

    // Parse PeerId
    let peer_id = match PeerId::from_str(peer_id_str) {
        Ok(id) => id,
        Err(_) => {
            return (
                StatusCode::BAD_REQUEST,
                Json(json!({ "error": "Invalid peer_id format" })),
            );
        }
    };

    // Lock the swarm and send the message
    let mut swarm = swarm.lock().await;
    let request_id = swarm
        .behaviour_mut()
        .messaging
        .send_request(&peer_id, message.as_bytes().to_vec());

    // Respond with success
    (
        StatusCode::OK,
        Json(json!({
            "status": "Message sent",
            "peer_id": peer_id.to_string(),
            "message": message,
            "request_id": format!("{:?}", request_id),
        })),
    )
}


// // pub async fn receive_messages(
// //     Extension(message_rx): Extension<Arc<Mutex<Receiver<(String, String)>>>>,
// // ) -> Json<Vec<(String, String)>> {
// //     let mut rx = message_rx.lock().await;
// //     let mut messages = Vec::new();

// //     while let Ok(msg) = rx.try_recv() {
// //         messages.push(msg);
// //     }

// //     Json(messages)
// // }
// // /// Connect to a specific peer
// // pub async fn connect_peer(
// //     MsgJson(payload): MsgJson<Value>, // Use the new MessagingJson alias
// //     Extension(swarm): Extension<Arc<Mutex<Swarm<MyBehaviour>>>>,
// // ) -> Json<Value> {
// //     let address = payload["address"].as_str().unwrap_or_default();

// //     if address.is_empty() {
// //         return Json(json!({"error": "address is required"}));
// //     }

// //     let multiaddr = match address.parse::<Multiaddr>() {
// //         Ok(addr) => addr,
// //         Err(_) => return Json(json!({"error": "Invalid address"})),
// //     };

// //     let mut swarm = swarm.lock().await;
// //     match swarm.dial(multiaddr.clone()) {
// //         Ok(_) => Json(json!({"status": "Dial initiated", "address": address})),
// //         Err(err) => Json(json!({"error": format!("Failed to dial: {}", err)})),
// //     }
// // }


