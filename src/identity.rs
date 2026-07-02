//! Persistent node identity: an Ed25519 keypair stored in the data dir so the
//! PeerId (and therefore the wallet address) survives restarts. The legacy
//! node generated a fresh key on every start.

use anyhow::{Context, Result};
use libp2p::identity::Keypair;
use std::path::Path;

pub fn load_or_generate(dir: &Path) -> Result<Keypair> {
    let path = dir.join("identity.key");
    if path.exists() {
        let bytes = std::fs::read(&path).with_context(|| format!("reading {}", path.display()))?;
        Keypair::from_protobuf_encoding(&bytes)
            .with_context(|| format!("decoding keypair from {}", path.display()))
    } else {
        std::fs::create_dir_all(dir)?;
        let keypair = Keypair::generate_ed25519();
        let bytes = keypair.to_protobuf_encoding().context("encoding keypair")?;
        std::fs::write(&path, &bytes).with_context(|| format!("writing {}", path.display()))?;
        // Best-effort tighten permissions on unix.
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600));
        }
        Ok(keypair)
    }
}
