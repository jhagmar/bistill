//! The inbox list behind `ls` and `watch`.
//!
//! [`list_inbox`] reads application-properties, the user, and both inbox roles.
//! It follows `nextPageStart`, and if a role comes back HTTP 400 it tries that
//! role again in lowercase. The 50 oldest open pull requests get the extra
//! detail. The rest stay in the list with the plain defaults.

use std::collections::HashMap;

use crate::Error;
use crate::enrich::{self, Source};
use crate::fingerprint;
use crate::inbox::{
    Build, Enrichment, PageEnd, PullRequest, ReviewStatus, Reviewer, Row, Sections, State,
    classify, parse_page, parse_state,
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
#[derive(Clone, Debug)]
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

/// A failed list. `retry_after_ms` is set from `Retry-After` on HTTP 429.
#[derive(Debug)]
pub struct InboxFault {
    /// The failure.
    pub error: Error,
    /// Delay from `Retry-After`, in milliseconds.
    pub retry_after_ms: Option<u64>,
}

impl From<Error> for InboxFault {
    fn from(error: Error) -> Self {
        InboxFault {
            error,
            retry_after_ms: None,
        }
    }
}

/// Read both inbox roles and build a list snapshot.
/// `publish` runs after each applied inbox page and enrich reply.
pub fn list_inbox(
    client: &Client,
    fetch: &mut dyn Fetch,
    now_ms: u64,
    publish: &mut dyn FnMut(&Snapshot) -> Result<(), Error>,
) -> Result<Listed, Error> {
    poll_list(client, fetch, now_ms, publish).map_err(|fault| fault.error)
}

/// [`list_inbox`] plus `Retry-After` when the failing response carries it.
pub fn poll_list(
    client: &Client,
    fetch: &mut dyn Fetch,
    now_ms: u64,
    publish: &mut dyn FnMut(&Snapshot) -> Result<(), Error>,
) -> Result<Listed, InboxFault> {
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
    let mut prs = Vec::new();
    for role in ["REVIEWER", "AUTHOR"] {
        role_pages(client, fetch, &mut requests, role, &mut prs, &mut |rows| {
            let mut snapshot = assemble(client, &product, &user, rows, now_ms);
            fingerprint::stamp(&mut snapshot);
            publish(&snapshot).map_err(InboxFault::from)
        })?;
    }
    let sources = sources_of(&prs);
    let mut listed = Listed {
        snapshot: assemble(client, &product, &user, &prs, now_ms),
        requests,
    };
    enrich_listed(client, fetch, &mut listed, &sources, publish)?;
    fingerprint::stamp(&mut listed.snapshot);
    publish(&listed.snapshot)?;
    Ok(listed)
}

fn assemble(
    client: &Client,
    product: &crate::ping::Product,
    user: &crate::ping::User,
    prs: &[PullRequest],
    now_ms: u64,
) -> Snapshot {
    let sections = classify(
        prs,
        &user.slug,
        client.base_url(),
        now_ms,
        client.stale_days(),
    );
    Snapshot {
        fetched_ms: now_ms,
        user_slug: user.slug.clone(),
        user_name: user.display_name.clone(),
        bitbucket_version: product.version.clone(),
        bitbucket_name: product.display_name.clone(),
        status: SnapshotStatus::Ok,
        status_since_ms: now_ms,
        truncated: truncated(&sections),
        poll_seconds: client.poll_seconds(),
        needs_review: sections.needs_review,
        waiting: sections.waiting,
    }
}

/// One GET of a pull request that left the inbox. A failure leaves the default phrase.
pub fn clarify_gone(client: &Client, fetch: &mut dyn Fetch, changes: &mut [crate::Change]) {
    for change in changes {
        if change.reasons.contains(&crate::Reason::Gone) {
            change.gone_text = gone_word(client, fetch, &change.id);
        }
    }
}

fn gone_word(client: &Client, fetch: &mut dyn Fetch, id: &str) -> String {
    let Some((project, repo, number)) = split_id(id) else {
        return String::new();
    };
    let path = format!(
        "/rest/api/1.0/projects/{}/repos/{}/pull-requests/{number}",
        encode_segment(project),
        encode_segment(repo),
    );
    let mut requests = Vec::new();
    let response = match fetch_raw(client, fetch, &mut requests, &path) {
        Ok(response) => response,
        Err(_) => return String::new(),
    };
    if response.status != 200 {
        return String::new();
    }
    match pr_state(&response.body) {
        Ok(State::Merged) => "merged".to_owned(),
        Ok(State::Declined) => "declined".to_owned(),
        Ok(State::Open) | Err(_) => String::new(),
    }
}

fn split_id(id: &str) -> Option<(&str, &str, &str)> {
    let (path, number) = id.rsplit_once('/')?;
    let (project, repo) = path.rsplit_once('/')?;
    Some((project, repo, number))
}

fn pr_state(body: &[u8]) -> Result<State, Error> {
    let value = json::parse(body)?;
    match value.get("state").and_then(|state| state.as_str()) {
        Some(text) => parse_state(text),
        None => Err(shape("missing state")),
    }
}

fn sources_of(prs: &[PullRequest]) -> HashMap<String, Source> {
    let mut sources = HashMap::new();
    for pr in prs {
        sources.insert(
            format!("{}/{}/{}", pr.project, pr.repo, pr.number),
            Source {
                author_slug: pr.author.slug.clone(),
                from_commit: pr.from_commit.clone(),
            },
        );
    }
    sources
}

fn enrich_listed(
    client: &Client,
    fetch: &mut dyn Fetch,
    listed: &mut Listed,
    sources: &HashMap<String, Source>,
    publish: &mut dyn FnMut(&Snapshot) -> Result<(), Error>,
) -> Result<(), InboxFault> {
    let user_slug = listed.snapshot.user_slug.clone();
    let chosen = enrich::slots(
        &listed.snapshot.needs_review,
        &listed.snapshot.waiting,
        ENRICH_CAP,
    );
    for slot in chosen {
        let (project, repo, number, source) = target(&listed.snapshot, slot, sources);
        let filled = {
            let Listed { snapshot, requests } = listed;
            let mut get =
                |path: &str| fetch_raw(client, fetch, requests, path).map_err(InboxFault::from);
            let mut on = |progress| {
                note_progress(snapshot, slot, progress);
                fingerprint::stamp(snapshot);
                publish(snapshot).map_err(InboxFault::from)
            };
            enrich::fetch(
                &mut get,
                &enrich::Query {
                    project: &project,
                    repo: &repo,
                    number,
                    author_slug: &source.author_slug,
                    user_slug: &user_slug,
                    from_commit: source.from_commit.as_deref(),
                },
                &mut on,
            )?
        };
        match slot {
            enrich::Slot::Needs(index) => {
                enrich::write(&mut listed.snapshot.needs_review[index], filled);
            }
            enrich::Slot::Waiting(index) => {
                enrich::write(&mut listed.snapshot.waiting[index], filled);
            }
        }
    }
    Ok(())
}

fn note_progress(snapshot: &mut Snapshot, slot: enrich::Slot, progress: enrich::Progress) {
    let row = match slot {
        enrich::Slot::Needs(index) => &mut snapshot.needs_review[index],
        enrich::Slot::Waiting(index) => &mut snapshot.waiting[index],
    };
    match progress {
        enrich::Progress::Activities { author, reviewer } => {
            row.unanswered_as_author = author;
            row.unanswered_as_reviewer = reviewer;
        }
        enrich::Progress::Tasks { open } => row.open_tasks = open,
        enrich::Progress::Build(build) => row.build = build,
        enrich::Progress::Merge {
            conflicted,
            can_merge,
        } => {
            row.conflicted = conflicted;
            row.can_merge = can_merge;
        }
    }
}

fn target(
    snapshot: &Snapshot,
    slot: enrich::Slot,
    sources: &HashMap<String, Source>,
) -> (String, String, u64, Source) {
    let row = match slot {
        enrich::Slot::Needs(index) => &snapshot.needs_review[index],
        enrich::Slot::Waiting(index) => &snapshot.waiting[index],
    };
    let source = sources[&row.id].clone();
    (row.project.clone(), row.repo.clone(), row.number, source)
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
    prs: &mut Vec<PullRequest>,
    after_page: &mut dyn FnMut(&[PullRequest]) -> Result<(), InboxFault>,
) -> Result<(), InboxFault> {
    let mut role = role.to_owned();
    let mut start = 0u64;
    loop {
        let path = format!("/rest/api/1.0/inbox/pull-requests?role={role}&start={start}&limit=25");
        let response = fetch_raw(client, fetch, requests, &path)?;
        if response.status == 400 && role.chars().any(|c| c.is_ascii_uppercase()) {
            role.make_ascii_lowercase();
            continue;
        }
        if response.status != 200 {
            return Err(http_fault(response.status, response.retry_after));
        }
        let page = parse_page(&response.body)?;
        prs.extend(page.values);
        after_page(prs)?;
        match page.end {
            PageEnd::Last => break,
            PageEnd::More { next_page_start } => {
                if next_page_start <= start {
                    return Err(shape("nextPageStart did not advance").into());
                }
                start = next_page_start;
            }
        }
    }
    Ok(())
}

fn fetch_ok(
    client: &Client,
    fetch: &mut dyn Fetch,
    requests: &mut Vec<host::Request>,
    path: &str,
) -> Result<Vec<u8>, InboxFault> {
    let response = fetch_raw(client, fetch, requests, path)?;
    if response.status != 200 {
        return Err(http_fault(response.status, response.retry_after));
    }
    Ok(response.body)
}

fn http_fault(status: u16, retry_after: Option<u64>) -> InboxFault {
    let retry_after_ms = if status == 429 {
        retry_after.map(|seconds| seconds.saturating_mul(1000))
    } else {
        None
    };
    InboxFault {
        error: Error::Http(status),
        retry_after_ms,
    }
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
