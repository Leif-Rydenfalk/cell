use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;
use tokio::runtime::Runtime;

pub struct CameraFeed {
    pub current_frame: Arc<Mutex<Option<image::RgbaImage>>>,
}

impl CameraFeed {
    pub fn new(url: String) -> Self {
        let current_frame = Arc::new(Mutex::new(None));
        let thread_buffer = current_frame.clone();

        thread::spawn(move || {
            let rt = Runtime::new().unwrap();
            rt.block_on(async move {
                // Add a timeout so it doesn't hang if IP is wrong
                let client = reqwest::Client::builder()
                    .timeout(Duration::from_millis(1000))
                    .build()
                    .unwrap_or_default();

                println!("Attempting to connect to camera: {}", url);

                loop {
                    match client.get(&url).send().await {
                        Ok(resp) => {
                            if let Ok(bytes) = resp.bytes().await {
                                if let Ok(img) = image::load_from_memory(&bytes) {
                                    let mut lock = thread_buffer.lock().unwrap();
                                    *lock = Some(img.to_rgba8());
                                }
                            }
                        }
                        Err(_) => {
                            // Suppress spamming error logs, just wait
                            tokio::time::sleep(Duration::from_secs(1)).await;
                        }
                    }

                    // ~30 FPS polling
                    tokio::time::sleep(Duration::from_millis(33)).await;
                }
            });
        });

        Self { current_frame }
    }

    pub fn get_frame(&self) -> Option<image::RgbaImage> {
        let mut lock = self.current_frame.lock().unwrap();
        lock.take()
    }
}
