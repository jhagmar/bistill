//! Notification body for one fingerprint change.

use crate::Change;

/// Title of every OS notification.
pub const TITLE: &str = "Bistill";

/// `project/repo#number`, English reasons, then the pull request URL.
pub fn toast_body(change: &Change) -> String {
    let label = label(&change.id);
    let phrases = change
        .reasons
        .iter()
        .map(|reason| reason.phrase())
        .collect::<Vec<_>>()
        .join(", ");
    format!("{label} {phrases}\n{}", change.html_url)
}

fn label(id: &str) -> String {
    match id.rsplit_once('/') {
        Some((path, number)) => format!("{path}#{number}"),
        None => id.to_owned(),
    }
}
