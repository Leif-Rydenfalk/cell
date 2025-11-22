use anyhow::{Context, Result};
use std::fs::OpenOptions;
use std::os::unix::io::AsRawFd;
use std::os::unix::net::UnixListener;
use std::path::{Path, PathBuf};
use std::process::Stdio; // Enum definitions
use std::time::SystemTime;
use tokio::process::{Child, Command}; // Async Command

#[cfg(target_os = "linux")]
use cgroups_rs::{cgroup_builder::CgroupBuilder, hierarchies, CgroupPid};

fn sys_log(level: &str, msg: &str) {
    let timestamp = humantime::format_rfc3339_seconds(SystemTime::now());
    eprintln!("[{}] [{}] [NUCLEUS] {}", timestamp, level, msg);
}

/// A wrapper that kills the child process when it goes out of scope.
pub struct ChildGuard(Child);

impl ChildGuard {
    /// Extracts pipes from the async child. No conversion needed.
    pub fn take_pipes(
        &mut self,
    ) -> (
        Option<tokio::process::ChildStdout>,
        Option<tokio::process::ChildStderr>,
    ) {
        (self.0.stdout.take(), self.0.stderr.take())
    }
}

impl Drop for ChildGuard {
    fn drop(&mut self) {
        let _ = self.0.start_kill(); // Async kill signal
                                     // We can't await inside Drop, but start_kill() is non-blocking on Tokio.
                                     // The OS will reap the zombie eventually or we rely on Tokio runtime reaping.
    }
}

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
    // 1. Socket Activation (Sync IO is fine here, done once during startup)
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
            // Tokio's piped() is non-blocking and async-ready
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
        Err(_) => None,
    }
}
