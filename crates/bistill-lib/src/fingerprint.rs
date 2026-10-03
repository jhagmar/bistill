//! Row fingerprints and the on-disk snapshot.
//!
//! [`stamp`] writes the lowercase hex record from `idea/spec.md`. [`diff`]
//! turns two snapshots into reason tokens. [`write_snapshot`] stores the
//! compact JSON from [`crate::to_json`].

use crate::Error;
use crate::inbox::{Build, Enrichment, ReviewStatus, Reviewer, Row};
use crate::list::{Snapshot, SnapshotStatus};
use json::Value;
use std::path::Path;

const FILE_NAME: &str = "snapshot.json";

/// Which section a row belongs to.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Section {
    /// Needs review.
    NeedsReview,
    /// Waiting on others.
    Waiting,
}

/// Why a row changed since the previous snapshot.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Reason {
    /// New id in Needs review.
    NeedsReview,
    /// New id in Waiting.
    Waiting,
    /// `unanswered_as_author` or `unanswered_as_reviewer` increased.
    Unanswered,
    /// `open_tasks` increased.
    Tasks,
    /// A reviewer became `APPROVED` on a waiting row.
    Approved,
    /// A reviewer became `NEEDS_WORK`.
    NeedsWork,
    /// `build` became `failed`.
    BuildFailed,
    /// The id was in the previous snapshot and is absent now.
    Gone,
}

impl Reason {
    /// The spec token.
    pub fn token(self) -> &'static str {
        match self {
            Reason::NeedsReview => "needs_review",
            Reason::Waiting => "waiting",
            Reason::Unanswered => "unanswered",
            Reason::Tasks => "tasks",
            Reason::Approved => "approved",
            Reason::NeedsWork => "needs_work",
            Reason::BuildFailed => "build_failed",
            Reason::Gone => "gone",
        }
    }

    /// The English phrase in a notification body.
    pub fn phrase(self) -> &'static str {
        match self {
            Reason::NeedsReview => "needs review",
            Reason::Waiting => "waiting",
            Reason::Unanswered => "unanswered comments",
            Reason::Tasks => "open tasks",
            Reason::Approved => "approved",
            Reason::NeedsWork => "needs work",
            Reason::BuildFailed => "build failed",
            Reason::Gone => "merged or declined",
        }
    }
}

/// One pull request and the tokens that describe how it changed.
#[derive(Debug, Eq, PartialEq)]
pub struct Change {
    /// `{project}/{repo}/{number}`.
    pub id: String,
    /// HTML UI URL from the row that owns this change.
    pub html_url: String,
    /// Tokens in spec-table order.
    pub reasons: Vec<Reason>,
}

/// Fill [`Row::fingerprint`] for every row.
pub fn stamp(snapshot: &mut Snapshot) {
    let slug = snapshot.user_slug.clone();
    for row in &mut snapshot.needs_review {
        row.fingerprint = fingerprint(row, Section::NeedsReview, &slug);
    }
    for row in &mut snapshot.waiting {
        row.fingerprint = fingerprint(row, Section::Waiting, &slug);
    }
}

/// Reason tokens from `previous` to `current`. `previous` absent means every current row is new.
pub fn diff(previous: Option<&Snapshot>, current: &Snapshot) -> Vec<Change> {
    let mut changes = Vec::new();
    for (section, rows) in [
        (Section::NeedsReview, &current.needs_review),
        (Section::Waiting, &current.waiting),
    ] {
        for row in rows {
            match previous.and_then(|snapshot| locate(snapshot, &row.id)) {
                None => changes.push(Change {
                    id: row.id.clone(),
                    html_url: row.html_url.clone(),
                    reasons: vec![new_reason(section)],
                }),
                Some((_, prev)) => {
                    let reasons = reasons(prev, row, section);
                    if !reasons.is_empty() {
                        changes.push(Change {
                            id: row.id.clone(),
                            html_url: row.html_url.clone(),
                            reasons,
                        });
                    }
                }
            }
        }
    }
    if let Some(previous) = previous {
        for row in previous.needs_review.iter().chain(previous.waiting.iter()) {
            if locate(current, &row.id).is_none() {
                changes.push(Change {
                    id: row.id.clone(),
                    html_url: row.html_url.clone(),
                    reasons: vec![Reason::Gone],
                });
            }
        }
    }
    changes
}

/// Write `snapshot.json` under `dir`.
pub fn write_snapshot(dir: &Path, snapshot: &Snapshot) -> Result<(), Error> {
    std::fs::create_dir_all(dir)?;
    let path = dir.join(FILE_NAME);
    std::fs::write(path, crate::to_json(snapshot).as_bytes())?;
    Ok(())
}

/// Read `snapshot.json`. A missing file is `Ok(None)`.
pub fn read_snapshot(dir: &Path) -> Result<Option<Snapshot>, Error> {
    let path = dir.join(FILE_NAME);
    let bytes = match std::fs::read(&path) {
        Ok(bytes) => bytes,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(err) => return Err(Error::from(err)),
    };
    Ok(Some(parse_snapshot(&bytes)?))
}

/// Parse a snapshot object and check each stored fingerprint.
pub fn parse_snapshot(bytes: &[u8]) -> Result<Snapshot, Error> {
    let value = json::parse(bytes)?;
    if value.as_object().is_none() {
        return Err(shape("snapshot is not an object"));
    }
    let fetched_ms = req_u64(&value, "fetched_ms")?;
    let user = field_object(&value, "user")?;
    let user_slug = req_string(user, "slug")?;
    let user_name = req_string(user, "display_name")?;
    let bitbucket = field_object(&value, "bitbucket")?;
    let bitbucket_version = req_string(bitbucket, "version")?;
    let bitbucket_name = req_string(bitbucket, "display_name")?;
    let status = parse_status(&req_string(&value, "status")?)?;
    let status_since_ms = req_u64(&value, "status_since_ms")?;
    let needs_review = parse_rows(&value, "needs_review", &user_slug, Section::NeedsReview)?;
    let waiting = parse_rows(&value, "waiting", &user_slug, Section::Waiting)?;
    let truncated = req_u64(&value, "truncated")?;
    let poll_seconds = req_u64(&value, "poll_seconds")?;
    Ok(Snapshot {
        fetched_ms,
        user_slug,
        user_name,
        bitbucket_version,
        bitbucket_name,
        status,
        status_since_ms,
        needs_review,
        waiting,
        truncated,
        poll_seconds,
    })
}

fn fingerprint(row: &Row, section: Section, user_slug: &str) -> String {
    let mut text = String::new();
    push_line(&mut text, &row.updated_ms.to_string());
    push_line(
        &mut text,
        match section {
            Section::NeedsReview => "needs_review",
            Section::Waiting => "waiting",
        },
    );
    push_line(&mut text, user_status(row, user_slug));
    let mut reviewers: Vec<&Reviewer> = row.reviewers.iter().collect();
    reviewers.sort_by(|left, right| left.slug.as_bytes().cmp(right.slug.as_bytes()));
    for reviewer in reviewers {
        push_line(
            &mut text,
            &format!("{} {}", reviewer.slug, review_text(reviewer.status)),
        );
    }
    if row.enrichment == Enrichment::Ready {
        push_line(&mut text, &row.unanswered_as_author.to_string());
        push_line(&mut text, &row.unanswered_as_reviewer.to_string());
        push_line(&mut text, &row.open_tasks.to_string());
        push_line(&mut text, build_text(row.build));
        push_line(&mut text, bool_text(row.conflicted));
        push_line(&mut text, bool_text(row.can_merge));
    }
    hex_encode(text.as_bytes())
}

fn push_line(text: &mut String, line: &str) {
    text.push_str(line);
    text.push('\n');
}

fn user_status(row: &Row, user_slug: &str) -> &'static str {
    match row
        .reviewers
        .iter()
        .find(|reviewer| reviewer.slug.eq_ignore_ascii_case(user_slug))
    {
        Some(reviewer) => review_text(reviewer.status),
        None => "",
    }
}

fn new_reason(section: Section) -> Reason {
    match section {
        Section::NeedsReview => Reason::NeedsReview,
        Section::Waiting => Reason::Waiting,
    }
}

fn reasons(previous: &Row, current: &Row, section: Section) -> Vec<Reason> {
    let mut out = Vec::new();
    if current.unanswered_as_author > previous.unanswered_as_author
        || current.unanswered_as_reviewer > previous.unanswered_as_reviewer
    {
        out.push(Reason::Unanswered);
    }
    if current.open_tasks > previous.open_tasks {
        out.push(Reason::Tasks);
    }
    if section == Section::Waiting && became(current, previous, ReviewStatus::Approved) {
        out.push(Reason::Approved);
    }
    if became(current, previous, ReviewStatus::NeedsWork) {
        out.push(Reason::NeedsWork);
    }
    if current.build == Build::Failed && previous.build != Build::Failed {
        out.push(Reason::BuildFailed);
    }
    out
}

fn became(current: &Row, previous: &Row, status: ReviewStatus) -> bool {
    current.reviewers.iter().any(|reviewer| {
        reviewer.status == status && status_of(previous, &reviewer.slug) != Some(status)
    })
}

fn status_of(row: &Row, slug: &str) -> Option<ReviewStatus> {
    row.reviewers
        .iter()
        .find(|reviewer| reviewer.slug.eq_ignore_ascii_case(slug))
        .map(|reviewer| reviewer.status)
}

fn locate<'a>(snapshot: &'a Snapshot, id: &str) -> Option<(Section, &'a Row)> {
    let needs = snapshot
        .needs_review
        .iter()
        .find(|row| row.id == id)
        .map(|row| (Section::NeedsReview, row));
    needs.or_else(|| {
        snapshot
            .waiting
            .iter()
            .find(|row| row.id == id)
            .map(|row| (Section::Waiting, row))
    })
}

fn parse_rows(
    value: &Value,
    name: &str,
    user_slug: &str,
    section: Section,
) -> Result<Vec<Row>, Error> {
    let Some(items) = value.get(name) else {
        return Err(shape(&format!("missing {name}")));
    };
    let Some(items) = items.as_array() else {
        return Err(shape(&format!("{name} is not an array")));
    };
    items
        .iter()
        .map(|item| parse_row(item, user_slug, section))
        .collect()
}

fn parse_row(value: &Value, user_slug: &str, section: Section) -> Result<Row, Error> {
    if value.as_object().is_none() {
        return Err(shape("row is not an object"));
    }
    let enrichment = parse_enrichment(&req_string(value, "enrichment")?)?;
    let (unanswered_as_author, unanswered_as_reviewer, open_tasks, build, conflicted, can_merge) =
        if enrichment == Enrichment::Ready {
            (
                req_u64(value, "unanswered_as_author")?,
                req_u64(value, "unanswered_as_reviewer")?,
                req_u64(value, "open_tasks")?,
                parse_build(&req_string(value, "build")?)?,
                req_bool(value, "conflicted")?,
                req_bool(value, "can_merge")?,
            )
        } else {
            (0, 0, 0, Build::None, false, false)
        };
    let stored = req_string(value, "fingerprint")?;
    let row = Row {
        id: req_string(value, "id")?,
        project: req_string(value, "project")?,
        repo: req_string(value, "repo")?,
        number: req_u64(value, "number")?,
        title: req_string(value, "title")?,
        author: req_string(value, "author")?,
        from_branch: req_string(value, "from_branch")?,
        to_branch: req_string(value, "to_branch")?,
        reviewers: parse_reviewers(value)?,
        created_ms: req_u64(value, "created_ms")?,
        updated_ms: req_u64(value, "updated_ms")?,
        html_url: req_string(value, "html_url")?,
        draft: req_bool(value, "draft")?,
        stale: req_bool(value, "stale")?,
        needs_work: req_bool(value, "needs_work")?,
        unanswered_as_author,
        unanswered_as_reviewer,
        open_tasks,
        enrichment,
        build,
        conflicted,
        can_merge,
        fingerprint: String::new(),
    };
    let computed = fingerprint(&row, section, user_slug);
    if stored != computed {
        return Err(shape("fingerprint mismatch"));
    }
    let mut row = row;
    row.fingerprint = computed;
    Ok(row)
}

fn parse_reviewers(value: &Value) -> Result<Vec<Reviewer>, Error> {
    let Some(items) = value.get("reviewers") else {
        return Err(shape("missing reviewers"));
    };
    let Some(items) = items.as_array() else {
        return Err(shape("reviewers is not an array"));
    };
    items.iter().map(parse_reviewer).collect()
}

fn parse_reviewer(value: &Value) -> Result<Reviewer, Error> {
    if value.as_object().is_none() {
        return Err(shape("reviewer is not an object"));
    }
    Ok(Reviewer {
        name: req_string(value, "name")?,
        slug: req_string(value, "slug")?,
        status: parse_review(&req_string(value, "status")?)?,
    })
}

fn parse_status(text: &str) -> Result<SnapshotStatus, Error> {
    match text {
        "fetching" => Ok(SnapshotStatus::Fetching),
        "ok" => Ok(SnapshotStatus::Ok),
        "unreachable" => Ok(SnapshotStatus::Unreachable),
        "auth" => Ok(SnapshotStatus::Auth),
        "tls" => Ok(SnapshotStatus::Tls),
        "rate_limited" => Ok(SnapshotStatus::RateLimited),
        "error" => Ok(SnapshotStatus::Error),
        _ => Err(shape("unknown status")),
    }
}

fn parse_enrichment(text: &str) -> Result<Enrichment, Error> {
    match text {
        "ready" => Ok(Enrichment::Ready),
        "pending" => Ok(Enrichment::Pending),
        _ => Err(shape("unknown enrichment")),
    }
}

fn parse_build(text: &str) -> Result<Build, Error> {
    match text {
        "none" => Ok(Build::None),
        "successful" => Ok(Build::Successful),
        "in_progress" => Ok(Build::InProgress),
        "failed" => Ok(Build::Failed),
        _ => Err(shape("unknown build")),
    }
}

fn parse_review(text: &str) -> Result<ReviewStatus, Error> {
    match text {
        "UNAPPROVED" => Ok(ReviewStatus::Unapproved),
        "NEEDS_WORK" => Ok(ReviewStatus::NeedsWork),
        "APPROVED" => Ok(ReviewStatus::Approved),
        _ => Err(shape("unknown status")),
    }
}

fn field_object<'a>(value: &'a Value, name: &str) -> Result<&'a Value, Error> {
    let Some(child) = value.get(name) else {
        return Err(shape(&format!("missing {name}")));
    };
    if child.as_object().is_none() {
        return Err(shape(&format!("{name} is not an object")));
    }
    Ok(child)
}

fn req_string(value: &Value, name: &str) -> Result<String, Error> {
    match value.get(name) {
        Some(field) => match field.as_str() {
            Some(text) => Ok(text.to_owned()),
            None => Err(shape(&format!("{name} is not a string"))),
        },
        None => Err(shape(&format!("missing {name}"))),
    }
}

fn req_u64(value: &Value, name: &str) -> Result<u64, Error> {
    match value.get(name) {
        Some(field) => field
            .as_u64()
            .ok_or_else(|| shape(&format!("{name} is not an integer"))),
        None => Err(shape(&format!("missing {name}"))),
    }
}

fn req_bool(value: &Value, name: &str) -> Result<bool, Error> {
    match value.get(name) {
        Some(field) => field
            .as_bool()
            .ok_or_else(|| shape(&format!("{name} is not a boolean"))),
        None => Err(shape(&format!("missing {name}"))),
    }
}

fn review_text(status: ReviewStatus) -> &'static str {
    match status {
        ReviewStatus::Unapproved => "UNAPPROVED",
        ReviewStatus::NeedsWork => "NEEDS_WORK",
        ReviewStatus::Approved => "APPROVED",
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

fn bool_text(value: bool) -> &'static str {
    if value { "true" } else { "false" }
}

fn hex_encode(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut out = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        out.push(HEX[(byte >> 4) as usize] as char);
        out.push(HEX[(byte & 0x0f) as usize] as char);
    }
    out
}

fn shape(message: &str) -> Error {
    Error::Json(json::Error {
        message: message.to_owned(),
        offset: 0,
        line: 1,
        column: 1,
    })
}
