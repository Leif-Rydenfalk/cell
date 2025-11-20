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

        // Simulating CPU work
        let mut rng = rand::thread_rng();
        let mut sum = 0u64;
        // Reduce this to 100 to measure Network overhead, not RNG speed
        for _ in 0..100 {
            sum = sum.wrapping_add(rng.gen::<u64>());
        }

        // Only log every 1000 requests to save I/O
        if job.id % 1000 == 0 {
            println!("Processed batch up to Job #{}", job.id);
        }

        let res = WorkResult {
            processed: job.iterations,
            checksum: sum,
        };

        let bytes = cell_sdk::rkyv::to_bytes::<_, 256>(&res)?.into_vec();
        Ok(vesicle::Vesicle::wrap(bytes))
    })
}
