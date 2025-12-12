use cell_sdk::process::MyceliumRoot;
use cell_sdk::System;
use tokio::sync::OnceCell;
use std::sync::Arc;

static ROOT: OnceCell<Arc<MyceliumRoot>> = OnceCell::const_new();

#[ctor::ctor]
fn start_substrate() {
    // 1. Configure isolated test environment
    let mut target_dir = std::env::current_dir().unwrap();
    target_dir.push("target");
    target_dir.push("test-sockets");
    
    if target_dir.exists() {
        let _ = std::fs::remove_dir_all(&target_dir);
    }
    std::fs::create_dir_all(&target_dir).unwrap();

    // Standard env vars used by the Daemon
    std::env::set_var("CELL_SOCKET_DIR", target_dir.to_str().unwrap());
    std::env::set_var("CELL_DISABLE_SHM", "0");
    std::env::set_var("CELL_NODE_ID", "100");
    
    let fake_home = target_dir.join("home");
    std::fs::create_dir_all(&fake_home).unwrap();
    std::env::set_var("HOME", fake_home.to_str().unwrap());

    // 2. Initialize Logging
    let _ = tracing_subscriber::fmt()
        .with_env_filter("info")
        .with_test_writer()
        .try_init();

    // 3. Boot the Daemon
    tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .unwrap()
        .block_on(async {
            // Start Root (Daemon) using the SDK's process module
            let root = MyceliumRoot::ignite().await.expect("Failed to start Mycelium Root");
            ROOT.set(Arc::new(root)).ok();
            
            // Spawn Core Services via standard System API
            // This tests the actual spawn path used by real cells
            
            // Nucleus (System Manager)
            for _ in 0..50 {
                if System::spawn("nucleus", None).await.is_ok() {
                    break;
                }
                tokio::time::sleep(std::time::Duration::from_millis(50)).await;
            }
            
            // Axon (Network)
            System::spawn("axon", None).await.expect("Failed to spawn axon");

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