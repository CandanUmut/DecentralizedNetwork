use rsa::{RsaPrivateKey, RsaPublicKey};
use rand::rngs::OsRng;
use ipfs::PeerId;
use sha2::{Digest, Sha256};
use bs58; // Base58 encoding library
use rsa::pkcs1v15::{Pkcs1v15Sign, SigningKey, VerifyingKey, Signature};
use rsa::signature::{Signer, Verifier};
use hex;
use rsa::signature::SignatureEncoding;

/// Signs a message using PKCS#1 v1.5 padding and SHA-256
pub fn sign_message(private_key: &RsaPrivateKey, message: &[u8]) -> Vec<u8> {
    let signing_key = SigningKey::<Sha256>::new(private_key.clone());
    signing_key.sign(message).to_vec()
}

/// Verifies a signature using PKCS#1 v1.5 padding and SHA-256
pub fn verify_signature(public_key: &RsaPublicKey, message: &[u8], signature: &[u8]) -> bool {
    let verifying_key = VerifyingKey::<Sha256>::new(public_key.clone());
    match Signature::try_from(signature) {
        Ok(parsed_signature) => verifying_key.verify(message, &parsed_signature).is_ok(),
        Err(_) => false,
    }
}

/// Derives a PeerId from a public key
pub fn derive_peer_id(public_key: &str) -> PeerId {
    let hash = Sha256::digest(public_key.as_bytes());
    PeerId::from_bytes(&hash).expect("Invalid PeerID")
}

/// Derives a wallet address from a public key
pub fn derive_wallet_address(public_key: &str) -> String {
    format!("0x{}", hex::encode(Sha256::digest(public_key.as_bytes())))
}

/// Hashes transaction data for immutability
pub fn hash_transaction(sender: &str, receiver: &str, amount: u64, parents: &[String]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(format!("{}{}{}{:?}", sender, receiver, amount, parents));
    format!("{:x}", hasher.finalize())
}

/// Validates a transaction hash matches its contents
pub fn validate_transaction_hash(
    hash: &str,
    sender: &str,
    receiver: &str,
    amount: u64,
    parents: &[String],
) -> bool {
    let computed_hash = hash_transaction(sender, receiver, amount, parents);
    hash == computed_hash
}
