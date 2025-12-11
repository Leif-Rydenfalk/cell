use cell_test_support::*;

#[ctor::ctor]
fn start_substrate() {
    // Setup logging
    std::env::set_var("RUST_LOG", "cell=debug");
    let _ = tracing_subscriber::fmt::try_init();

    // Setup Test Environment Variables
    let mut target_dir = std::env::current_dir().unwrap();
    target_dir.push("target");
    target_dir.push("test-sockets");
    std::fs::create_dir_all(&target_dir).unwrap();
    std::env::set_var("CELL_SOCKET_DIR", target_dir.to_str().unwrap());
    std::env::set_var("CELL_DISABLE_SHM", "0"); // Ensure SHM is active

    // Ignite System in Background
    tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .unwrap()
        .block_on(async {
            // Start Root
            let _ = root().await;
            
            // Start Nucleus (System Manager)
            // Note: In real life, Root starts Nucleus. Here we force spawn it.
            // We assume "nucleus" DNA is available (compiled).
            // If running `cargo test`, binaries might not be in the expected DNA location.
            // A robust test setup would symlink target/debug/nucleus to ~/.cell/dna/nucleus.
            
            // For this test harness to work 100%, we assume the user has run `cargo build --bins`.
        });
}

#[dtor::dtor]
fn stop_substrate() {
    // Cleanup sockets
    let target_dir = std::env::var("CELL_SOCKET_DIR").unwrap();
    std::fs::remove_dir_all(target_dir).ok();
}