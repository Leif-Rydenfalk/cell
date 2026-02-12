use anyhow::Result;
use cell_cli::nucleus;
use std::os::unix::fs::PermissionsExt;
use std::time::Duration; // Required for chmod

#[tokio::test]
async fn test_nucleus_billing_clean_exit() -> Result<()> {
    let temp = tempfile::tempdir()?;
    let run_dir = temp.path();
    let cell_sock = run_dir.join("cell.sock");
    let golgi_sock = run_dir.join("golgi.sock");

    // Use a script that sleeps for 100ms so we can measure > 0ms wall time.
    // /bin/echo is too fast and often results in 0ms.
    let script_path = run_dir.join("clean_job.sh");
    std::fs::write(&script_path, "#!/bin/sh\n/bin/sleep 0.1")?;
    std::fs::set_permissions(&script_path, std::fs::Permissions::from_mode(0o755))?;

    // 1. Activate Nucleus
    let mut guard = nucleus::activate(
        &cell_sock,
        nucleus::LogStrategy::Piped,
        &script_path,
        &golgi_sock,
    )?;

    // 2. Wait for it to finish naturally
    let metabolism = guard.wait().await?;

    // 3. Verify Billing Data
    println!("Metabolism Report: {:?}", metabolism);

    assert_eq!(metabolism.exit_code, Some(0), "Process should exit cleanly");
    assert!(
        metabolism.wall_time_ms > 0,
        "Wall time should be recorded (> 0ms)"
    );

    // Check consistency
    assert!(
        metabolism.cpu_time_ms <= metabolism.wall_time_ms,
        "CPU time cannot exceed Wall time"
    );

    Ok(())
}

#[tokio::test]
async fn test_nucleus_billing_on_kill() -> Result<()> {
    let temp = tempfile::tempdir()?;
    let run_dir = temp.path();
    let cell_sock = run_dir.join("cell.sock");
    let golgi_sock = run_dir.join("golgi.sock");

    // Create a long-running script to kill
    let script_path = run_dir.join("sleeper.sh");
    std::fs::write(&script_path, "#!/bin/sh\n/bin/sleep 10")?;
    std::fs::set_permissions(&script_path, std::fs::Permissions::from_mode(0o755))?;

    let mut guard = nucleus::activate(
        &cell_sock,
        nucleus::LogStrategy::Piped,
        &script_path,
        &golgi_sock,
    )?;

    // Let it run for 100ms
    tokio::time::sleep(Duration::from_millis(100)).await;

    // Kill it
    guard.kill().await?;

    // Wait for the bill
    let metabolism = guard.wait().await?;

    println!("Metabolism Report (Killed): {:?}", metabolism);

    // Verify it didn't finish cleanly (exit code should be None or signal-based non-zero)
    // Note: status.code() returns None if killed by signal on Unix
    let killed_correctly = metabolism.exit_code.is_none() || metabolism.exit_code != Some(0);
    assert!(
        killed_correctly,
        "Process should have been killed (non-zero exit)"
    );

    // Verify we still tracked time
    assert!(
        metabolism.wall_time_ms >= 100,
        "Should have tracked at least 100ms wall time"
    );

    Ok(())
}
