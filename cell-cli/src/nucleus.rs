use anyhow::{Context, Result};
use std::fs::OpenOptions;
use std::os::unix::io::AsRawFd;
use std::os::unix::io::RawFd;
use std::os::unix::net::UnixListener;
use std::path::Path;
use std::process::{Child, Command, Stdio};
use std::time::SystemTime;

#[cfg(target_os = "linux")]
use cgroups_rs::{cgroup_builder::CgroupBuilder, hierarchies, CgroupPid};

fn sys_log(level: &str, msg: &str) {
    let timestamp = humantime::format_rfc3339_seconds(SystemTime::now());
    eprintln!("[{}] [{}] [NUCLEUS] {}", timestamp, level, msg);
}

/// A wrapper that kills the child process when it goes out of scope.
pub struct ChildGuard(Child);

impl Drop for ChildGuard {
    fn drop(&mut self) {
        let _ = self.0.kill(); // Send SIGKILL
        let _ = self.0.wait(); // Reap zombie
                               // sys_log("INFO", "Nucleus child process terminated.");
    }
}

pub fn activate(cell_sock: &Path, binary: &Path, golgi_sock: &Path) -> Result<ChildGuard> {
    let run_dir = cell_sock.parent().unwrap();

    // 1. Setup Logging
    let log_path = run_dir.join("service.log");
    let log_file = OpenOptions::new()
        .create(true)
        .append(true)
        .open(&log_path)
        .context("Failed to open service log file")?;

    // Clone the file handle so we can use it for both stdout and stderr
    let log_file_stdout = log_file.try_clone().context("Failed to clone log handle")?;

    // 2. Socket Activation
    if cell_sock.exists() {
        std::fs::remove_file(cell_sock).context("Failed to cleanup old socket")?;
    }
    let listener = UnixListener::bind(cell_sock).context("Failed to bind service socket")?;
    listener.set_nonblocking(false)?;
    let fd = listener.as_raw_fd();

    // Fix: Remove FD_CLOEXEC flag
    unsafe {
        let flags = libc::fcntl(fd, libc::F_GETFD);
        if flags >= 0 {
            libc::fcntl(fd, libc::F_SETFD, flags & !libc::FD_CLOEXEC);
        }
    }

    // 3. Cgroups
    #[cfg(target_os = "linux")]
    let cgroup = setup_cgroup();

    sys_log("INFO", &format!("Spawning binary: {}", binary.display()));

    // 4. Prepare Command
    let mut cmd = Command::new(binary);
    cmd.env("CELL_SOCKET_FD", fd.to_string())
        .env("CELL_GOLGI_SOCK", golgi_sock)
        .stdin(Stdio::null())
        .stdout(Stdio::from(log_file_stdout)) // <--- FIX: Capture stdout
        .stderr(Stdio::from(log_file)); // <--- Capture stderr

    // 5. Apply Isolation
    #[cfg(target_os = "linux")]
    unsafe {
        use std::os::unix::process::CommandExt;
        if let Some(cg) = &cgroup {
            let cg_clone = cg.clone();
            cmd.pre_exec(move || {
                let pid = CgroupPid::from(libc::getpid() as u64);
                if let Err(e) = cg_clone.add_task(pid) {
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

    std::mem::forget(listener);

    Ok(ChildGuard(child))
}

#[cfg(target_os = "linux")]
fn setup_cgroup() -> Option<cgroups_rs::Cgroup> {
    let hier = hierarchies::auto();
    let gname = format!("cell_{}", std::process::id());
    match CgroupBuilder::new(&gname)
        .memory()
        .memory_hard_limit(1024 * 1024 * 1024)
        .done()
        .build(hier)
    {
        Ok(cg) => Some(cg),
        Err(_) => None, // Fallback silently on dev machines
    }
}
