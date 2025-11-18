use anyhow::{bail, Context, Result};
use std::fs;
use std::os::unix::io::AsRawFd;
use std::os::unix::net::UnixListener;
use std::os::unix::process::CommandExt;
use std::path::Path;
use std::process::{Command, Stdio};

/// Wrap a cell binary: keep socket open, exec real binary on first connection.
/// The real binary inherits the listening socket fd and continues serving.
pub fn run_nucleus(socket_path: &Path, real_binary: &Path) -> Result<()> {
    if !real_binary.is_file() {
        bail!("real binary not found: {}", real_binary.display());
    }

    // 1.  create run/ directory if missing
    let run_dir = socket_path.parent().unwrap();
    fs::create_dir_all(run_dir)?;

    // 2.  bind the socket (delete stale)
    let _ = fs::remove_file(socket_path);
    let listener = UnixListener::bind(socket_path)?;
    listener.set_nonblocking(false)?;

    println!("🧬  Nucleus ready on {}", socket_path.display());

    // 3.  wait for first client
    let (_stream, _addr) = listener.accept()?;
    println!("🧬  First connection – exec real binary");

    // 4.  duplicate the listener fd so it survives exec
    let listen_fd = listener.as_raw_fd();
    let cloned_fd = unsafe { libc::dup(listen_fd) };
    if cloned_fd < 0 {
        bail!("dup failed");
    }

    // 5.  build command that inherits the duplicated fd
    let mut cmd = Command::new(real_binary);
    unsafe {
        cmd.env("CELL_SOCKET_PATH", socket_path)
            .stdin(Stdio::null())
            .stdout(Stdio::inherit())
            .stderr(Stdio::inherit())
            .pre_exec(move || {
                libc::fcntl(cloned_fd, libc::F_SETFD, 0);
                Ok(())
            });
    };

    // 6.  replace nucleus process with real binary
    let err = cmd.exec(); // only returns on error
    Err(anyhow::Error::from(err)).context("exec failed")
}
