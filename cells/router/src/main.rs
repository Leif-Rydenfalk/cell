// cells/router/src/main.rs
use anyhow::Result;
use cell_core::RouterDescriptor;
use tokio::fs;
use tokio::io::AsyncReadExt;

#[tokio::main]
async fn main() -> Result<()> {
    let socket_dir = cell_sdk::resolve_socket_dir();
    let routers_dir = socket_dir.join("routers");
    let pipes_dir = socket_dir.join("pipes");
    
    fs::create_dir_all(&routers_dir).await?;
    fs::create_dir_all(&pipes_dir).await?;

    // 1. Create our Input Pipe
    let pipe_name = "router_tcp_01";
    let pipe_path = pipes_dir.join(pipe_name);
    // Use libc::mkfifo in real impl
    fs::File::create(&pipe_path).await?; 

    // 2. Establish Connection (Simulated)
    // Assume we connected to "ledger" via TCP.
    // "ledger" ID = 0x1234...
    let ledger_name = "ledger";
    let h = blake3::hash(ledger_name.as_bytes());
    let ledger_id = u64::from_le_bytes(h.as_bytes()[..8].try_into().unwrap());

    // 3. Drop the Router File
    // This tells the SDK: "To reach 'ledger', write to 'router_tcp_01'"
    let mut name_bytes = [0u8; 32];
    name_bytes[..pipe_name.len()].copy_from_slice(pipe_name.as_bytes());

    let desc = RouterDescriptor {
        pipe_name: name_bytes,
        transport_type: 3, // TCP
        _pad: [0; 31],
    };

    let router_file = routers_dir.join(format!("{:016x}.router", ledger_id));
    
    // Unsafe cast to bytes for write
    let desc_bytes: [u8; 64] = unsafe { std::mem::transmute(desc) };
    fs::write(&router_file, &desc_bytes).await?;
    
    println!("Router Active. Bridging 'ledger' -> '{}'", pipe_name);

    // 4. Pump Data
    let mut rx = fs::OpenOptions::new().read(true).open(&pipe_path).await?;
    loop {
        // Read from SDK, Write to TCP...
        let mut buf = [0u8; 1024];
        let n = rx.read(&mut buf).await?;
        if n == 0 { break; }
        println!("Router forwarded {} bytes", n);
    }

    Ok(())
}