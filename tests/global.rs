use cell_test_support::*;
use std::path::PathBuf;

#[ctor::ctor]
fn start_substrate() {
    // Initialize logging for test visibility
    let _ = tracing_subscriber::fmt()
        .with_env_filter("info")
        .with_test_writer()
        .try_init();
    
    // Setup isolated test environment
    let mut target_dir = std::env::current_dir().unwrap();
    target_dir.push("target");
    target_dir.push("test-sockets");
    
    // Clean previous run
    if target_dir.exists() {
        std::fs::remove_dir_all(&target_dir).unwrap();
    }
    std::fs::create_dir_all(&target_dir).unwrap();

    // Set environment variables for the Runtime and Discovery
    std::env::set_var("CELL_SOCKET_DIR", target_dir.to_str().unwrap());
    std::env::set_var("CELL_DISABLE_SHM", "0");
    std::env::set_var("CELL_NODE_ID", "100");
    
    // Set a fake home for cells that rely on ~/.cell
    let fake_home = target_dir.join("home");
    std::fs::create_dir_all(&fake_home).unwrap();
    std::env::set_var("HOME", fake_home.to_str().unwrap());

    // Boot the substrate
    tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .unwrap()
        .block_on(async {
            // Start Root (Daemon)
            let _ = root().await;
            
            // Start Nucleus (System Manager)
            let _ = spawn("nucleus").await;
            
            // Start Axon (Network/Discovery) - Required for LanDiscovery globals
            let _ = spawn("axon").await;

            // Wait a moment for discovery to settle
            tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        });
}

#[dtor::dtor]
fn stop_substrate() {
    if let Ok(dir) = std::env::var("CELL_SOCKET_DIR") {
        let _ = std::fs::remove_dir_all(dir);
    }
}