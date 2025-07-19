use std::sync::Arc;
use tokio::sync::mpsc::Sender;
use libp2p::Swarm;

/// Represents the shared state of the network
pub struct NetworkState {
    pub swarm: Arc<Swarm>,                // Shared Swarm instance
    pub ipfs_handler: Arc<IPFSHandler>,   // IPFS handler
    pub message_tx: Sender<String>,       // Message sender channel
}

impl NetworkState {
    pub fn new(swarm: Swarm, ipfs_handler: IPFSHandler, message_tx: Sender<String>) -> Self {
        Self {
            swarm: Arc::new(swarm),
            ipfs_handler: Arc::new(ipfs_handler),
            message_tx,
        }
    }
}
