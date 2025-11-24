use anyhow::Result;
use cell_cli::nucleus;
use std::path::PathBuf;
use std::time::Duration;

#[tokio::test]
async fn test_nucleus_billing_clean_exit() -> Result<()> {
    let temp = tempfile::tempdir()?;
    let run_dir = temp.path();
    let cell_sock = run_dir.join("cell.sock");
    let golgi_sock = run_dir.join("golgi.sock");

    // We use standard "echo" as our binary to simulate a quick job
    let binary = PathBuf::from("/bin/echo");

    // 1. Activate Nucleus
    let mut guard = nucleus::activate(
        &cell_sock,
        nucleus::LogStrategy::Piped,
        &binary,
        &golgi_sock,
    )?;

    // 2. Wait for it to finish naturally
    let metabolism = guard.wait().await?;

    // 3. Verify Billing Data
    println!("Metabolism Report: {:?}", metabolism);

    assert_eq!(metabolism.exit_code, Some(0), "Process should exit cleanly");
    assert!(metabolism.wall_time_ms > 0, "Wall time should be recorded");

    // On many systems, for a process as fast as echo, CPU time might be 0 or 1.
    // We just verify the field exists and is accessible.
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

    // Sleep for 10 seconds (we will kill it before then)
    let binary = PathBuf::from("/bin/sleep");

    // We need to pass arguments to sleep, but nucleus::activate currently only takes a binary path
    // and sets env vars.
    // TRICK: Create a wrapper script
    let script_path = run_dir.join("sleeper.sh");
    std::fs::write(&script_path, "#!/bin/sh\n/bin/sleep 10")?;
    use std::os::unix::fs::PermissionsExt;
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

    // Verify it didn't finish cleanly
    assert!(metabolism.exit_code.is_none() || metabolism.exit_code != Some(0));

    // Verify we still tracked time
    assert!(
        metabolism.wall_time_ms >= 100,
        "Should have tracked at least 100ms wall time"
    );

    Ok(())
}
