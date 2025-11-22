use anyhow::Result;
use cell_sdk::*;
use rand::Rng;

signal_receptor! {
    name: worker,
    input: WorkLoad {
        id: u32,
        iterations: u32,
        payload: Vec<u8>,
    },
    output: WorkResult {
        processed: u32,
        checksum: u64,
    }
}

fn main() -> Result<()> {
    println!("Worker started and ready for heavy load.");

    Membrane::bind(__GENOME__, |vesicle| {
        let job = cell_sdk::rkyv::check_archived_root::<WorkLoad>(vesicle.as_slice())
            .map_err(|e| anyhow::anyhow!("Corrupt DNA: {}", e))?;

        // --- SIMULATE HEAVY WORK ---
        let mut rng = rand::thread_rng();
        let mut sum = 0u64;

        // Uncomment to simulate real CPU load
        // If this is too low, the bottleneck is the Golgi Router, not the workers.
        // for _ in 0..500_000 {
        //     sum = sum.wrapping_add(rng.gen::<u64>());
        // }
        // ---------------------------

        // Log to prove which worker is handling it (Vacuole will tag this)
        // Only log occasionally to avoid disk spam affecting benchmarks
        if job.id % 500 == 0 {
            println!("Processed Job #{}", job.id);
        }

        let res = WorkResult {
            processed: job.iterations,
            checksum: sum,
        };

        let bytes = cell_sdk::rkyv::to_bytes::<_, 256>(&res)?.into_vec();
        Ok(vesicle::Vesicle::wrap(bytes))
    })
}
