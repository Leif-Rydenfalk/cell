#[cfg(test)]
mod tests {
    use cell_discovery::hardware::HardwareCaps;

    #[test]
    fn test_hardware_scan() {
        let caps = HardwareCaps::scan();
        
        // Basic sanity checks for the host running the test
        assert!(caps.cpu_cores > 0, "Must detect at least 1 core");
        assert!(caps.total_memory_mb > 0, "Must detect memory");
        
        println!("Detected Hardware: {:?}", caps);
        
        // Since we are likely running in a standard env, AVX might be true or false,
        // but we verify the struct fields are populated.
    }

    #[test]
    fn test_caps_serialization() {
        use cell_model::rkyv;
        
        let original = HardwareCaps {
            cpu_cores: 64,
            total_memory_mb: 512000,
            has_avx512: true,
            has_gpu: true,
            is_tee: true,
            load_avg: 0.5,
            thermal_zone_temp: Some(60.0),
        };

        let bytes = rkyv::to_bytes::<_, 256>(&original).expect("Serialize failed");
        let archived = rkyv::check_archived_root::<HardwareCaps>(&bytes).expect("Verify failed");
        let deserialized: HardwareCaps = archived.deserialize(&mut rkyv::Infallible).unwrap();

        assert_eq!(original.cpu_cores, deserialized.cpu_cores);
        assert_eq!(original.has_avx512, deserialized.has_avx512);
        assert_eq!(original.is_tee, deserialized.is_tee);
    }
}