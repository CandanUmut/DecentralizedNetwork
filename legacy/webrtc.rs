use std::collections::HashMap;
use std::sync::Arc;
use webrtc::peer_connection::RTCPeerConnection;
use webrtc::peer_connection::configuration::RTCConfiguration;
use webrtc::peer_connection::peer_connection_state::RTCPeerConnectionState;
use webrtc::ice_transport::ice_candidate::RTCIceCandidate;
use webrtc::api::{APIBuilder, API};
use tokio::sync::mpsc;

#[derive(Debug)]
pub struct WebRTCHandler {
    pub api: APIWrapper, // Wrap API to provide Debug
    pub peers: HashMap<String, Arc<RTCPeerConnection>>,
    pub event_sender: mpsc::Sender<WebRTCEvent>,
}

pub struct APIWrapper(API);

impl APIWrapper {
    pub fn new() -> Self {
        APIWrapper(APIBuilder::new().build())
    }

    pub fn inner(&self) -> &API {
        &self.0
    }
}

// Implement Debug manually to avoid requiring Debug on API
impl std::fmt::Debug for APIWrapper {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("APIWrapper")
            .field("API", &"<hidden>")
            .finish()
    }
}

#[derive(Debug)]
pub enum WebRTCEvent {
    PeerConnectionEstablished(String),
    PeerConnectionStateChanged(String, RTCPeerConnectionState),
    ICECandidate(String, String),
}

impl WebRTCHandler {
    /// Creates a new WebRTCHandler instance
    pub fn new() -> (Self, mpsc::Receiver<WebRTCEvent>) {
        let api = APIWrapper::new();
        let (event_sender, event_receiver) = mpsc::channel(100);
        (
            WebRTCHandler {
                api,
                peers: HashMap::new(),
                event_sender,
            },
            event_receiver,
        )
    }

    /// Creates a new peer connection and adds it to the peer list
    pub async fn create_peer_connection(
        &mut self,
        peer_id: &str,
    ) -> Result<Arc<RTCPeerConnection>, Box<dyn std::error::Error>> {
        let config = RTCConfiguration::default();
        let peer_connection = Arc::new(self.api.inner().new_peer_connection(config).await?);

        // Handle ICE candidates
        let peer_id_clone = peer_id.to_string();
        let event_sender_clone = self.event_sender.clone();
        peer_connection.on_ice_candidate(Box::new(move |candidate| {
            let peer_id = peer_id_clone.clone();
            let event_sender = event_sender_clone.clone();
            Box::pin(async move {
                if let Some(c) = candidate {
                    match c.to_json() {
                        Ok(json) => {
                            let _ = event_sender
                                .send(WebRTCEvent::ICECandidate(peer_id, json.candidate))
                                .await;
                        }
                        Err(e) => {
                            eprintln!("Error serializing ICE candidate: {:?}", e);
                        }
                    }
                }
            })
        }));

        // Monitor connection state
        let peer_id_clone = peer_id.to_string();
        let event_sender_clone = self.event_sender.clone();
        peer_connection.on_peer_connection_state_change(Box::new(move |state| {
            let peer_id = peer_id_clone.clone();
            let event_sender = event_sender_clone.clone();
            Box::pin(async move {
                let _ = event_sender
                    .send(WebRTCEvent::PeerConnectionStateChanged(peer_id, state))
                    .await;
            })
        }));

        self.peers
            .insert(peer_id.to_string(), Arc::clone(&peer_connection));
        let _ = self
            .event_sender
            .send(WebRTCEvent::PeerConnectionEstablished(peer_id.to_string()))
            .await;
        Ok(peer_connection)
    }
}
