//! A desktop notification on Linux, sent with `notify-send`.

use std::time::Duration;

pub(crate) const PROGRAM: &str = "notify-send";

pub(crate) fn desktop_toast(body: &str, _url: &str) -> host::Toast {
    host::Toast::NotifySend {
        program: PROGRAM.to_owned(),
        title: bistill_lib::TITLE.to_owned(),
        body: body.to_owned(),
        expire: Some(Duration::from_millis(10_000)),
        timeout: Duration::from_secs(5),
    }
}
