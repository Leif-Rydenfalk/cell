use anyhow::Result;
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::os::unix::io::AsRawFd;
use std::os::unix::net::UnixListener;
use std::os::unix::process::CommandExt;
use std::path::Path;
use std::process::{Command, Stdio};

#[cfg(target_os = "linux")]
use cgroups_rs::{cgroup_builder::CgroupBuilder, hierarchies, CgroupPid};

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

pub fn run_nucleus(socket_path: &Path, real_binary: &Path, router_path: &Path) -> Result<()> {
    let run_dir = socket_path.parent().unwrap();
    fs::create_dir_all(run_dir)?;

    log(
        run_dir,
        &format!("Nucleus start. Bin: {}", real_binary.display()),
    )?;

    if socket_path.exists() {
        fs::remove_file(socket_path)?;
    }
    let listener = UnixListener::bind(socket_path)?;
    listener.set_nonblocking(false)?;

    let listen_fd = listener.as_raw_fd();
    let cloned_fd = unsafe { libc::dup(listen_fd) };

    let flags = unsafe { libc::fcntl(cloned_fd, libc::F_GETFD) };
    if flags >= 0 {
        unsafe { libc::fcntl(cloned_fd, libc::F_SETFD, flags & !libc::FD_CLOEXEC) };
    }

    let log_file = OpenOptions::new()
        .create(true)
        .append(true)
        .open(run_dir.join("service.log"))?;

    // --- CGROUPS ISOLATION (Soft Fail) ---
    #[cfg(target_os = "linux")]
    let cgroup = {
        let hier = hierarchies::auto();
        let gname = format!("cell_{}", std::process::id());

        // Try to build cgroup. If it fails (Permission Denied), log and return None.
        match CgroupBuilder::new(&gname)
            .memory()
            .memory_hard_limit(1024 * 1024 * 1024) // 1GB Limit
            .done()
            .cpu()
            .shares(512)
            .done()
            .build(hier)
        {
            Ok(cg) => Some(cg),
            Err(e) => {
                let _ = log(
                    run_dir,
                    &format!(
                        "WARNING: Cgroup creation failed (running without isolation): {}",
                        e
                    ),
                );
                None
            }
        }
    };

    let mut cmd = Command::new(real_binary);
    unsafe {
        cmd.env("CELL_SOCKET_FD", cloned_fd.to_string())
            .env("CELL_SOCKET_PATH", socket_path)
            .env("CELL_ROUTER_SOCK", router_path)
            .stdin(Stdio::null())
            .stdout(Stdio::inherit())
            .stderr(log_file.try_clone()?)
            .envs(std::env::vars())
            .pre_exec(move || {
                #[cfg(target_os = "linux")]
                {
                    // Only try to add task if cgroup exists
                    if let Some(ref cg) = cgroup {
                        let pid = CgroupPid::from(libc::getpid() as u64);
                        let _ = cg.add_task(pid);
                    }
                }
                Ok(())
            });
    }

    let err = cmd.exec();
    log(run_dir, &format!("FATAL: exec failed: {}", err))?;
    Err(anyhow::Error::from(err))
}
