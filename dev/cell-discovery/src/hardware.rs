// SPDX-License-Identifier: MIT
// Hardware Fingerprinting

use serde::{Deserialize, Serialize};
use rkyv::{Archive, Serialize as RkyvSerialize, Deserialize as RkyvDeserialize};

#[derive(Debug, Clone, Serialize, Deserialize, Archive, RkyvSerialize, RkyvDeserialize, Default)]
#[archive(check_bytes)]
pub struct HardwareCaps {
    pub cpu_cores: u32,
    pub total_memory_mb: u64,
    pub has_avx512: bool,
    pub has_gpu: bool,
    pub is_tee: bool, // Trusted Execution Environment
    pub load_avg: f32,
    pub thermal_zone_temp: Option<f32>,
}

impl HardwareCaps {
    pub fn scan() -> Self {
        // In a real impl, utilize 'sysinfo' and 'raw-cpuid' crates
        // Mocking detection for this implementation to avoid huge dep trees in example code
        Self {
            cpu_cores: std::thread::available_parallelism().map(|n| n.get() as u32).unwrap_or(1),
            total_memory_mb: 16384,
            has_avx512: std::is_x86_feature_detected!("avx"), // approximating
            has_gpu: std::path::Path::new("/dev/nvidia0").exists(),
            is_tee: std::path::Path::new("/dev/sev").exists(),
            load_avg: 0.1, // Mock
            thermal_zone_temp: Some(45.0),
        }
    }
}