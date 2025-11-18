use anyhow::{bail, Context, Result};
use std::fs::{self, OpenOptions};
use std::io::Write; // <- NEW
use std::os::unix::io::AsRawFd;
use std::os::unix::net::UnixListener;
use std::os::unix::process::CommandExt;
use std::path::Path;
use std::process::{Command, Stdio};

/// tiny helper: timestamp + message → run/nucleus.log
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
    let run_dir = socket_path.parent().unwrap();
    fs::create_dir_all(run_dir)?;

    log(
        run_dir,
        &format!("nucleus starting, real binary: {}", real_binary.display()),
    )?;

    let _ = fs::remove_file(socket_path);
    let listener = UnixListener::bind(socket_path).context("bind socket")?;
    listener.set_nonblocking(false)?;
    log(run_dir, &format!("bound socket: {}", socket_path.display()))?;

    let listen_fd = listener.as_raw_fd();
    let cloned_fd = unsafe { libc::dup(listen_fd) };
    if cloned_fd < 0 {
        log(run_dir, "dup failed")?;
        bail!("dup failed");
    }
    log(
        run_dir,
        &format!("dup original fd {} → {}", listen_fd, cloned_fd),
    )?;

    // CLEAR CLOEXEC IMMEDIATELY
    unsafe { libc::fcntl(cloned_fd, libc::F_SETFD, 0) };
    log(run_dir, &format!("cleared CLOEXEC on fd {}", cloned_fd))?;

    log(
        run_dir,
        "dup'd listener fd, execing real binary immediately",
    )?;

    let log_file = OpenOptions::new()
        .create(true)
        .append(true)
        .open(run_dir.join("service.log"))
        .context("open service.log")?;

    let mut cmd = Command::new(real_binary);
    unsafe {
        cmd.env("CELL_SOCKET_FD", cloned_fd.to_string())
            .env("CELL_SOCKET_PATH", socket_path)
            .stdin(Stdio::null())
            .stdout(Stdio::inherit())
            .stderr(log_file.try_clone().context("clone log fd")?)
            .pre_exec(move || {
                // redundant but safe
                libc::fcntl(cloned_fd, libc::F_SETFD, 0);
                Ok(())
            });
    }

    let err = cmd.exec();
    log(run_dir, &format!("exec returned error: {}", err))?; // only on failure
    Err(anyhow::Error::from(err)).context("exec failed")
}
