use cell_sdk::System;

#[ctor::ctor]
fn start_substrate() {
    // Boot the substrate daemon using the public SDK API
    tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .unwrap()
        .block_on(async {
            System::ignite_local_cluster().await.expect("Failed to ignite local cluster");
        });
}

#[dtor::dtor]
fn stop_substrate() {
    if let Ok(dir) = std::env::var("CELL_SOCKET_DIR") {
        let _ = std::fs::remove_dir_all(dir);
    }
}