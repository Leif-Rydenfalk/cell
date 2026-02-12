use crate::protocol::{RenderCommand, InputState};
use cell_sdk::{Membrane, vesicle::Vesicle};
use std::sync::{Arc, Mutex};
use lazy_static::lazy_static;

lazy_static! {
    pub static ref INBOX: Arc<Mutex<Vec<RenderCommand>>> = Arc::new(Mutex::new(Vec::new()));
    pub static ref LATEST_INPUT: Arc<Mutex<InputState>> = Arc::new(Mutex::new(InputState {
        keys_down: vec![],
        mouse_delta: [0.0, 0.0],
    }));
}

pub fn start_listener() {
    std::thread::spawn(|| {
        let rt = tokio::runtime::Runtime::new().unwrap();
        rt.block_on(async {
            println!("[Retina] Membrane Active on 'renderer'.");
            
            // Listen for commands
            // FIX: Use 'bind' instead of 'bind_background', pass None for genome
            let _ = Membrane::bind("renderer", |vesicle| async move {
                if let Ok(cmd) = cell_sdk::rkyv::from_bytes::<RenderCommand>(vesicle.as_slice()) {
                    match cmd {
                        RenderCommand::GetInputState => {
                            // Immediate response from shared state
                            if let Ok(mut lock) = (*LATEST_INPUT).lock() {
                                // We clone because 'to_bytes' needs a reference, avoiding deadlock if we held lock too long
                                // But here simple access is fine.
                                let state = InputState {
                                    keys_down: lock.keys_down.clone(),
                                    mouse_delta: lock.mouse_delta,
                                };
                                
                                // Reset delta after reading?
                                // Usually input polling resets deltas. 
                                // If Brain polls faster than Frame, we might get partial deltas.
                                // If Brain polls slower, we accumulate.
                                // Let's reset here to ensure "consumption".
                                lock.mouse_delta = [0.0, 0.0];
                                
                                let bytes = cell_sdk::rkyv::to_bytes::<_, 256>(&state)?.into_vec();
                                return Ok(Vesicle::wrap(bytes));
                            }
                        }
                        _ => {
                            // Enqueue for main thread
                            if let Ok(mut lock) = (*INBOX).lock() {
                                lock.push(cmd);
                            }
                        }
                    }
                }
                Ok(Vesicle::new())
            }, None).await; // Added None for genome_json
            
            loop { tokio::time::sleep(std::time::Duration::from_secs(3600)).await; }
        });
    });
}