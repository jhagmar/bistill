//! Open a pull request in the browser with `xdg-open`.

use std::time::Duration;

/// The `host` value that opens `url`.
pub fn browser(url: &str) -> host::Open {
    host::Open::XdgOpen {
        program: "xdg-open".to_owned(),
        url: url.to_owned(),
        timeout: Duration::from_secs(15),
    }
}
