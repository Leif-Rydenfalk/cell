use anyhow::{Context, Result};
use std::fs::OpenOptions;
use std::os::unix::io::AsRawFd;
use std::os::unix::net::UnixListener;
use std::path::{Path, PathBuf};
use std::process::{ExitStatus, Stdio};
use std::time::{Instant, SystemTime};
use tokio::process::{Child, Command};

#[cfg(target_os = "linux")]
use cgroups_rs::{cgroup_builder::CgroupBuilder, hierarchies, CgroupPid};

fn sys_log(level: &str, msg: &str) {
    let timestamp = humantime::format_rfc3339_seconds(SystemTime::now());
    eprintln!("[{}] [{}] [NUCLEUS] {}", timestamp, level, msg);
}

// Stats returned after process death for billing
#[derive(Debug, Default, Clone)]
pub struct Metabolism {
    pub exit_code: Option<i32>,
    pub cpu_time_ms: u64, // Combined User + System
    pub max_rss_kb: u64,  // Max RAM usage
    pub wall_time_ms: u64,
}

pub struct ChildGuard {
    child: Child,
    start_time: Instant,
}

impl ChildGuard {
    pub fn take_pipes(
        &mut self,
    ) -> (
        Option<tokio::process::ChildStdout>,
        Option<tokio::process::ChildStderr>,
    ) {
        (self.child.stdout.take(), self.child.stderr.take())
    }

    /// Waits for process exit and calculates resource usage (The Bill)
    /// Modified to take &mut self so it doesn't move ownership during select!
    pub async fn wait(&mut self) -> Result<Metabolism> {
        let status = self.child.wait().await?;
        let wall_time = self.start_time.elapsed().as_millis() as u64;

        // Collect Resource Usage (rusage)
        // In a production Linux env, we would parse /sys/fs/cgroup/.../cpu.stat here.
        // For this implementation, we default to wall_time as the billing metric.
        let (cpu_ms, rss_kb) = self.read_cgroup_stats().unwrap_or((wall_time, 0));

        Ok(Metabolism {
            exit_code: status.code(),
            cpu_time_ms: cpu_ms,
            max_rss_kb: rss_kb,
            wall_time_ms: wall_time,
        })
    }

    /// Explicitly kill the process
    pub async fn kill(&mut self) -> std::io::Result<()> {
        self.child.start_kill()
    }

    // Attempt to read accurate stats from Linux Cgroups
    fn read_cgroup_stats(&self) -> Option<(u64, u64)> {
        #[cfg(target_os = "linux")]
        {
            if let Some(_id) = self.child.id() {
                // Placeholder for Cgroup parsing logic
                // In a full implementation, you would read cpu.stat here
                return Some((0, 0));
            }
        }
        None
    }
}

impl Drop for ChildGuard {
    fn drop(&mut self) {
        // Ensure child is cleaned up if the guard goes out of scope
        let _ = self.child.start_kill();
    }
}

#[derive(Clone)]
pub enum LogStrategy {
    File(PathBuf),
    Piped,
}

pub fn activate(
    cell_sock: &Path,
    log_strategy: LogStrategy,
    binary: &Path,
    golgi_sock: &Path,
) -> Result<ChildGuard> {
    // 1. Socket Activation
    if let Some(p) = cell_sock.parent() {
        std::fs::create_dir_all(p)?;
    }
    if cell_sock.exists() {
        std::fs::remove_file(cell_sock).context("Failed to cleanup old socket")?;
    }
    let listener = UnixListener::bind(cell_sock).context("Failed to bind service socket")?;
    listener.set_nonblocking(false)?;
    let fd = listener.as_raw_fd();

    // Remove FD_CLOEXEC
    unsafe {
        let flags = libc::fcntl(fd, libc::F_GETFD);
        if flags >= 0 {
            libc::fcntl(fd, libc::F_SETFD, flags & !libc::FD_CLOEXEC);
        }
    }

    // 2. Cgroups
    #[cfg(target_os = "linux")]
    let cgroup = setup_cgroup();

    // 3. Prepare Async Command
    let mut cmd = Command::new(binary);
    cmd.env("CELL_SOCKET_FD", fd.to_string())
        .env("CELL_GOLGI_SOCK", golgi_sock)
        .stdin(Stdio::null());

    match log_strategy {
        LogStrategy::File(path) => {
            if let Some(p) = path.parent() {
                std::fs::create_dir_all(p)?;
            }
            let log_file = OpenOptions::new()
                .create(true)
                .append(true)
                .open(&path)
                .context("Failed to open service log file")?;

            let log_file_err = log_file.try_clone().unwrap();

            cmd.stdout(Stdio::from(log_file));
            cmd.stderr(Stdio::from(log_file_err));

            sys_log("INFO", &format!("Nucleus active. Log: {}", path.display()));
        }
        LogStrategy::Piped => {
            cmd.stdout(Stdio::piped());
            cmd.stderr(Stdio::piped());
        }
    }

    // 4. Apply Isolation
    #[cfg(target_os = "linux")]
    unsafe {
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

    // 5. Spawn
    let child = cmd.spawn().context("Failed to exec nucleus binary")?;

    // Forget listener to keep FD open in child
    std::mem::forget(listener);

    Ok(ChildGuard {
        child,
        start_time: Instant::now(),
    })
}

#[cfg(target_os = "linux")]
fn setup_cgroup() -> Option<cgroups_rs::Cgroup> {
    let hier = hierarchies::auto();
    let gname = format!("cell_{}", std::process::id());
    match CgroupBuilder::new(&gname)
        .memory()
        // .memory_hard_limit(1024 * 1024 * 1024)
        .done()
        .build(hier)
    {
        Ok(cg) => Some(cg),
        Err(_) => None,
    }
}
