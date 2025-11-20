use anyhow::Result;
use cell_sdk::*; // This exports call_as and signal_receptor macros
use rand::Rng;

// Define the genetic inputs and outputs
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
    // Bind the Membrane to these genetics
    Membrane::bind(__GENOME__, |vesicle| {
        // 1. Decode (Zero-Copy check)
        let job = cell_sdk::rkyv::check_archived_root::<WorkLoad>(vesicle.as_slice())
            .map_err(|e| anyhow::anyhow!("Corrupt DNA: {}", e))?;

        // 2. Do Heavy Work
        let mut rng = rand::thread_rng();
        let mut sum = 0u64;
        for _ in 0..job.iterations {
            sum = sum.wrapping_add(rng.gen::<u64>());
        }

        if job.iterations > 1000 {
            println!(
                "💪 Worker processed Job #{} ({} ops)",
                job.id, job.iterations
            );
        }

        // 3. Return Result
        let res = WorkResult {
            processed: job.iterations,
            checksum: sum,
        };

        // 4. Pack Vesicle
        let bytes = cell_sdk::rkyv::to_bytes::<_, 256>(&res)?.into_vec();
        Ok(vesicle::Vesicle::wrap(bytes))
    })
}
