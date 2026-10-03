//! Open a pull request with PowerShell.

use std::time::Duration;

/// The `host` value that opens `url`.
pub fn browser(url: &str) -> host::Open {
    host::Open::WindowsStart {
        program: "powershell.exe".to_owned(),
        url: url.to_owned(),
        timeout: Duration::from_secs(15),
    }
}
