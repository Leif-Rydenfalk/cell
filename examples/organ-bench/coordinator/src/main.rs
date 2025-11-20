use anyhow::Result;
use cytosol::*;
use ribosome::call_as;
use std::time::Instant;

fn main() -> Result<()> {
    // 1. Define the request data (Must match Worker's input struct)
    // Note: In a real app, we might share a crate for types,
    // but here we define the data structure we want to send.
    #[derive(
        serde::Serialize, serde::Deserialize, cytosol::rkyv::Archive, cytosol::rkyv::Serialize,
    )]
    struct WorkLoad {
        id: u32,
        iterations: u32,
        payload: Vec<u8>,
    }

    // We need the response type in scope for the macro to deserialize into
    #[derive(
        serde::Serialize,
        serde::Deserialize,
        cytosol::rkyv::Archive,
        cytosol::rkyv::Deserialize,
        Debug,
    )]
    #[archive(check_bytes)]
    struct WorkResult {
        processed: u32,
        checksum: u64,
    }

    println!("🧠 Coordinator starting benchmark...");
    let start = Instant::now();

    // 2. Send Signals
    for i in 0..10 {
        let job = WorkLoad {
            id: i,
            iterations: 1_000_000,
            payload: vec![0u8; 1024], // 1KB payload
        };

        // 3. The Macro Magic
        // - Connects to 'worker' (via Golgi)
        // - Serializes 'job'
        // - Fires Vesicle
        // - Awaits Response
        // - Deserializes to 'WorkResult'
        let result: Result<WorkResult> = call_as!(worker, job);

        match result {
            Ok(r) => println!("✅ Job {} complete. Checksum: {:x}", i, r.checksum),
            Err(e) => println!("❌ Job {} failed: {}", i, e),
        }
    }

    println!("🏁 Benchmark complete in {:?}", start.elapsed());
    Ok(())
}
