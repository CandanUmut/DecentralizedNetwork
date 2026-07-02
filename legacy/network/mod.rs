pub mod messaging;       // Messaging (send and receive)
pub mod messagingroutes; // Routes for messaging
pub mod myipfs;          // IPFS-related functions
pub mod routes;          // General routing
pub mod webrtc;          // WebRTC connections
pub mod utils;           // Logging and utility functions

// Re-export key functions for ease of use
pub use messagingroutes::{send_message, receive_message};
pub use myipfs::{add_file, fetch_content};
pub use routes::initialize_routes;
pub use webrtc::start_webrtc_connection;
pub use utils::{log_info, log_error};
