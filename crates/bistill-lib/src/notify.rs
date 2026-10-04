//! The text of a notification when one pull request changes.

use crate::Change;

/// Title of every OS notification.
pub const TITLE: &str = "Bistill";

/// `project/repo#number`, English reasons, then the pull request URL.
pub fn toast_body(change: &Change) -> String {
    let label = label(&change.id);
    let phrases = change
        .reasons
        .iter()
        .map(|reason| match reason {
            crate::Reason::Gone if !change.gone_text.is_empty() => change.gone_text.as_str(),
            other => other.phrase(),
        })
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
