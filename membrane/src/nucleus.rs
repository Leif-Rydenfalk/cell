use anyhow::{Context, Result};
use std::fs::OpenOptions;
use std::os::unix::io::AsRawFd;
use std::os::unix::net::UnixListener;
use std::path::Path;
use std::process::{Command, Stdio};
use std::time::SystemTime;

#[cfg(target_os = "linux")]
use cgroups_rs::{cgroup_builder::CgroupBuilder, hierarchies, CgroupPid};

fn sys_log(level: &str, msg: &str) {
    let timestamp = humantime::format_rfc3339_seconds(SystemTime::now());
    eprintln!("[{}] [{}] [NUCLEUS] {}", timestamp, level, msg);
}

/// Activates the cell nucleus (spawns the worker binary).
/// Handles Socket Activation and Resource Isolation.
pub fn activate(cell_sock: &Path, binary: &Path, golgi_sock: &Path) -> Result<()> {
    let run_dir = cell_sock.parent().unwrap();

    // 1. Setup Logging
    let log_path = run_dir.join("service.log");
    let log_file = OpenOptions::new()
        .create(true)
        .append(true)
        .open(&log_path)
        .context("Failed to open service log file")?;

    // 2. Socket Activation (Bind before spawn)
    if cell_sock.exists() {
        std::fs::remove_file(cell_sock).context("Failed to cleanup old socket")?;
    }
    let listener = UnixListener::bind(cell_sock).context("Failed to bind service socket")?;
    listener.set_nonblocking(false)?;
    let fd = listener.as_raw_fd();

    // 3. Configure Cgroups (Linux Only)
    #[cfg(target_os = "linux")]
    let cgroup = {
        let hier = hierarchies::auto();
        let gname = format!("cell_{}", std::process::id());

        // Default Limits: 1GB RAM, 1 CPU Share
        match CgroupBuilder::new(&gname)
            .memory()
            .memory_hard_limit(1024 * 1024 * 1024)
            .done()
            .cpu()
            .shares(512)
            .done()
            .build(hier)
        {
            Ok(cg) => Some(cg),
            Err(e) => {
                sys_log(
                    "WARN",
                    &format!("Failed to create cgroup (running unconstrained): {}", e),
                );
                None
            }
        }
    };

    sys_log("INFO", &format!("Spawning binary: {}", binary.display()));

    // 4. Prepare Command
    let mut cmd = Command::new(binary);
    cmd.env("CELL_SOCKET_FD", fd.to_string())
        .env("CELL_GOLGI_SOCK", golgi_sock)
        .stdin(Stdio::null())
        .stdout(Stdio::null()) // Redirect stdout to null (logs go to service.log via stderr)
        .stderr(Stdio::from(log_file));

    // 5. Apply Isolation Hook
    #[cfg(target_os = "linux")]
    if let Some(cg) = &cgroup {
        unsafe {
            cmd.pre_exec(move || {
                let pid = CgroupPid::from(libc::getpid() as u64);
                if let Err(e) = cg.add_task(pid) {
                    eprintln!("Failed to join cgroup: {}", e);
                }
                Ok(())
            });
        }
    }

    // 6. Spawn
    let child = cmd.spawn().context("Failed to exec nucleus binary")?;

    sys_log(
        "INFO",
        &format!(
            "Nucleus active. PID: {}. Log: {}",
            child.id(),
            log_path.display()
        ),
    );

    // IMPORTANT: Prevent Rust from closing the FD in the parent,
    // ensuring it remains open for the child to inherit.
    std::mem::forget(listener);

    Ok(())
}
