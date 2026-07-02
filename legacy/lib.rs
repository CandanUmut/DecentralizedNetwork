use anyhow::Result;

async fn new_function_doing_everything() -> Result<()> {
    // All your main.rs logic is here, for example:
    println!("Initializing Nova Network...");
    Ok(())
}

/// Public-facing function for other crates
pub async fn start_network() -> Result<()> {
    new_function_doing_everything().await
}