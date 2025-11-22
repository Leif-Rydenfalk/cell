pub mod antigens;
pub mod genesis;
pub mod golgi;
pub mod nucleus;
pub mod synapse;
pub mod vacuole;

use std::time::SystemTime;

pub fn sys_log(level: &str, msg: &str) {
    let timestamp = humantime::format_rfc3339_seconds(SystemTime::now());
    eprintln!("[{}] [{}] [MEMBRANE] {}", timestamp, level, msg);
}
