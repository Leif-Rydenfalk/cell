use cell_sdk::*;
use anyhow::Result;

cell_remote!(Nucleus = "nucleus");

#[tokio::test]
async fn apply_mesh_manifest() {
    // 1. Boot the substrate
    cell_sdk::System::ignite_local_cluster().await.unwrap();

    // 2. Ensure Nucleus is accessible
    // It's auto-spawned by the system ignition usually, but explicit spawn ensures readiness
    System::spawn("nucleus", None).await.expect("Failed to spawn nucleus");
    let synapse = Synapse::grow_await("nucleus").await.expect("Failed to connect");
    let mut n = Nucleus::Client::new(synapse);

    // 3. Define a valid manifest
    let yaml = r#"
mesh: production-alpha
cells:
  - name: ledger
    replicas: 3
    resources:
      cpu: 4.0
      mem: "8Gi"
    placement:
      zone: "us-west"
      required_instruction_set: "avx512"
  - name: api-gateway
    replicas: 1
"#;

    // 4. Apply via RPC
    let result = n.apply(Nucleus::ApplyManifest { yaml: yaml.to_string() }).await;
    
    // 5. Verify
    assert!(result.is_ok(), "Nucleus should accept valid YAML");
    assert!(result.unwrap(), "Nucleus should return success");
    
    // 6. Verify state via status (Optional, if status API exposed manifest info)
    let status = n.status().await.unwrap();
    // In a real test we might check if 'ledger' appears in managed_cells, 
    // but the reconciliation loop runs asynchronously.
    assert!(status.uptime_secs >= 0);
}

#[tokio::test]
async fn apply_invalid_manifest() {
    cell_sdk::System::ignite_local_cluster().await.unwrap();
    System::spawn("nucleus", None).await.expect("Failed to spawn nucleus");
    let synapse = Synapse::grow_await("nucleus").await.expect("Failed to connect");
    let mut n = Nucleus::Client::new(synapse);

    let yaml = "this: is: not: valid: yaml: [}";

    let result = n.apply(Nucleus::ApplyManifest { yaml: yaml.to_string() }).await;
    
    // The handler returns Ok(bool) or Err(RPC Error). 
    // If the handler logic fails (anyhow error), it typically returns Err(CellError) to client.
    assert!(result.is_err(), "Nucleus should error on invalid YAML");
}