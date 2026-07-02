use ipfs::PeerId;


#[derive(Debug)]
#[derive(Clone, Eq, Hash, PartialEq)]
pub struct IPFSPeerId {
    peer_id: PeerId,  // Unique peer identity
    balance: u64,     // Balance in TimeCoin
}



impl IPFSPeerId {
    // Constructor: Initialize a new IPFSPeerId with default balance (0)
    pub fn new(peer_id: PeerId) -> Self {
        Self {
            peer_id,
            balance: 0,
        }
    }


    pub fn from_base58(base58_str: &str) -> Result<Self, String> {
        let peer_id = libp2p::PeerId::from_bytes(
            &bs58::decode(base58_str).into_vec().map_err(|e| format!("Invalid Base58 encoding: {}", e))?
        )
        .map_err(|e| format!("Failed to convert PeerId from bytes: {}", e))?;
        Self::from_libp2p(peer_id)
    }
    
    // Convert from libp2p PeerId
    pub fn from_libp2p(peer_id: libp2p::PeerId) -> Result<Self, String> {
        PeerId::from_bytes(&peer_id.to_bytes())
            .map(|id| Self::new(id))
            .map_err(|e| format!("Failed to convert PeerId: {}", e))
    }

    // Convert back to libp2p PeerId
    pub fn to_libp2p(&self) -> libp2p::PeerId {
        libp2p::PeerId::from_bytes(&self.peer_id.to_bytes())
            .expect("Conversion back to libp2p::PeerId should not fail")
    }

    // Balance Management
    pub fn get_balance(&self) -> u64 {
        self.balance
    }

    pub fn add_balance(&mut self, amount: u64) {
        self.balance += amount;
    }

    pub fn subtract_balance(&mut self, amount: u64) -> Result<(), String> {
        if self.balance >= amount {
            self.balance -= amount;
            Ok(())
        } else {
            Err("Insufficient balance".to_string())
        }
    }

    // Display as base58 for easy identification
    pub fn to_base58(&self) -> String {
        self.peer_id.to_base58()
    }

    // Clone the inner PeerId
    pub fn clone_inner(&self) -> PeerId {
        self.peer_id.clone()
    }
}

// Display implementation for logging or outputs
impl std::fmt::Display for IPFSPeerId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.to_base58())
    }
}
