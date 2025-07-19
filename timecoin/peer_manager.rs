use std::collections::{HashMap, HashSet};
use crate::timecoin::peer_id::IPFSPeerId;
use crate::timecoin::utils::{hash_transaction, validate_transaction_hash, sign_message, verify_signature};
use rsa::{RsaPrivateKey, RsaPublicKey};
use sha2::{Digest, Sha256}; // For transaction hashing
use serde_json::json;
use axum::http::StatusCode;
use axum::Json;
use axum::response::IntoResponse;
use axum::Extension; // For wrapping shared state
use std::sync::Arc; // For reference counting
use tokio::sync::Mutex; // For asynchronous locking
use std::collections::VecDeque;






#[derive(Clone, Debug)]
pub struct TransactionRequest {
    pub sender: IPFSPeerId,
    pub receiver: IPFSPeerId,
    pub amount: u64,
    pub parents: HashSet<String>, // Add this field for parent transaction hashes
    pub private_key: RsaPrivateKey,
    pub public_key: RsaPublicKey,
}
/// Represents a single TimeCoin transaction
pub struct Transaction {
    pub hash: String,          // Unique transaction ID (hash)
    pub sender: String,        // Wallet address of sender
    pub receiver: String,      // Wallet address of receiver
    pub amount: u64,           // Amount of TimeCoin transferred
    pub parents: HashSet<String>, // Parent transaction hashes
    pub signature: Vec<u8>,    // Cryptographic signature
    pub public_key: RsaPublicKey, // Sender's public key for verification
}

impl Transaction {
    /// Create a new transaction, compute its hash, and sign it
    pub fn new(
        sender: String,
        receiver: String,
        amount: u64,
        parents: HashSet<String>,
        private_key: &RsaPrivateKey,
        public_key: RsaPublicKey,
    ) -> Self {
        // Convert parents HashSet to Vec for hashing
        let parents_vec: Vec<String> = parents.iter().cloned().collect();
        let hash = hash_transaction(&sender, &receiver, amount, &parents_vec);

        // Sign the transaction hash
        let signature = sign_message(private_key, hash.as_bytes());

        Self {
            hash,
            sender,
            receiver,
            amount,
            parents,
            signature,
            public_key,
        }
    }

    /// Verify the integrity and authenticity of the transaction
    pub fn is_valid(&self) -> bool {
        // Convert parents HashSet to Vec for validation
        let parents_vec: Vec<String> = self.parents.iter().cloned().collect();

        // Validate the hash integrity
        let valid_hash = validate_transaction_hash(&self.hash, &self.sender, &self.receiver, self.amount, &parents_vec);

        // Verify the signature
        let valid_signature = verify_signature(&self.public_key, self.hash.as_bytes(), &self.signature);

        valid_hash && valid_signature
    }
}

/// Struct to manage PeerID ↔ Wallet Address mappings and TimeCoin transactions
#[derive(Default)]
pub struct PeerManager {
    peer_map: HashMap<IPFSPeerId, String>, // Maps PeerID to Wallet Address
    transactions: HashMap<String, Transaction>, // DAG of transactions
    transaction_queue: VecDeque<TransactionRequest>, // Queue of pending transactions
}

impl PeerManager {
    pub fn new() -> Self {
        Self::default()
    }


    

    /// Add a new Peer ↔ Wallet mapping
    pub fn add_mapping(&mut self, peer_id: IPFSPeerId, wallet_address: String) {
        self.peer_map.insert(peer_id, wallet_address);
    }

    pub fn bootstrap_transactions(&mut self) {
        // Generate keys for the genesis transactions
        let private_key = RsaPrivateKey::new(&mut rand::rngs::OsRng, 2048).unwrap();
        let public_key = RsaPublicKey::from(&private_key);

        // Create the first genesis transaction
        let genesis_transaction_1 = Transaction::new(
            "System".to_string(),
            "SystemReceiver1".to_string(),
            0, // No actual amount
            HashSet::new(), // No parents
            &private_key,
            public_key.clone(),
        );

        // Create the second genesis transaction, referencing the first one
        let genesis_transaction_2 = Transaction::new(
            "System".to_string(),
            "SystemReceiver2".to_string(),
            0, // No actual amount
            HashSet::from([genesis_transaction_1.hash.clone()]), // First transaction as parent
            &private_key,
            public_key.clone(),
        );

        // Insert genesis transactions into the DAG
        self.transactions.insert(genesis_transaction_1.hash.clone(), genesis_transaction_1);
        self.transactions.insert(genesis_transaction_2.hash.clone(), genesis_transaction_2);

        println!("Bootstrap transactions added to the DAG.");
    }


    

    /// Remove a peer
    pub fn remove_peer(&mut self, peer_id: &IPFSPeerId) {
        self.peer_map.remove(peer_id);
    }

    /// Get wallet address by PeerID
    pub fn get_wallet(&self, peer_id: &IPFSPeerId) -> Option<&String> {
        self.peer_map.get(peer_id)
    }

    /// List all registered peers
    pub fn list_peers(&self) -> Vec<IPFSPeerId> {
        self.peer_map.keys().cloned().collect()
    }
    pub fn is_registered(&self, peer_id: &IPFSPeerId) -> bool {
        self.peer_map.contains_key(peer_id)
    }

    /// List all transactions in the DAG
    pub fn list_transactions(&self) -> Vec<&Transaction> {
        self.transactions.values().collect()
    }

    pub fn enqueue_transaction(&mut self, request: TransactionRequest) {
        self.transaction_queue.push_back(request.clone());
        println!(
            "Transaction added to the queue with parents: {:?}. Queue size: {}",
            request.parents,
            self.transaction_queue.len()
        );
    }
    /// Process the next transaction in the queue
    pub fn process_transaction_queue(&mut self) -> Result<String, String> {
        if let Some(request) = self.transaction_queue.pop_front() {
            // Ensure all parent transactions exist in the DAG
            let parents_exist = request
                .parents
                .iter()
                .all(|parent_hash| self.transactions.contains_key(parent_hash));
    
            if !parents_exist {
                return Err("Some parent transactions are missing".to_string());
            }
    
            // Attempt to create and validate a new transaction
            let sender_wallet = self.peer_map.get(&request.sender).ok_or("Sender not registered")?;
            let receiver_wallet = self.peer_map.get(&request.receiver).ok_or("Receiver not registered")?;
    
            // Create a new transaction
            let transaction = Transaction::new(
                sender_wallet.clone(),
                receiver_wallet.clone(),
                request.amount,
                request.parents.clone(),
                &request.private_key,
                request.public_key.clone(),
            );
    
            // Validate the transaction
            if !transaction.is_valid() {
                return Err("Invalid transaction".to_string());
            }
    
            // Insert the transaction into the DAG
            self.transactions.insert(transaction.hash.clone(), transaction);
            println!("Transaction processed successfully.");
            Ok("Transaction processed successfully.".to_string())
        } else {
            Err("No transactions in the queue".to_string())
        }
    }
    

    /// Create and validate a transaction
    pub fn create_transaction(
        &mut self,
        sender: &str,
        receiver: &str,
        amount: u64,
        private_key: &RsaPrivateKey,
        public_key: RsaPublicKey,
    ) -> Result<String, String> {
        // Validate sender's balance
        let sender_peer = self
            .peer_map
            .iter()
            .find(|(_, wallet)| wallet.as_str() == sender)
            .map(|(peer_id, _)| peer_id)
            .ok_or("Sender not registered")?;
    
        let mut sender_peer_mut = sender_peer.clone();
        if sender_peer_mut.get_balance() < amount {
            return Err("Insufficient balance".to_string());
        }
    
        // Deduct balance and credit receiver
        sender_peer_mut.subtract_balance(amount);
    
        let receiver_peer = self
            .peer_map
            .iter()
            .find(|(_, wallet)| wallet.as_str() == receiver)
            .map(|(peer_id, _)| peer_id)
            .ok_or("Receiver not registered")?;
    
        let mut receiver_peer_mut = receiver_peer.clone();
        receiver_peer_mut.add_balance(amount);
    
        // Create the transaction
        let parents: HashSet<String> = self.transactions.keys().cloned().collect();
        let transaction = Transaction::new(
            sender.to_string(),
            receiver.to_string(),
            amount,
            parents,
            private_key,
            public_key,
        );
    
        // Verify transaction integrity and authenticity
        if !transaction.is_valid() {
            eprintln!(
                "Transaction validation failed: sender={} receiver={} amount={} hash={}",
                transaction.sender, transaction.receiver, transaction.amount, transaction.hash
            );
            return Err("Transaction failed verification".to_string());
        }
    
        // Add transaction to DAG
        self.transactions.insert(transaction.hash.clone(), transaction);
    
        Ok("Transaction successful!".to_string())
    }
    

   
    pub fn send_timecoin_logic(
        &mut self,
        sender_peer_id: IPFSPeerId,
        receiver_peer_id: IPFSPeerId,
        amount: u64,
        private_key: &RsaPrivateKey,
        public_key: RsaPublicKey,
    ) -> Result<serde_json::Value, serde_json::Value> {
        // Extract sender wallet
        let sender_wallet = self
            .get_wallet(&sender_peer_id)
            .cloned()
            .ok_or_else(|| json!({ "error": format!("Sender peer {} not registered", sender_peer_id) }))?;
    
        // Extract receiver wallet
        let receiver_wallet = self
            .get_wallet(&receiver_peer_id)
            .cloned()
            .ok_or_else(|| json!({ "error": format!("Receiver peer {} not registered", receiver_peer_id) }))?;
    
        // Check sender balance
        let sender_balance = sender_wallet.parse::<u64>().unwrap_or(0);
        if sender_balance < amount {
            return Err(json!({
                "error": format!(
                    "Insufficient balance for sender: {}, required: {}",
                    sender_wallet, amount
                ),
            }));
        }
    
        // Prepare the transaction request for the queue
        let transaction_request = TransactionRequest {
            sender: sender_peer_id.clone(),
            receiver: receiver_peer_id.clone(),
            amount,
            parents: HashSet::new(), // Initialize with an empty HashSet for now
            private_key: private_key.clone(),
            public_key,
        };
    
        // Enqueue the transaction
        self.enqueue_transaction(transaction_request);
    
        // Process the transaction queue
        match self.process_transaction_queue() {
            Ok(message) => Ok(json!({
                "status": "success",
                "message": message,
            })),
            Err(error) => Err(json!({
                "error": format!("Transaction processing failed: {}", error),
            })),
        }
    }
    
    
    

    pub fn reward_timecoin(
        &mut self,
        peer_id: IPFSPeerId,
        amount: u64,
    ) -> Result<serde_json::Value, serde_json::Value> {
        if let Some(wallet) = self.peer_map.get_mut(&peer_id) {
            // Parse and update balance
            let current_balance = wallet.parse::<u64>().unwrap_or(0);
            *wallet = (current_balance + amount).to_string();

            Ok(json!({
                "status": "success",
                "peer_id": peer_id.to_string(),
                "amount_rewarded": amount,
                "new_balance": wallet.clone(),
            }))
        } else {
            Err(json!({
                "status": "failure",
                "error": format!("Peer {} not registered", peer_id),
            }))
        }
    }
    


    pub fn get_balance(&self, peer_id: &IPFSPeerId) -> Result<serde_json::Value, serde_json::Value> {
        if let Some(wallet) = self.peer_map.get(peer_id) {
            match wallet.parse::<u64>() {
                Ok(balance) => Ok(json!({
                    "status": "success",
                    "peer_id": peer_id.to_string(),
                    "balance": balance,
                })),
                Err(_) => Err(json!({
                    "status": "failure",
                    "error": "Failed to parse balance as u64",
                })),
            }
        } else {
            Err(json!({
                "status": "failure",
                "error": format!("Peer {} not registered", peer_id),
            }))
        }
    }


    
}
