//! Whether a pid is running on Linux.

use std::path::Path;

pub(crate) fn pid_alive(pid: u32) -> bool {
    pid != 0 && Path::new(&format!("/proc/{pid}")).is_dir()
}
