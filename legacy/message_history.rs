use libp2p::PeerId;
use std::collections::HashMap;
use std::sync::Mutex;
use chrono::Utc;
use lazy_static::lazy_static; // Import lazy_static here

// Lazy static global storage for CID history
lazy_static! {
    static ref CID_STORAGE: Mutex<HashMap<PeerId, Vec<(String, i64)>>> = Mutex::new(HashMap::new());
}

/// Store the CID with a timestamp associated with the PeerId
pub fn store_cid(peer: PeerId, cid: String) {
    let timestamp = Utc::now().timestamp(); // Current UTC timestamp
    let mut storage = CID_STORAGE.lock().unwrap();

    storage
        .entry(peer)
        .or_default()
        .push((cid.clone(), timestamp));

    println!(
        "✅ CID {} stored for Peer {:?} with Timestamp {}",
        cid, peer, timestamp
    );
}

/// Fetch all CIDs for a given PeerId, ordered by timestamp
pub fn get_cids_for_peer(peer: PeerId) -> Vec<(String, i64)> {
    let storage = CID_STORAGE.lock().unwrap();
    let mut entries = storage.get(&peer).cloned().unwrap_or_default();

    // Sort the CIDs by their timestamps
    entries.sort_by_key(|&(_, timestamp)| timestamp);
    entries
}
