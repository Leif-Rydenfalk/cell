use anyhow::Result;
use cell_sdk::*;
use rand::Rng;

// This generates WorkLoad and WorkResult structs with:
// #[derive(Serialize, Deserialize, Archive, rkyv::Serialize, rkyv::Deserialize, ...)]
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
        // 1. Zero-Copy Access
        // We check the bytes to ensure safety before access.
        let job = cell_sdk::rkyv::check_archived_root::<WorkLoad>(vesicle.as_slice())
            .map_err(|e| anyhow::anyhow!("Corrupt DNA: {}", e))?;

        // --- SIMULATE HEAVY WORK ---
        let mut rng = rand::thread_rng();
        let mut sum = 0u64;

        // Note: rkyv accessors (job.iterations) are usually zero-cost dereferences.
        // We use native endianness, so it compiles to a direct memory load.
        let iters = job.iterations;

        // Uncomment to simulate real CPU load:
        /*
        for _ in 0..iters {
             sum = sum.wrapping_add(rng.gen::<u64>());
        }
        */
        // For pure bandwidth benchmarking, we just pretend we calculated something.
        sum = 42;

        // Log occasionally (logging IO is slow, don't do it every request in a benchmark)
        if job.id % 1000 == 0 {
            println!("Processed Job #{}", job.id);
        }

        // 2. Serialize Response
        let res = WorkResult {
            processed: iters,
            checksum: sum,
        };

        let bytes = cell_sdk::rkyv::to_bytes::<_, 256>(&res)?.into_vec();
        Ok(vesicle::Vesicle::wrap(bytes))
    })
}
