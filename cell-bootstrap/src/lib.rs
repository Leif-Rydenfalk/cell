// cell-bootstrap/src/lib.rs
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};
use cell_sdk::Synapse;

pub fn ensure_system() -> Result<Synapse, String> {
    let sock = cell_sdk::resolve_socket_dir().join("mitosis.sock");
    if sock.exists() {
        return Synapse::grow("hypervisor").map_err(|e| e.to_string());
    }

    eprintln!("[System] Hypervisor not found – starting…");
    let _child = Command::new("cell")
        .arg("up")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::inherit())
        .spawn()
        .map_err(|e| format!("Unable to start hypervisor: {}", e))?;

    let deadline = Instant::now() + Duration::from_secs(5);
    while Instant::now() < deadline {
        if sock.exists() {
            eprintln!("[System] Hypervisor ready ✔");
            return Synapse::grow("hypervisor").map_err(|e| e.to_string());
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    Err("Hypervisor start timed out".into())
}