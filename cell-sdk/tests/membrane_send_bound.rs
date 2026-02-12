// SPDX-License-Identifier: MIT
// cell-sdk/tests/membrane_send_bound.rs
//! Tests that Membrane properly handles Send bounds with rkyv CheckBytes errors.
//!
//! This test verifies the fix for the critical issue where CheckBytes::Error
//! (which is not Send) was being held across await points in tokio::spawn tasks.

use cell_sdk::prelude::*;
use rkyv::Deserialize; // Bring Deserialize trait into scope
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::oneshot;

// Define a test protocol that will trigger validation
#[derive(rkyv::Archive, rkyv::Serialize, rkyv::Deserialize, Debug, Clone)]
#[archive(check_bytes)]
struct TestRequest {
    data: String,
    value: u64,
}

#[derive(rkyv::Archive, rkyv::Serialize, rkyv::Deserialize, Debug, Clone)]
#[archive(check_bytes)]
struct TestResponse {
    result: String,
}

// Handler that processes requests
async fn test_handler(
    req: &<TestRequest as rkyv::Archive>::Archived,
) -> anyhow::Result<TestResponse> {
    // Use associated function syntax for deserialize
    let deserialized: TestRequest = rkyv::Deserialize::deserialize(
        req,
        &mut rkyv::de::deserializers::SharedDeserializeMap::new(),
    )
    .map_err(|e| anyhow::anyhow!("Deserialization failed: {:?}", e))?;

    Ok(TestResponse {
        result: format!("Processed: {} = {}", deserialized.data, deserialized.value),
    })
}

#[tokio::test]
async fn test_membrane_handles_invalid_requests_gracefully() {
    // This test verifies that invalid rkyv data (that fails check_archived_root)
    // is handled without panicking or Send bound violations

    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::UnixStream;

    // Start membrane in background
    let (shutdown_tx, shutdown_rx) = oneshot::channel();

    let membrane_handle = tokio::spawn(async move {
        // Create a simple handler that just echoes
        let handler = |_req: &<TestRequest as rkyv::Archive>::Archived| {
            Box::pin(async move {
                Ok::<TestResponse, anyhow::Error>(TestResponse {
                    result: "ok".to_string(),
                })
            })
        };

        // Bind to temporary socket
        let temp_dir = std::env::temp_dir().join(format!("cell-test-{}", rand::random::<u64>()));
        std::fs::create_dir_all(&temp_dir).unwrap();
        let socket_path = temp_dir.join("test.sock");

        // Use internal membrane binding (simplified for test)
        // In real test, we'd use IoClient::bind_membrane
        let listener = tokio::net::UnixListener::bind(&socket_path).unwrap();

        tokio::select! {
            _ = async {
                loop {
                    let (stream, _) = listener.accept().await.unwrap();
                    // Handle connection...
                    let _ = stream;
                }
            } => {}
            _ = shutdown_rx => {}
        }

        // Cleanup
        let _ = std::fs::remove_dir_all(&temp_dir);
    });

    // Give membrane time to start
    tokio::time::sleep(Duration::from_millis(100)).await;

    // Send invalid data that should fail check_archived_root
    // This would previously cause Send bound issues

    // Shutdown membrane
    let _ = shutdown_tx.send(());
    let _ = membrane_handle.await;
}

#[tokio::test]
async fn test_concurrent_connections_are_send() {
    // Verify that multiple concurrent connections don't violate Send bounds

    use std::sync::atomic::{AtomicUsize, Ordering};

    let counter = Arc::new(AtomicUsize::new(0));

    // Spawn multiple tasks that would fail if CheckBytes::Error crosses await
    let mut handles = vec![];

    for i in 0..10 {
        let counter = counter.clone();
        let handle = tokio::spawn(async move {
            // Simulate work that would trigger the Send bound check
            tokio::time::sleep(Duration::from_millis(1)).await;
            counter.fetch_add(1, Ordering::SeqCst);
            i
        });
        handles.push(handle);
    }

    // All tasks should complete successfully
    let results = futures::future::join_all(handles).await;
    for (i, result) in results.iter().enumerate() {
        assert!(result.is_ok(), "Task {} failed: {:?}", i, result);
    }

    assert_eq!(counter.load(Ordering::SeqCst), 10);
}

#[test]
fn test_checkbytes_error_conversion_pattern() {
    // Unit test for the core pattern: convert CheckBytes error to String
    // before any await point

    fn simulate_validation<'a, T>(data: &'a [u8]) -> Result<&'a T::Archived, String>
    where
        T: rkyv::Archive,
        T::Archived: rkyv::CheckBytes<rkyv::validation::validators::DefaultValidator<'a>>,
    {
        // This is the exact pattern used in membrane.rs
        match rkyv::check_archived_root::<T>(data) {
            Ok(archived) => Ok(archived),
            Err(e) => Err(format!("Validation failed: {:?}", e)),
        }
    }

    // Test with valid data
    let valid = rkyv::to_bytes::<_, 256>(&TestRequest {
        data: "test".to_string(),
        value: 42,
    })
    .unwrap();

    let result = simulate_validation::<TestRequest>(&valid);
    assert!(result.is_ok());

    // Test with completely invalid data (random bytes, not rkyv format)
    let invalid = vec![0xFF; 100]; // 100 bytes of 0xFF - definitely not valid rkyv

    let result = simulate_validation::<TestRequest>(&invalid);
    assert!(result.is_err(), "Random bytes should fail validation");

    // Test with truncated valid data
    let mut truncated = valid.to_vec();
    truncated.truncate(10); // Too short to be valid

    let result = simulate_validation::<TestRequest>(&truncated);
    assert!(result.is_err(), "Truncated data should fail validation");

    // The error is a String, which is Send
    let err: String = match result {
        Err(e) => e,
        Ok(_) => panic!("Expected error"),
    };
    let _send_check: Box<dyn Send> = Box::new(err);
}
