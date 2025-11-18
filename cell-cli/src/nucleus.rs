use anyhow::{bail, Context, Result};
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::os::unix::io::{AsRawFd, FromRawFd};
use std::os::unix::net::UnixListener;
use std::os::unix::process::CommandExt;
use std::path::Path;
use std::process::{Command, Stdio};

/// Wrapper: keep the *same* socket fd open, exec real binary on first connection.
/// The real binary simply starts accepting on the inherited fd.
pub fn run_nucleus(socket_path: &Path, real_binary: &Path) -> Result<()> {
    if !real_binary.is_file() {
        bail!("real binary not found: {}", real_binary.display());
    }

    // 1. ensure run/ exists
    let run_dir = socket_path.parent().unwrap();
    fs::create_dir_all(run_dir)?;

    // 2. open log file *before* we might exec
    let log_file = OpenOptions::new()
        .create(true)
        .append(true)
        .open(run_dir.join("service.log"))
        .context("open service.log")?;

    // 3. bind / listen (remove stale)
    let _ = fs::remove_file(socket_path);
    let listener = UnixListener::bind(socket_path).context("bind socket")?;
    listener.set_nonblocking(false)?;

    // 4. duplicate the fd so it stays open across exec
    let listen_fd = listener.as_raw_fd();
    let cloned_fd = unsafe { libc::dup(listen_fd) };
    if cloned_fd < 0 {
        bail!("dup failed");
    }

    // 5. wait for first client (blocks)
    let (_stream, _addr) = listener.accept().context("accept")?;

    // 6. exec real binary, inheriting the duplicated fd
    let mut cmd = Command::new(real_binary);
    unsafe {
        cmd.env("CELL_SOCKET_FD", cloned_fd.to_string())
            .env("CELL_SOCKET_PATH", socket_path) // still useful for rebuilds
            .stdin(Stdio::null())
            .stdout(Stdio::inherit())
            .stderr(log_file.try_clone().context("clone log fd")?)
            .pre_exec(move || {
                // clear CLOEXEC on the duplicated listener fd
                libc::fcntl(cloned_fd, libc::F_SETFD, 0);
                Ok(())
            });
    };

    // 7. we never return on success
    let err = cmd.exec();
    Err(anyhow::Error::from(err)).context("exec failed")
}
