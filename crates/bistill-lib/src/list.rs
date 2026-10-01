//! Inbox list for `ls`.
//!
//! [`list_inbox`] reads application-properties, the user, and both inbox roles.
//! It follows `nextPageStart` and retries a role in lowercase after HTTP 400.
//! The snapshot is list-only: enrichment counts stay 0, `build` is `none`.

use crate::Error;
use crate::fingerprint;
use crate::inbox::{
    Build, Enrichment, PageEnd, ReviewStatus, Reviewer, Row, Sections, classify, parse_page,
};
use crate::ping::{Client, Fetch, encode_segment, parse_product, parse_user};
use json::Value;

/// `status` on a snapshot.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SnapshotStatus {
    /// A fetch is in flight.
    Fetching,
    /// The list was applied.
    Ok,
    /// Bitbucket could not be reached.
    Unreachable,
    /// The token was rejected.
    Auth,
    /// TLS failed.
    Tls,
    /// HTTP 429.
    RateLimited,
    /// Another poll failure.
    Error,
}

/// How many OPEN rows stay list-only after the first 50.
pub const ENRICH_CAP: usize = 50;

/// A list snapshot. `ls --json` prints [`to_json`] of this value.
#[derive(Debug)]
pub struct Snapshot {
    /// When the list was read, epoch milliseconds.
    pub fetched_ms: u64,
    /// JSON `slug` from the user record.
    pub user_slug: String,
    /// `displayName` from the user record.
    pub user_name: String,
    /// Bitbucket `version`.
    pub bitbucket_version: String,
    /// Bitbucket `displayName`.
    pub bitbucket_name: String,
    /// Poll status. `ls` writes [`SnapshotStatus::Ok`].
    pub status: SnapshotStatus,
    /// When `status` last changed, epoch milliseconds.
    pub status_since_ms: u64,
    /// Needs review, oldest update first.
    pub needs_review: Vec<Row>,
    /// Waiting, oldest update first.
    pub waiting: Vec<Row>,
    /// OPEN rows beyond [`ENRICH_CAP`]. Those rows stay in the sections.
    pub truncated: u64,
    /// Poll interval in seconds.
    pub poll_seconds: u64,
}

/// A snapshot and the GETs that produced it, in call order.
pub struct Listed {
    /// The list.
    pub snapshot: Snapshot,
    /// Each request, including a lowercase retry.
    pub requests: Vec<host::Request>,
}

/// Read both inbox roles and build a list snapshot.
pub fn list_inbox(client: &Client, fetch: &mut dyn Fetch, now_ms: u64) -> Result<Listed, Error> {
    let mut requests = Vec::new();
    let product = fetch_ok(
        client,
        fetch,
        &mut requests,
        "/rest/api/1.0/application-properties",
    )?;
    let product = parse_product(&product)?;
    let user_path = format!("/rest/api/1.0/users/{}", encode_segment(client.username()));
    let user = fetch_ok(client, fetch, &mut requests, &user_path)?;
    let user = parse_user(client.username(), &user)?;
    let mut prs = role_pages(client, fetch, &mut requests, "REVIEWER")?;
    prs.extend(role_pages(client, fetch, &mut requests, "AUTHOR")?);
    let sections = classify(
        &prs,
        &user.slug,
        client.base_url(),
        now_ms,
        client.stale_days(),
    );
    let mut listed = Listed {
        snapshot: Snapshot {
            fetched_ms: now_ms,
            user_slug: user.slug,
            user_name: user.display_name,
            bitbucket_version: product.version,
            bitbucket_name: product.display_name,
            status: SnapshotStatus::Ok,
            status_since_ms: now_ms,
            truncated: truncated(&sections),
            poll_seconds: client.poll_seconds(),
            needs_review: sections.needs_review,
            waiting: sections.waiting,
        },
        requests,
    };
    fingerprint::stamp(&mut listed.snapshot);
    Ok(listed)
}

/// Needs review, plus Waiting rows whose author-thread or task count is above 0.
pub fn attention_count(snapshot: &Snapshot) -> u64 {
    let extra = snapshot
        .waiting
        .iter()
        .filter(|row| row.unanswered_as_author > 0 || row.open_tasks > 0)
        .count();
    snapshot.needs_review.len() as u64 + extra as u64
}

/// Compact snapshot JSON.
pub fn to_json(snapshot: &Snapshot) -> String {
    let bytes = json::to_vec(&snapshot_value(snapshot));
    String::from_utf8_lossy(&bytes).into_owned()
}

fn truncated(sections: &Sections) -> u64 {
    let open = sections.needs_review.len() + sections.waiting.len();
    open.saturating_sub(ENRICH_CAP) as u64
}

fn role_pages(
    client: &Client,
    fetch: &mut dyn Fetch,
    requests: &mut Vec<host::Request>,
    role: &str,
) -> Result<Vec<crate::inbox::PullRequest>, Error> {
    let mut role = role.to_owned();
    let mut start = 0u64;
    let mut out = Vec::new();
    loop {
        let path = format!("/rest/api/1.0/inbox/pull-requests?role={role}&start={start}&limit=25");
        let response = fetch_raw(client, fetch, requests, &path)?;
        if response.status == 400 && role.chars().any(|c| c.is_ascii_uppercase()) {
            role.make_ascii_lowercase();
            continue;
        }
        if response.status != 200 {
            return Err(Error::Http(response.status));
        }
        let page = parse_page(&response.body)?;
        out.extend(page.values);
        match page.end {
            PageEnd::Last => break,
            PageEnd::More { next_page_start } => {
                if next_page_start <= start {
                    return Err(shape("nextPageStart did not advance"));
                }
                start = next_page_start;
            }
        }
    }
    Ok(out)
}

fn fetch_ok(
    client: &Client,
    fetch: &mut dyn Fetch,
    requests: &mut Vec<host::Request>,
    path: &str,
) -> Result<Vec<u8>, Error> {
    let response = fetch_raw(client, fetch, requests, path)?;
    if response.status != 200 {
        return Err(Error::Http(response.status));
    }
    Ok(response.body)
}

fn fetch_raw(
    client: &Client,
    fetch: &mut dyn Fetch,
    requests: &mut Vec<host::Request>,
    path: &str,
) -> Result<host::Response, Error> {
    let request = client.request(path);
    requests.push(request.clone());
    Ok(fetch.get(&request)?)
}

fn snapshot_value(snapshot: &Snapshot) -> Value {
    object(vec![
        ("fetched_ms", number(snapshot.fetched_ms)),
        (
            "user",
            object(vec![
                ("slug", string(&snapshot.user_slug)),
                ("display_name", string(&snapshot.user_name)),
            ]),
        ),
        (
            "bitbucket",
            object(vec![
                ("version", string(&snapshot.bitbucket_version)),
                ("display_name", string(&snapshot.bitbucket_name)),
            ]),
        ),
        ("status", string(status_text(snapshot.status))),
        ("status_since_ms", number(snapshot.status_since_ms)),
        ("needs_review", rows(&snapshot.needs_review)),
        ("waiting", rows(&snapshot.waiting)),
        ("truncated", number(snapshot.truncated)),
        ("poll_seconds", number(snapshot.poll_seconds)),
    ])
}

fn rows(rows: &[Row]) -> Value {
    Value::Array(rows.iter().map(row_value).collect())
}

fn row_value(row: &Row) -> Value {
    let mut pairs = vec![
        ("id", string(&row.id)),
        ("project", string(&row.project)),
        ("repo", string(&row.repo)),
        ("number", number(row.number)),
        ("title", string(&row.title)),
        ("author", string(&row.author)),
        ("from_branch", string(&row.from_branch)),
        ("to_branch", string(&row.to_branch)),
        (
            "reviewers",
            Value::Array(row.reviewers.iter().map(reviewer_value).collect()),
        ),
        ("created_ms", number(row.created_ms)),
        ("updated_ms", number(row.updated_ms)),
        ("html_url", string(&row.html_url)),
        ("draft", boolean(row.draft)),
        ("enrichment", string(enrichment_text(row.enrichment))),
        ("stale", boolean(row.stale)),
        ("needs_work", boolean(row.needs_work)),
    ];
    if row.enrichment == Enrichment::Ready {
        pairs.extend([
            ("unanswered_as_author", number(row.unanswered_as_author)),
            ("unanswered_as_reviewer", number(row.unanswered_as_reviewer)),
            ("open_tasks", number(row.open_tasks)),
            ("build", string(build_text(row.build))),
            ("conflicted", boolean(row.conflicted)),
            ("can_merge", boolean(row.can_merge)),
        ]);
    }
    pairs.push(("fingerprint", string(&row.fingerprint)));
    object(pairs)
}

fn status_text(status: SnapshotStatus) -> &'static str {
    match status {
        SnapshotStatus::Fetching => "fetching",
        SnapshotStatus::Ok => "ok",
        SnapshotStatus::Unreachable => "unreachable",
        SnapshotStatus::Auth => "auth",
        SnapshotStatus::Tls => "tls",
        SnapshotStatus::RateLimited => "rate_limited",
        SnapshotStatus::Error => "error",
    }
}

fn enrichment_text(enrichment: Enrichment) -> &'static str {
    match enrichment {
        Enrichment::Ready => "ready",
        Enrichment::Pending => "pending",
    }
}

fn build_text(build: Build) -> &'static str {
    match build {
        Build::None => "none",
        Build::Successful => "successful",
        Build::InProgress => "in_progress",
        Build::Failed => "failed",
    }
}

fn reviewer_value(reviewer: &Reviewer) -> Value {
    object(vec![
        ("name", string(&reviewer.name)),
        ("slug", string(&reviewer.slug)),
        ("status", string(review_text(reviewer.status))),
    ])
}

fn review_text(status: ReviewStatus) -> &'static str {
    match status {
        ReviewStatus::Unapproved => "UNAPPROVED",
        ReviewStatus::NeedsWork => "NEEDS_WORK",
        ReviewStatus::Approved => "APPROVED",
    }
}

fn shape(message: &str) -> Error {
    Error::Json(json::Error {
        message: message.to_owned(),
        offset: 0,
        line: 1,
        column: 1,
    })
}

fn object(pairs: Vec<(&str, Value)>) -> Value {
    Value::Object(
        pairs
            .into_iter()
            .map(|(key, value)| (key.to_owned(), value))
            .collect(),
    )
}

fn string(text: &str) -> Value {
    Value::String(text.to_owned())
}

fn number(value: u64) -> Value {
    Value::Number(value.to_string())
}

fn boolean(value: bool) -> Value {
    Value::Bool(value)
}
