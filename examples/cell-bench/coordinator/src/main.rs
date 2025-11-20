use anyhow::Result;
use cell_sdk::*;
use std::time::Instant;

fn main() -> Result<()> {
    #[derive(
        serde::Serialize, serde::Deserialize, cell_sdk::rkyv::Archive, cell_sdk::rkyv::Serialize,
    )]
    // Tell rkyv to use the re-exported crate path
    #[archive(crate = "cell_sdk::rkyv")]
    #[archive(check_bytes)]
    struct WorkLoad {
        id: u32,
        iterations: u32,
        payload: Vec<u8>,
    }

    #[derive(
        serde::Serialize,
        serde::Deserialize,
        cell_sdk::rkyv::Archive,
        cell_sdk::rkyv::Deserialize,
        Debug,
    )]
    #[archive(crate = "cell_sdk::rkyv")]
    #[archive(check_bytes)]
    #[archive_attr(derive(Debug))]
    struct WorkResult {
        processed: u32,
        checksum: u64,
    }

    println!("🧠 Coordinator starting benchmark...");
    let start = Instant::now();

    for i in 0..10 {
        let job = WorkLoad {
            id: i,
            iterations: 1_000_000,
            payload: vec![0u8; 1024],
        };

        let result: Result<WorkResult> = call_as!(worker, job);

        match result {
            Ok(r) => println!("✅ Job {} complete. Checksum: {:x}", i, r.checksum),
            Err(e) => println!("❌ Job {} failed: {}", i, e),
        }
    }

    println!("🏁 Benchmark complete in {:?}", start.elapsed());
    Ok(())
}
