// ipfs.rs

use bs58; // For Base58 encoding
use anyhow::{anyhow, Context, Result};
// Import timecoin modules
use crate::timecoin::peer_id::IPFSPeerId;
use crate::timecoin::peer_manager::PeerManager;
use crate::timecoin::utils::derive_wallet_address;
use tracing::{info, error};
use tokio::process::Command;
use ipfs::{
    Block, Ipfs, IpfsOptions, IpfsPath, TestTypes, UninitializedIpfs, MultiaddrWithoutPeerId,
    MultiaddrWithPeerId, Connection, PeerId, Protocol,
};
use tokio::fs::{self, read_dir};
use std::collections::VecDeque;
use std::env;
use std::pin::Pin;
use ipfs::p2p::{TSwarm};
use libp2p::PeerId as Libp2pPeerId;
use std::path::PathBuf;
use ipfs::repo::RepoOptions;
use ipfs::p2p::transport::build_transport;
use ipfs::repo::Repo; // Ensure Repo is correctly imported
use ipfs::p2p::SwarmOptions;
use ipfs::p2p::create_swarm;
use tracing::Span;
use tokio::sync::Mutex;
use tokio::{task, time::{sleep, Duration}};
use futures::StreamExt; // For handling streams
use cid::{Cid, Codec as CidCodec};
use multihash::{Code as MultihashCode};
use sha2::{Digest as Sha2Digest, Sha256};
use std::sync::Arc;
use ipfs_api::IpfsClient;
use std::str::FromStr;
use ipfs::{Multiaddr};
use libp2p::quic::tokio::Transport as QuicTransport;
use libp2p::quic::Config as QuicConfig;
use ipfs_unixfs::file::adder::FileAdder;




#[derive(Clone)]
pub struct IPFSHandler {
    ipfs: Ipfs<TestTypes>, // IPFS node instance
    pub peer_manager: Arc<Mutex<PeerManager>>,
}

fn extract_peer_id_from_multiaddr(addr: &MultiaddrWithoutPeerId) -> Option<ipfs::PeerId> {
    let addr: Multiaddr = addr.clone().into(); // Convert MultiaddrWithoutPeerId to Multiaddr
    for component in addr.iter() {
        if let Protocol::P2p(peer_id_bytes) = component {
            // Convert bytes to ipfs::PeerId
            return ipfs::PeerId::from_multihash(peer_id_bytes.clone()).ok();
        }
    }
    None
}


impl IPFSHandler {
    fn get_bootstrap_nodes() -> Vec<(Multiaddr, PeerId)> {
        vec![
            
            
           
            
            (
                "/ip4/104.131.131.82/tcp/4001/p2p/QmaCpDMGvV2BGHeYERUEnRQAwe3N8SzbUtfsmvsqQLuvuJ"
                    .parse::<Multiaddr>()
                    .expect("Invalid Multiaddr for bootstrap node"),
                "QmaCpDMGvV2BGHeYERUEnRQAwe3N8SzbUtfsmvsqQLuvuJ"
                    .parse::<PeerId>()
                    .expect("Invalid PeerId for bootstrap node"),
            ),
            (
                "/ip4/104.131.131.82/udp/4001/quic/p2p/QmaCpDMGvV2BGHeYERUEnRQAwe3N8SzbUtfsmvsqQLuvuJ"
                    .parse::<Multiaddr>()
                    .expect("Invalid Multiaddr for bootstrap node"),
                "QmaCpDMGvV2BGHeYERUEnRQAwe3N8SzbUtfsmvsqQLuvuJ"
                    .parse::<PeerId>()
                    .expect("Invalid PeerId for bootstrap node"),
            ),
        ]
    }


    pub async fn refresh(&self) -> Result<(), anyhow::Error> {
        println!("🔧 Refreshing IPFS node...");
    
        // Step 1: Fetch active peers
        let peers = self.ipfs.peers().await?;
        println!("👥 Active peers: {}", peers.len());
    
        // Step 2: Reconnect to bootstrap nodes if peer count is low
        if peers.len() < 10 {
            println!("📡 Reconnecting to bootstrap nodes...");
            for (multiaddr, peer_id) in Self::get_bootstrap_nodes() {
                // Convert Multiaddr to MultiaddrWithoutPeerId
                let multiaddr_without_peer_id = MultiaddrWithoutPeerId::try_from(multiaddr)
                    .expect("Failed to convert Multiaddr to MultiaddrWithoutPeerId");
            
                // Create MultiaddrWithPeerId
                let peer_with_id = MultiaddrWithPeerId {
                    multiaddr: multiaddr_without_peer_id,
                    peer_id: peer_id.clone(),
                };
            
                // Attempt to connect
                match self.ipfs.connect(peer_with_id).await {
                    Ok(_) => println!("✅ Connected to bootstrap peer: {}", peer_id),
                    Err(e) => eprintln!("❌ Failed to connect to bootstrap peer {}: {:?}", peer_id, e),
                }
            }
        }
    
        Ok(())
    }


    pub async fn initialize_ipfs_daemon() {
        let os = env::consts::OS;
    
        if os == "windows" {
            // Check if IPFS daemon is running on Windows
            let is_running = Command::new("tasklist")
                .arg("/FI")
                .arg("IMAGENAME eq ipfs.exe")
                .output()
                .await
                .map(|output| {
                    let stdout = String::from_utf8_lossy(&output.stdout);
                    stdout.contains("ipfs.exe")
                })
                .unwrap_or(false);
    
            if is_running {
                println!("🔍 Found an existing IPFS daemon instance. Attempting to terminate it...");
    
                // Try to kill the IPFS daemon
                if let Err(e) = Command::new("taskkill")
                    .arg("/F")
                    .arg("/IM")
                    .arg("ipfs.exe")
                    .output()
                    .await
                {
                    eprintln!("⚠️ Warning: Failed to terminate IPFS daemon: {:?}", e);
                } else {
                    println!("✅ Successfully terminated the existing IPFS daemon.");
                }
            } else {
                println!("ℹ️ No running IPFS daemon found. Proceeding...");
            }
    
            // Start a new IPFS daemon (non-fatal if it fails)
            println!("🚀 Attempting to start a new IPFS daemon instance...");
            if let Err(e) = Command::new("cmd.exe")
                .args(&["/C", "ipfs daemon"])
                .spawn()
            {
                eprintln!("⚠️ Warning: Failed to start IPFS daemon: {:?}", e);
            } else {
                println!("✅ IPFS daemon start attempted. It might still be initializing...");
            }
    
        } else {
            // Unix-based systems
            let is_running = Command::new("pgrep")
                .arg("-f")
                .arg("ipfs daemon")
                .output()
                .await
                .map(|output| output.status.success())
                .unwrap_or(false);
    
            if is_running {
                println!("🔍 Found an existing IPFS daemon instance. Attempting to terminate it...");
    
                if let Err(e) = Command::new("pkill")
                    .arg("-f")
                    .arg("ipfs daemon")
                    .output()
                    .await
                {
                    eprintln!("⚠️ Warning: Failed to terminate IPFS daemon: {:?}", e);
                } else {
                    println!("✅ Successfully terminated the existing IPFS daemon.");
                }
            } else {
                println!("ℹ️ No running IPFS daemon found. Proceeding...");
            }
    
            println!("🚀 Attempting to start a new IPFS daemon instance...");
            if let Err(e) = Command::new("ipfs")
                .arg("daemon")
                .spawn()
            {
                eprintln!("⚠️ Warning: Failed to start IPFS daemon: {:?}", e);
            } else {
                println!("✅ IPFS daemon start attempted. It might still be initializing...");
            }
        }
    
        // Graceful wait to allow daemon to initialize (optional)
        sleep(Duration::from_secs(5)).await;
        println!("ℹ️ Initialization function complete. Proceeding...");
    }

    /// Initializes a new IPFSHandler instance.
    pub async fn new(peer_manager: Arc<Mutex<PeerManager>>) -> Result<Self> {
        
        Self::initialize_ipfs_daemon().await;
        // Create IPFS options
        let mut opts = IpfsOptions::inmemory_with_generated_keys();
        
        opts.mdns = true;
        opts.bootstrap = Self::get_bootstrap_nodes();
    
        // Use the build_transport function
        let keypair = opts.keypair.clone();
        let transport = build_transport(keypair.clone())
            .expect("Failed to build transport");
    
        // Set up SwarmOptions
        let swarm_options = SwarmOptions {
            keypair: keypair.clone(), // Clone here to avoid moving
            peer_id: keypair.public().to_peer_id(),
            bootstrap: opts.bootstrap.clone(),
            mdns: opts.mdns,
            kad_protocol: opts.kad_protocol.clone(),
        };
    
        // Create RepoOptions
        let repo_options = RepoOptions::from(&opts); // Use the `from` method to initialize RepoOptions
    
        // Create the Repo instance using RepoOptions
        let (repo, _event_receiver) = Repo::<TestTypes>::new(repo_options);
    
        // Create the Swarm
        let mut swarm = create_swarm(swarm_options, Span::current(), Arc::new(repo)).await?;
        
        // Bind the swarm to a listening address
        let listen_addr = "/ip4/127.0.0.1/tcp/0".parse()?;
        TSwarm::listen_on(&mut swarm, listen_addr).map_err(|e| anyhow::anyhow!("Failed to start listening: {:?}", e))?;
    
        // Start the IPFS node
        let (ipfs, fut) = UninitializedIpfs::new(opts).start().await?;
        tokio::task::spawn(fut);
    
        println!("IPFS Node initialized with DHT and bootstrap nodes.");
    
        // Log the listening addresses
        let listeners = TSwarm::listeners(&swarm).cloned().collect::<Vec<_>>();
        if listeners.is_empty() {
            eprintln!("No active listeners found!");
        } else {
            for addr in listeners {
                println!("Node is listening on: {}", addr);
            }
        }
        let connected_peers = TSwarm::connected_peers(&swarm).collect::<Vec<_>>();
        if connected_peers.is_empty() {
            println!("No peers are currently connected.");
        } else {
            println!("Connected peers: {:?}", connected_peers);
        }

    
        let handler = Self {
            ipfs: ipfs.clone(),
            peer_manager,
        };
    
        // Start periodic peer discovery
        let handler_clone = Arc::new(Mutex::new(handler.clone()));
        tokio::task::spawn(Self::start_peer_discovery(handler_clone));
    
        Ok(handler)
    }

    pub async fn add_peers(&mut self, peers: Vec<(PeerId, Multiaddr)>) -> Result<()> {
        for (peer_id, addr) in peers {
            println!("Connecting to IPFS peer: {} at address: {}", peer_id, addr);

            // Strip PeerId from Multiaddr to get MultiaddrWithoutPeerId
            let multiaddr_without_peer_id = MultiaddrWithoutPeerId::try_from(addr.clone()).map_err(|e| {
                eprintln!("Failed to convert Multiaddr to MultiaddrWithoutPeerId: {:?}", e);
                e
            })?;

            // Combine MultiaddrWithoutPeerId and PeerId into MultiaddrWithPeerId
            let target = MultiaddrWithPeerId {
                multiaddr: multiaddr_without_peer_id,
                peer_id,
            };

            // Use the `connect` method to connect to the peer
            if let Err(e) = self.ipfs.connect(target).await {
                eprintln!("Failed to connect to IPFS peer {}: {:?}", peer_id, e);
            } else {
                println!("Successfully connected to IPFS peer: {}", peer_id);
            }
        }
        Ok(())
    }

    /// Starts periodic peer discovery.
    async fn start_peer_discovery(handler: Arc<Mutex<Self>>) {
        let refresh_interval = Duration::from_secs(300); // 5 minutes
        loop {
            println!("Starting peer discovery...");
            if let Err(e) = handler.lock().await.discover_peers().await {
                eprintln!("Peer discovery error: {:?}", e);
            }
            sleep(refresh_interval).await;
        }
    }

    /// Boots up the DHT with predefined bootstrap nodes.
    pub async fn bootstrap(&self) -> Result<()> {
        for (addr, peer_id) in Self::get_bootstrap_nodes() {
            let addr_with_peer_id = MultiaddrWithPeerId {
                multiaddr: addr.clone().try_into().expect("Invalid Multiaddr format"),
                peer_id,
            };
            match self.ipfs.add_bootstrapper(addr_with_peer_id).await {
                Ok(_) => println!("Connected to bootstrap node: {}", peer_id),
                Err(e) => eprintln!("Failed to add bootstrapper {}: {:?}", peer_id, e),
            }
        }
        println!("Bootstrap nodes added.");
        Ok(())
    }

    pub async fn discover_peers(&self) -> Result<()> {
        // Get the public key and addresses
        let (public_key, _) = self.ipfs.identity().await?;
        // Convert public key to PeerId
        let local_peer_id: PeerId = public_key.into();
    
        match self.ipfs.get_closest_peers(local_peer_id).await {
            Ok(peers) if !peers.is_empty() => {
                println!("Discovered peers: {:?}", peers);
                let mut manager = self.peer_manager.lock().await;
    
                for peer_id in peers {
                    // Convert ipfs::PeerId to libp2p::PeerId
                    let libp2p_peer_id = libp2p::PeerId::from_bytes(&peer_id.to_bytes())
                        .map_err(|e| anyhow::anyhow!("failed to convert peer_id: {:?}", e))?;
                
                    // Convert the libp2p::PeerId into IPFSPeerId, remapping the error
                    let ipfs_peer_id = IPFSPeerId::from_libp2p(libp2p_peer_id)
                        .map_err(|err_str| anyhow::anyhow!(err_str))?;
                
                    if !manager.is_registered(&ipfs_peer_id) {
                        let wallet_address = derive_wallet_address(&ipfs_peer_id.to_base58());
                        manager.add_mapping(ipfs_peer_id.clone(), wallet_address.clone());
                        println!(
                            "Registered peer for IPFS: {} with wallet: {}",
                            ipfs_peer_id, wallet_address
                        );
                    }
                }
            }
            Ok(_) => println!("No peers found."),
            Err(e) => eprintln!("Error during peer discovery: {:?}", e),
        }
    
        Ok(())
    }
    /// Retrieves closest peers to a given target PeerId.
    pub async fn get_closest_peers(&self, target: PeerId) -> Result<Vec<PeerId>> {
        let multiaddrs = self.ipfs.find_peer(target).await?;
        let peer_ids = multiaddrs
            .into_iter()
            .filter_map(|addr| {
                TryInto::<MultiaddrWithPeerId>::try_into(addr)
                    .ok()
                    .map(|addr_with_peer_id| addr_with_peer_id.peer_id)
            })
            .collect::<Vec<_>>();

        Ok(peer_ids)
    }


    pub async fn get_providers(&self, cid: Cid) -> Result<Vec<PeerId>> {
        let providers = self.ipfs.get_providers(cid.clone()).await?; // Clone cid here
        println!("Providers for CID {}: {:?}", cid, providers);
        Ok(providers)
    }
    


    /// Adds a file to IPFS and returns its CID.
    ///
    /// # Arguments
    ///
    /// * `path` - The file system path to the file to be added.
    ///
    /// # Errors
    ///



    pub async fn add_file(&self, path: &str, content_base64: Option<&str>) -> Result<String> {
        // Step 1: Read file content or decode Base64
        let file_content = if let Some(base64_content) = content_base64 {
            base64::decode(base64_content).map_err(|e| anyhow::anyhow!("Base64 decoding failed: {}", e))?
        } else {
            fs::read(path).await.map_err(|e| anyhow::anyhow!("Failed to read file '{}': {}", path, e))?
        };
    
        println!(
            "File content length for CID calculation: {} bytes",
            file_content.len()
        );
    
        // Step 2: Initialize FileAdder for UnixFS chunking
        let mut file_adder = FileAdder::default();
        let mut consumed_offset = 0;
        let mut blocks = Vec::new();
    
        // Step 3: Process file content in chunks
        while consumed_offset < file_content.len() {
            let remaining = &file_content[consumed_offset..];
            let (iter, consumed) = file_adder.push(remaining);
    
            // Add consumed bytes to offset
            consumed_offset += consumed;
    
            // Collect finalized blocks
            for (cid, block) in iter {
                println!("Generated block CID: {}", cid);
                blocks.push((cid, block));
            }
        }
    
        // Step 4: Finalize FileAdder and collect remaining blocks
        for (cid, block) in file_adder.finish() {
            println!("Generated final block CID: {}", cid);
            blocks.push((cid, block));
        }
    
        // Step 5: Extract the root block and CID
        let (root_cid, root_block) = blocks
            .pop()
            .ok_or_else(|| anyhow::anyhow!("No blocks were generated for the file"))?;
    
        println!("Root CID (CIDv1): {}", root_cid);
    
        // Step 6: Add all blocks to IPFS
        for (cid, block) in blocks {
            self.ipfs.put_block(ipfs::Block::new(block.into_boxed_slice(), cid.clone())).await?;
            println!("Stored block with CID: {}", cid);
        }
    
        // Add root block
        self.ipfs.put_block(ipfs::Block::new(root_block.into_boxed_slice(), root_cid.clone())).await?;
        println!("✅ File added successfully with CIDv1: {}", root_cid);
    
        // Step 7: Return CIDv1 or CIDv0 if possible
        let multihash = root_cid.hash().to_owned();
        let cid_v0 = Cid::new_v0(multihash.clone()).ok();
        let final_cid = if let Some(cid) = cid_v0 {
            println!("Generated CIDv0: {}", cid);
            cid.to_string()
        } else {
            root_cid.to_string()
        };
    
        Ok(final_cid)
    }


    pub async fn combine_folder_contents(
        &self,
        folder_path: &str,
        folder_cid_map: &[(String, String)],
    ) -> Result<String> {
        // For simplicity, use a JSON representation to map file paths to CIDs
        let folder_manifest: serde_json::Value = serde_json::json!({
            "path": folder_path,
            "contents": folder_cid_map
                .iter()
                .map(|(file_path, cid)| serde_json::json!({ "path": file_path, "cid": cid }))
                .collect::<Vec<_>>()
        });

        // Serialize the folder manifest to bytes
        let manifest_bytes = serde_json::to_vec(&folder_manifest)?;

        // Compute CID for the folder manifest
        let mut hasher = Sha256::new();
        hasher.update(&manifest_bytes);
        let hash_result = hasher.finalize();

        let multihash = MultihashCode::Sha2_256.digest(&hash_result);
        let cid = Cid::new_v1(CidCodec::Raw, multihash);

        // Box the content and add it as a block to IPFS
        let block = Block::new(manifest_bytes.into_boxed_slice(), cid.clone());
        self.ipfs.put_block(block).await?;

        println!("Folder manifest added to IPFS with CID: {}", cid);

        Ok(cid.to_string())
    }

     /// Initializes the IPFS node.
    ///
    /// # Errors
    ///
    /// Returns an error if the IPFS node fails to start.
   


    pub async fn add_folder(&self, folder_path: &str) -> Result<String> {
        // Verify the provided path is a directory
        let metadata = fs::metadata(folder_path).await?;
        if !metadata.is_dir() {
            return Err(anyhow::anyhow!("The provided path is not a directory"));
        }
    
        let mut folder_cid_map: Vec<(String, String)> = Vec::new(); // Store CIDs for each file
        let mut dir_entries = read_dir(folder_path).await?;
    
        // Iterate through directory entries
        while let Some(entry) = dir_entries.next_entry().await? {
            let path = entry.path();
            let file_type = entry.file_type().await?;
    
            if file_type.is_file() {
                // Add the file and store its CID
                if let Some(file_path) = path.to_str() {
                    println!("Adding file: {}", file_path);
                    let cid = self.add_file(file_path, None).await?; // Single CID
                    folder_cid_map.push((file_path.to_string(), cid)); // Use the returned CID
                }
            } else if file_type.is_dir() {
                // Box the recursive call for subdirectories
                if let Some(dir_path) = path.to_str() {
                    println!("Recursively adding folder: {}", dir_path);
                    let folder_cid = Pin::from(Box::new(self.add_folder(dir_path))).await?;
                    folder_cid_map.push((dir_path.to_string(), folder_cid));
                }
            }
        }
    
        // Combine the folder contents into a single representation
        let folder_cid = self.combine_folder_contents(folder_path, &folder_cid_map).await?;
        Ok(folder_cid)
    }
    


    pub async fn add_zip(&self, zip_path: &str) -> Result<String> {
        use tokio::fs::{create_dir_all, remove_dir_all, File};
        use tokio::io::AsyncWriteExt;
        use zip::ZipArchive;
    
        // Create a temporary directory for extraction
        let temp_dir = std::env::temp_dir().join("ipfs_zip_upload");
        create_dir_all(&temp_dir).await.map_err(|e| {
            anyhow::anyhow!(
                "Failed to create temporary directory for zip extraction: {}",
                e
            )
        })?;
    
        // Open the ZIP file and convert it to a synchronous `std::fs::File`
        let zip_file = File::open(zip_path)
            .await
            .map_err(|e| anyhow::anyhow!("Failed to open zip file: {}. Error: {}", zip_path, e))?;
        let zip_file = zip_file.into_std().await;
    
        // Spawn a blocking task to handle the ZIP extraction
        let temp_dir_clone = temp_dir.clone();
        tokio::task::spawn_blocking(move || {
            let mut archive = ZipArchive::new(zip_file).map_err(|e| {
                anyhow::anyhow!("Failed to read zip archive: {}", e)
            })?;
    
            for i in 0..archive.len() {
                let mut file = archive.by_index(i).map_err(|e| {
                    anyhow::anyhow!("Failed to access file in zip archive: {}", e)
                })?;
                let out_path = temp_dir_clone.join(file.name());
    
                // Ensure the output directory exists
                if let Some(parent) = out_path.parent() {
                    std::fs::create_dir_all(parent).map_err(|e| {
                        anyhow::anyhow!(
                            "Failed to create parent directory for extracted file: {}",
                            e
                        )
                    })?;
                }
    
                // Write the file content
                let mut out_file = std::fs::File::create(&out_path).map_err(|e| {
                    anyhow::anyhow!("Failed to create extracted file: {}", e)
                })?;
                std::io::copy(&mut file, &mut out_file).map_err(|e| {
                    anyhow::anyhow!("Failed to write extracted file content: {}", e)
                })?;
            }
            Ok::<_, anyhow::Error>(())
        })
        .await
        .map_err(|e| anyhow::anyhow!("Blocking task failed: {}", e))??;
    
        // Add the extracted folder to IPFS
        let folder_cid = self.add_folder(temp_dir.to_str().unwrap()).await?;
    
        // Clean up the temporary directory
        remove_dir_all(&temp_dir).await.map_err(|e| {
            anyhow::anyhow!("Failed to clean up temporary directory: {}", e)
        })?;
    
        Ok(folder_cid)
    }
    
    
    


    

    /// Retrieves a file from IPFS using its CID.
    ///
    /// # Arguments
    ///
    /// * `cid_str` - The CID of the file to retrieve.
    ///
    /// # Errors
    ///
    /// Returns an error if the CID is invalid or the file cannot be retrieved.
    pub async fn get_file(&self, cid_str: &str) -> Result<Vec<u8>> {
        // Log the received CID
        println!("Attempting to fetch file with CID: {}", cid_str);
    
        // Parse the CID string into a Cid object
        let cid: Cid = match cid_str.parse() {
            Ok(c) => {
                println!("Successfully parsed CID: {:?}", c);
                c
            }
            Err(e) => {
                eprintln!("Failed to parse CID: {} - Error: {:?}", cid_str, e);
                return Err(anyhow::anyhow!("Invalid CID format: {:?}", e));
            }
        };
    
        // Retrieve the block from IPFS using the CID
        match self.ipfs.get_block(&cid).await {
            Ok(block) => {
                println!("Successfully retrieved block for CID: {}", cid_str);
                Ok(block.data().to_vec())
            }
            Err(e) => {
                eprintln!("Failed to retrieve block for CID: {} - Error: {:?}", cid_str, e);
                Err(anyhow::anyhow!("Block retrieval failed: {:?}", e))
            }
        }
    }
    
    pub async fn pin_file(&self, cid_str: &str) -> Result<()> {
        // Parse the CID string
        let cid: Cid = cid_str.parse()?;

        // Check if the block exists in the blockstore
        match self.ipfs.get_block(&cid).await {
            Ok(_) => {
                println!("CID {} is already in the blockstore, ensuring persistence...", cid_str);
                // For `rust-ipfs`, simply ensuring the block exists in the blockstore is sufficient for pinning.
                // Additional logic can be added here to persist the block if needed.
                Ok(())
            }
            Err(e) => Err(anyhow::anyhow!("Failed to pin CID {}: {}", cid_str, e)),
        }
    }

    pub async fn unpin_file(&self, cid_str: &str) -> Result<()> {
        // Parse the CID string
        let cid: Cid = cid_str.parse()?;
    
        // Check if the block exists in the blockstore before attempting to unpin
        match self.ipfs.get_block(&cid).await {
            Ok(_) => {
                println!("CID {} found in the blockstore, proceeding with unpinning...", cid_str);
                // Remove the block from the blockstore to allow garbage collection
                self.ipfs.remove_block(cid).await?;
                println!("CID {} successfully unpinned.", cid_str);
                Ok(())
            }
            Err(e) => Err(anyhow::anyhow!("Failed to unpin CID {}: {}", cid_str, e)),
        }
    }


    /// Retrieves the list of multiaddresses associated with the IPFS node.
    ///
    /// # Errors
    ///
    /// Returns an error if the identity information cannot be retrieved.
    pub async fn get_addresses(&self) -> Result<Vec<String>> {
        // Get the identity information of the IPFS node
        let (_, addresses) = self.ipfs.identity().await?;

        // Convert multiaddresses to strings
        Ok(addresses.into_iter().map(|addr| addr.to_string()).collect())
    }

    /// Discovers and retrieves content from IPFS using its CID.
    ///
    /// # Arguments
    ///
    /// * `cid` - The CID of the content to discover.
    ///
    /// # Errors
    ///
    /// Returns an error if the content cannot be fetched.
    pub async fn discover_content(&self, cid: &str) -> Result<()> {
        // Parse the CID string into an IpfsPath
        let parsed_cid: IpfsPath = cid.parse()?;

        // Fetch the UnixFS stream from IPFS
        let stream = self.ipfs.cat_unixfs(parsed_cid, None).await?;
        tokio::pin!(stream); // Pin the stream for asynchronous iteration

        // Iterate over the stream to retrieve chunks of data
        while let Some(chunk) = stream.next().await {
            match chunk {
                Ok(data) => println!("Discovered chunk: {:?}", data),
                Err(e) => eprintln!("Failed to fetch content: {}", e),
            }
        }

        Ok(())
    }
}