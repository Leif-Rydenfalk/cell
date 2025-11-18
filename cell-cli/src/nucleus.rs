use anyhow::{bail, Context, Result};
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::os::unix::io::AsRawFd;
use std::os::unix::net::UnixListener;
use std::os::unix::process::CommandExt;
use std::path::Path;
use std::process::{Command, Stdio};

/// Append a timestamped message to run/nucleus.log
fn log(run_dir: &Path, msg: &str) -> Result<()> {
    let mut f = OpenOptions::new()
        .create(true)
        .append(true)
        .open(run_dir.join("nucleus.log"))?;
    writeln!(
        f,
        "[{}] {}",
        humantime::format_rfc3339(std::time::SystemTime::now()),
        msg
    )?;
    Ok(())
}

pub fn run_nucleus(socket_path: &Path, real_binary: &Path) -> Result<()> {
    let run_dir = socket_path
        .parent()
        .ok_or_else(|| anyhow::anyhow!("Socket path has no parent directory"))?;

    fs::create_dir_all(run_dir).context("Failed to create run directory")?;

    log(
        run_dir,
        &format!("Nucleus starting. Real binary: {}", real_binary.display()),
    )?;

    // Ensure clean slate for socket
    if socket_path.exists() {
        fs::remove_file(socket_path).context("Failed to remove existing socket file")?;
    }

    let listener = UnixListener::bind(socket_path).context("Failed to bind Unix socket")?;
    // Important: We must set non-blocking to false so the accept loop in the SDK blocks efficiently
    listener
        .set_nonblocking(false)
        .context("Failed to set socket to blocking mode")?;

    log(
        run_dir,
        &format!("Bound socket at: {}", socket_path.display()),
    )?;

    // Duplicate the file descriptor to pass to the child process
    let listen_fd = listener.as_raw_fd();
    let cloned_fd = unsafe { libc::dup(listen_fd) };
    if cloned_fd < 0 {
        let err = std::io::Error::last_os_error();
        log(run_dir, &format!("dup failed: {}", err))?;
        bail!("Failed to dup socket fd: {}", err);
    }

    log(
        run_dir,
        &format!("Dup'd original fd {} -> {}", listen_fd, cloned_fd),
    )?;

    // Clear CLOEXEC on the dup'd FD so it survives the exec() call
    let flags = unsafe { libc::fcntl(cloned_fd, libc::F_GETFD) };
    if flags >= 0 {
        unsafe { libc::fcntl(cloned_fd, libc::F_SETFD, flags & !libc::FD_CLOEXEC) };
    }
    log(run_dir, &format!("Cleared CLOEXEC on fd {}", cloned_fd))?;

    let log_file = OpenOptions::new()
        .create(true)
        .append(true)
        .open(run_dir.join("service.log"))
        .context("Failed to open service.log")?;

    log(run_dir, "Execing real binary now...")?;

    let mut cmd = Command::new(real_binary);
    unsafe {
        cmd.env("CELL_SOCKET_FD", cloned_fd.to_string())
            .env("CELL_SOCKET_PATH", socket_path)
            .stdin(Stdio::null())
            .stdout(Stdio::inherit()) // Let stdout go to terminal/parent for now
            .stderr(log_file.try_clone().context("Failed to clone log fd")?)
            // CRITICAL: Inherit environment variables (e.g., CELL_DEP_*_SOCK) from the CLI
            .envs(std::env::vars())
            .pre_exec(move || {
                // Redundant safety measure: ensure FD is open in child
                // (The primary mechanism is clearing CLOEXEC above)
                Ok(())
            });
    }

    // This replaces the current process image. It should not return.
    let err = cmd.exec();

    // If we are here, exec failed
    log(run_dir, &format!("FATAL: exec failed: {}", err))?;
    Err(anyhow::Error::from(err)).context("Failed to exec real binary")
}
