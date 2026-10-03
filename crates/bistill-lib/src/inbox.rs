//! Inbox pages and the two list sections.
//!
//! [`parse_page`] reads one Bitbucket inbox page. [`classify`] keeps OPEN pull
//! requests, drops merged and declined rows, and splits the rest into Needs
//! review and Waiting.

use crate::Error;
use json::Value;

/// One inbox page.
#[derive(Debug)]
pub struct InboxPage {
    /// Pull requests on this page.
    pub values: Vec<PullRequest>,
    /// `size`.
    pub size: u64,
    /// Whether another page follows.
    pub end: PageEnd,
}

/// Pagination after this page.
#[derive(Debug)]
pub enum PageEnd {
    /// `isLastPage` is true.
    Last,
    /// `nextPageStart` when another page exists.
    More { next_page_start: u64 },
}

/// A pull request from an inbox page, before section classification.
#[derive(Debug)]
pub struct PullRequest {
    /// Bitbucket pull request id.
    pub number: u64,
    /// Title.
    pub title: String,
    /// `OPEN`, `MERGED`, or `DECLINED`.
    pub state: State,
    /// `author.user`.
    pub author: UserRef,
    /// Reviewers and their status.
    pub reviewers: Vec<Reviewer>,
    /// Project key. Personal projects use `~slug`.
    pub project: String,
    /// Repository slug.
    pub repo: String,
    /// Source branch `displayId`.
    pub from_branch: String,
    /// `fromRef.latestCommit`, when the page includes it.
    pub from_commit: Option<String>,
    /// Destination branch `displayId`.
    pub to_branch: String,
    /// `createdDate` in epoch milliseconds.
    pub created_ms: u64,
    /// `updatedDate` in epoch milliseconds.
    pub updated_ms: u64,
    /// UI href from `links`, when one is present.
    pub ui_href: Option<String>,
    /// Draft pull request.
    pub draft: bool,
}

/// `author.user`.
#[derive(Debug)]
pub struct UserRef {
    /// `displayName`.
    pub name: String,
    /// `slug`.
    pub slug: String,
}

/// One reviewer.
#[derive(Debug)]
pub struct Reviewer {
    /// `displayName`.
    pub name: String,
    /// `slug`.
    pub slug: String,
    /// Reviewer status.
    pub status: ReviewStatus,
}

/// Pull request state.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum State {
    /// `OPEN`.
    Open,
    /// `MERGED`.
    Merged,
    /// `DECLINED`.
    Declined,
}

/// Whether enrich fields are on the row.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Enrichment {
    /// Enrich fields are present. List-only rows use the zero defaults.
    Ready,
    /// Enrich GETs have not been applied. Those JSON fields are omitted.
    Pending,
}

/// Build status for the from-ref commit.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Build {
    /// No build result.
    None,
    /// A successful build and none failed or in progress.
    Successful,
    /// A build is in progress and none failed.
    InProgress,
    /// A build failed.
    Failed,
}

/// Reviewer status.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ReviewStatus {
    /// `UNAPPROVED`.
    Unapproved,
    /// `NEEDS_WORK`.
    NeedsWork,
    /// `APPROVED`.
    Approved,
}

/// An OPEN pull request in one section.
#[derive(Debug)]
pub struct Row {
    /// `{project}/{repo}/{number}`.
    pub id: String,
    /// Project key.
    pub project: String,
    /// Repository slug.
    pub repo: String,
    /// Pull request id.
    pub number: u64,
    /// Title.
    pub title: String,
    /// Author display name.
    pub author: String,
    /// Source branch.
    pub from_branch: String,
    /// Destination branch.
    pub to_branch: String,
    /// Reviewers.
    pub reviewers: Vec<Reviewer>,
    /// `createdDate`.
    pub created_ms: u64,
    /// `updatedDate`.
    pub updated_ms: u64,
    /// HTML UI URL.
    pub html_url: String,
    /// Draft badge.
    pub draft: bool,
    /// Waiting row whose update is older than `stale_days`.
    pub stale: bool,
    /// Waiting row with a `NEEDS_WORK` reviewer.
    pub needs_work: bool,
    /// Threads waiting on the author. List rows use 0 until enrichment.
    pub unanswered_as_author: u64,
    /// Threads waiting on this reviewer. List rows use 0 until enrichment.
    pub unanswered_as_reviewer: u64,
    /// Open blocker comments. List rows use 0 until enrichment.
    pub open_tasks: u64,
    /// `pending` omits the enrich fields. List rows are `ready`.
    pub enrichment: Enrichment,
    /// From-ref build. List rows are `none`.
    pub build: Build,
    /// Merge `conflicted`. List rows are false.
    pub conflicted: bool,
    /// Merge `canMerge`. List rows are false.
    pub can_merge: bool,
    /// Lowercase hex record. Empty until the row is placed in a section.
    pub fingerprint: String,
}

/// The two sections, oldest update first.
#[derive(Debug)]
pub struct Sections {
    /// OPEN rows where this user's reviewer status is `UNAPPROVED` or `NEEDS_WORK`.
    pub needs_review: Vec<Row>,
    /// OPEN rows this user authored, when they are not in Needs review.
    pub waiting: Vec<Row>,
}

/// Parse an inbox page body.
pub fn parse_page(body: &[u8]) -> Result<InboxPage, Error> {
    let value = json::parse(body)?;
    Ok(InboxPage {
        values: pull_requests(&value)?,
        size: required_u64(&value, "size")?,
        end: page_end(&value)?,
    })
}

/// Classify `prs` for `username`.
///
/// Merged and declined rows are omitted. A user who is both author and reviewer
/// lands in Needs review when their reviewer status is `UNAPPROVED` or
/// `NEEDS_WORK`. `html_url` uses a UI link when the page has one, otherwise
/// `{base_url}/projects/{project}/repos/{repo}/pull-requests/{number}`.
/// `stale` is set on Waiting when `now_ms - updated_ms` exceeds `stale_days`.
pub fn classify(
    prs: &[PullRequest],
    username: &str,
    base_url: &str,
    now_ms: u64,
    stale_days: u32,
) -> Sections {
    let mut needs_review = Vec::new();
    let mut waiting = Vec::new();
    let mut seen = Vec::new();
    for pr in prs {
        let id = format!("{}/{}/{}", pr.project, pr.repo, pr.number);
        if seen.iter().any(|kept: &String| kept == &id) {
            continue;
        }
        match place(pr, username) {
            Some(Place::NeedsReview) => {
                seen.push(id);
                needs_review.push(row_from(pr, base_url, false, false));
            }
            Some(Place::Waiting) => {
                seen.push(id);
                let stale = age_ms(now_ms, pr.updated_ms) > day_ms(stale_days);
                let needs_work = pr
                    .reviewers
                    .iter()
                    .any(|reviewer| reviewer.status == ReviewStatus::NeedsWork);
                waiting.push(row_from(pr, base_url, stale, needs_work));
            }
            None => {}
        }
    }
    needs_review.sort_by_key(|row| row.updated_ms);
    waiting.sort_by_key(|row| row.updated_ms);
    Sections {
        needs_review,
        waiting,
    }
}

enum Place {
    NeedsReview,
    Waiting,
}

fn place(pr: &PullRequest, username: &str) -> Option<Place> {
    if pr.state != State::Open {
        return None;
    }
    if let Some(status) = pr
        .reviewers
        .iter()
        .find(|reviewer| reviewer.slug.eq_ignore_ascii_case(username))
        .map(|reviewer| reviewer.status)
    {
        if status == ReviewStatus::Unapproved || status == ReviewStatus::NeedsWork {
            return Some(Place::NeedsReview);
        }
    }
    if pr.author.slug.eq_ignore_ascii_case(username) {
        return Some(Place::Waiting);
    }
    None
}

fn row_from(pr: &PullRequest, base_url: &str, stale: bool, needs_work: bool) -> Row {
    Row {
        id: format!("{}/{}/{}", pr.project, pr.repo, pr.number),
        project: pr.project.clone(),
        repo: pr.repo.clone(),
        number: pr.number,
        title: pr.title.clone(),
        author: pr.author.name.clone(),
        from_branch: pr.from_branch.clone(),
        to_branch: pr.to_branch.clone(),
        reviewers: pr
            .reviewers
            .iter()
            .map(|reviewer| Reviewer {
                name: reviewer.name.clone(),
                slug: reviewer.slug.clone(),
                status: reviewer.status,
            })
            .collect(),
        created_ms: pr.created_ms,
        updated_ms: pr.updated_ms,
        html_url: pr
            .ui_href
            .clone()
            .unwrap_or_else(|| fallback_url(base_url, pr)),
        draft: pr.draft,
        stale,
        needs_work,
        unanswered_as_author: 0,
        unanswered_as_reviewer: 0,
        open_tasks: 0,
        enrichment: Enrichment::Ready,
        build: Build::None,
        conflicted: false,
        can_merge: false,
        fingerprint: String::new(),
    }
}

fn fallback_url(base_url: &str, pr: &PullRequest) -> String {
    format!(
        "{base_url}/projects/{}/repos/{}/pull-requests/{}",
        encode_segment(&pr.project),
        encode_segment(&pr.repo),
        pr.number
    )
}

fn age_ms(now_ms: u64, updated_ms: u64) -> u64 {
    now_ms.saturating_sub(updated_ms)
}

fn day_ms(days: u32) -> u64 {
    u64::from(days).saturating_mul(86_400_000)
}

fn pull_requests(value: &Value) -> Result<Vec<PullRequest>, Error> {
    let Some(items) = value.get("values") else {
        return Err(shape("missing values"));
    };
    let Some(items) = items.as_array() else {
        return Err(shape("values is not an array"));
    };
    items.iter().map(pull_request).collect()
}

fn pull_request(value: &Value) -> Result<PullRequest, Error> {
    let from_ref = nested(value, &["fromRef"])?;
    let repository = nested(from_ref, &["repository"])?;
    Ok(PullRequest {
        number: required_u64(value, "id")?,
        title: required_string(value, "title")?,
        state: state(required_string(value, "state")?.as_str())?,
        author: user_ref(nested(value, &["author", "user"])?)?,
        reviewers: reviewers(value)?,
        project: required_string(nested(repository, &["project"])?, "key")?,
        repo: required_string(repository, "slug")?,
        from_branch: required_string(from_ref, "displayId")?,
        from_commit: optional_string(from_ref, "latestCommit")?,
        to_branch: required_string(nested(value, &["toRef"])?, "displayId")?,
        created_ms: required_u64(value, "createdDate")?,
        updated_ms: required_u64(value, "updatedDate")?,
        ui_href: ui_href(value),
        draft: draft_flag(value)?,
    })
}

fn page_end(value: &Value) -> Result<PageEnd, Error> {
    if required_bool(value, "isLastPage")? {
        Ok(PageEnd::Last)
    } else {
        Ok(PageEnd::More {
            next_page_start: required_u64(value, "nextPageStart")?,
        })
    }
}

fn reviewers(value: &Value) -> Result<Vec<Reviewer>, Error> {
    let Some(items) = value.get("reviewers") else {
        return Ok(Vec::new());
    };
    let Some(items) = items.as_array() else {
        return Err(shape("reviewers is not an array"));
    };
    items.iter().map(reviewer).collect()
}

fn reviewer(value: &Value) -> Result<Reviewer, Error> {
    let user = user_ref(nested(value, &["user"])?)?;
    Ok(Reviewer {
        name: user.name,
        slug: user.slug,
        status: review_status(required_string(value, "status")?.as_str())?,
    })
}

fn user_ref(value: &Value) -> Result<UserRef, Error> {
    Ok(UserRef {
        name: required_string(value, "displayName")?,
        slug: required_string(value, "slug")?,
    })
}

fn state(text: &str) -> Result<State, Error> {
    match text {
        "OPEN" => Ok(State::Open),
        "MERGED" => Ok(State::Merged),
        "DECLINED" => Ok(State::Declined),
        _ => Err(shape("unknown state")),
    }
}

fn review_status(text: &str) -> Result<ReviewStatus, Error> {
    match text {
        "UNAPPROVED" => Ok(ReviewStatus::Unapproved),
        "NEEDS_WORK" => Ok(ReviewStatus::NeedsWork),
        "APPROVED" => Ok(ReviewStatus::Approved),
        _ => Err(shape("unknown status")),
    }
}

fn draft_flag(value: &Value) -> Result<bool, Error> {
    if let Some(flag) = value.get("draft") {
        return flag
            .as_bool()
            .ok_or_else(|| shape("draft is not a boolean"));
    }
    let Some(properties) = value.get("properties") else {
        return Ok(false);
    };
    match properties.get("draft") {
        None => Ok(false),
        Some(flag) => match flag.as_bool() {
            Some(bit) => Ok(bit),
            None => match flag.as_str() {
                Some("true") => Ok(true),
                Some("false") => Ok(false),
                _ => Err(shape("draft is not a boolean")),
            },
        },
    }
}

fn ui_href(value: &Value) -> Option<String> {
    let links = value.get("links")?.as_object()?;
    for (_, entry) in links {
        let Some(items) = entry.as_array() else {
            continue;
        };
        for item in items {
            let Some(href) = item.get("href").and_then(Value::as_str) else {
                continue;
            };
            if href.contains("/pull-requests/") && !href.contains("/rest/") {
                return Some(href.to_owned());
            }
        }
    }
    None
}

fn nested<'a>(value: &'a Value, path: &[&str]) -> Result<&'a Value, Error> {
    let mut current = value;
    for name in path {
        current = match current.get(name) {
            Some(child) => child,
            None => return Err(shape(&format!("missing {name}"))),
        };
    }
    Ok(current)
}

fn optional_string(value: &Value, name: &str) -> Result<Option<String>, Error> {
    match value.get(name) {
        None => Ok(None),
        Some(field) => match field.as_str() {
            Some(text) => Ok(Some(text.to_owned())),
            None => Err(shape(&format!("{name} is not a string"))),
        },
    }
}

fn required_string(value: &Value, name: &str) -> Result<String, Error> {
    match value.get(name) {
        Some(field) => match field.as_str() {
            Some(text) => Ok(text.to_owned()),
            None => Err(shape(&format!("{name} is not a string"))),
        },
        None => Err(shape(&format!("missing {name}"))),
    }
}

fn required_u64(value: &Value, name: &str) -> Result<u64, Error> {
    match value.get(name) {
        Some(field) => field
            .as_u64()
            .ok_or_else(|| shape(&format!("{name} is not an integer"))),
        None => Err(shape(&format!("missing {name}"))),
    }
}

fn required_bool(value: &Value, name: &str) -> Result<bool, Error> {
    match value.get(name) {
        Some(field) => field
            .as_bool()
            .ok_or_else(|| shape(&format!("{name} is not a boolean"))),
        None => Err(shape(&format!("missing {name}"))),
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

fn encode_segment(text: &str) -> String {
    let mut out = String::new();
    for byte in text.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(byte as char);
            }
            other => out.push_str(&format!("%{other:02X}")),
        }
    }
    out
}
