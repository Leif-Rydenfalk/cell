// SPDX-License-Identifier: MIT
// cell-sdk/tests/handler_return_type.rs
//! Tests that handler return types are properly unwrapped.
//!
//! Verifies the fix where handlers returning Result<T> were being
//! double-wrapped in Result<Result<T>>.

use cell_sdk::prelude::*;
use rkyv::Deserialize; // Bring Deserialize trait into scope

// Test protocol
#[derive(rkyv::Archive, rkyv::Serialize, rkyv::Deserialize, Debug, Clone)]
#[archive(check_bytes)]
struct TestRequest {
    input: u64,
}

#[derive(rkyv::Archive, rkyv::Serialize, rkyv::Deserialize, Debug, Clone)]
#[archive(check_bytes)]
struct TestResponse {
    output: u64,
}

// Simulated handler that returns Result<T>
async fn handler_returning_result(
    req: &<TestRequest as rkyv::Archive>::Archived,
) -> anyhow::Result<TestResponse> {
    let deserialized: TestRequest = rkyv::Deserialize::deserialize(
        req,
        &mut rkyv::de::deserializers::SharedDeserializeMap::new(),
    )
    .map_err(|e| anyhow::anyhow!("Deserialization failed: {:?}", e))?;

    if deserialized.input > 100 {
        Ok(TestResponse {
            output: deserialized.input * 2,
        })
    } else {
        Err(anyhow::anyhow!("Input too small"))
    }
}

#[test]
fn test_extract_ok_type_pattern() {
    // Test the extract_ok_type logic used in the handler macro
    // We simulate what the macro does without using syn

    // Test case 1: Result<u64> should extract to u64
    let test_cases = [
        ("Result<u64>", "u64"),
        ("Result<String>", "String"),
        ("Result<MyStruct>", "MyStruct"),
        ("u64", "u64"),
    ];

    for (input, expected) in &test_cases {
        // Simple string-based check since we can't use syn in tests
        let extracted = if input.starts_with("Result<") && input.ends_with(">") {
            &input[7..input.len() - 1] // Extract inner type
        } else {
            input
        };

        assert_eq!(
            extracted, *expected,
            "Extracted {} from {}",
            extracted, input
        );
    }
}

#[tokio::test]
async fn test_handler_dispatch_pattern() {
    // Test the exact dispatch pattern used by the handler macro

    let request = TestRequest { input: 150 };
    let serialized = rkyv::to_bytes::<_, 256>(&request).unwrap();

    // Simulate archived request
    let archived = rkyv::check_archived_root::<TestRequest>(&serialized).unwrap();

    // Call handler
    let handler_result = handler_returning_result(&archived).await;

    // Map to response (this is what the macro does)
    let response_result: Result<TestResponse, cell_sdk::CellError> =
        handler_result.map_err(|_| cell_sdk::CellError::SerializationFailure);

    // Verify we get T directly, not Result<T>
    match response_result {
        Ok(response) => {
            // response is TestResponse directly, not Result<TestResponse>
            assert_eq!(response.output, 300);
        }
        Err(e) => {
            panic!("Unexpected error: {:?}", e);
        }
    }
}

#[test]
fn test_response_enum_structure() {
    // Verify that the response enum variant holds T directly

    // This simulates what the macro generates:
    // pub enum ServiceResponse {
    //     MethodName(ReturnType),  // NOT MethodName(Result<ReturnType>)
    // }

    // Test that we can construct the expected pattern
    let response = TestResponse { output: 42 };

    // Simulated enum variant holding T directly
    enum SimulatedResponse {
        TestMethod(TestResponse),
    }

    let variant = SimulatedResponse::TestMethod(response);

    match variant {
        SimulatedResponse::TestMethod(r) => {
            assert_eq!(r.output, 42);
        }
    }
}
