//! A desktop notification on Windows, shown with a PowerShell toast.

use std::time::Duration;

pub(crate) const PROGRAM: &str = "powershell.exe";

pub(crate) fn desktop_toast(body: &str, url: &str) -> host::Toast {
    host::Toast::PowerShell {
        program: PROGRAM.to_owned(),
        title: bistill_lib::TITLE.to_owned(),
        body: body.to_owned(),
        url: Some(url.to_owned()),
        timeout: Duration::from_secs(5),
    }
}
