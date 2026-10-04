//! Extra detail for one pull request: activity, open tasks, build status, and merge.
//!
//! The list calls [`fetch`] once for each pull request it is filling in. A 404
//! on build status leaves `build` as `none`. If `count=true` on blocker
//! comments comes back HTTP 400, the code pages through the comments instead.

use std::cmp::Ordering;

use crate::Error;
use crate::inbox::{Build, Row};
use crate::list::InboxFault;
use crate::ping::encode_segment;
use json::Value;

/// Author slug and from-ref commit copied off the inbox page.
#[derive(Clone)]
pub(crate) struct Source {
    pub author_slug: String,
    pub from_commit: Option<String>,
}

/// Which section holds an enriched row.
#[derive(Clone, Copy)]
pub(crate) enum Slot {
    /// Index into Needs review.
    Needs(usize),
    /// Index into Waiting.
    Waiting(usize),
}

/// Counts and flags from one row's enrich GETs.
#[derive(Debug)]
pub(crate) struct Filled {
    /// Threads whose latest message is from someone else.
    pub unanswered_as_author: u64,
    /// Threads this user started that the author has not answered.
    pub unanswered_as_reviewer: u64,
    /// Open blocker comments.
    pub open_tasks: u64,
    /// Overall build for the from-ref commit.
    pub build: Build,
    /// Merge `conflicted`.
    pub conflicted: bool,
    /// Merge `canMerge`.
    pub can_merge: bool,
}

/// Oldest `updated_ms` first. Needs review precedes Waiting on a tie.
pub(crate) fn slots(needs: &[Row], waiting: &[Row], cap: usize) -> Vec<Slot> {
    let mut ranked = Vec::new();
    for (index, row) in needs.iter().enumerate() {
        ranked.push((row.updated_ms, 0u8, index, Slot::Needs(index)));
    }
    for (index, row) in waiting.iter().enumerate() {
        ranked.push((row.updated_ms, 1u8, index, Slot::Waiting(index)));
    }
    ranked.sort_by(|left, right| rank_cmp((left.0, left.1, left.2), (right.0, right.1, right.2)));
    ranked.truncate(cap);
    ranked.into_iter().map(|(_, _, _, slot)| slot).collect()
}

pub(crate) fn rank_cmp(left: (u64, u8, usize), right: (u64, u8, usize)) -> Ordering {
    left.0
        .cmp(&right.0)
        .then(left.1.cmp(&right.1))
        .then(left.2.cmp(&right.2))
}

/// One enrich reply applied to the row.
pub(crate) enum Progress {
    /// Cumulative thread counts through this activities page.
    Activities { author: u64, reviewer: u64 },
    /// Open blocker comments through this reply.
    Tasks { open: u64 },
    /// Build status for the from-ref commit.
    Build(Build),
    /// Merge flags.
    Merge { conflicted: bool, can_merge: bool },
}

/// Paths and slugs for one enriched pull request.
pub(crate) struct Query<'a> {
    /// Project key.
    pub project: &'a str,
    /// Repository slug.
    pub repo: &'a str,
    /// Pull request id.
    pub number: u64,
    /// Author slug.
    pub author_slug: &'a str,
    /// Current user slug.
    pub user_slug: &'a str,
    /// `fromRef.latestCommit` when the page has one.
    pub from_commit: Option<&'a str>,
}

/// Sequential GETs. `get` receives the path under `base_url`.
/// `on` runs after each applied reply, before the next request.
pub(crate) fn fetch(
    get: &mut dyn FnMut(&str) -> Result<host::Response, InboxFault>,
    query: &Query<'_>,
    on: &mut dyn FnMut(Progress) -> Result<(), InboxFault>,
) -> Result<Filled, InboxFault> {
    let base = format!(
        "/rest/api/1.0/projects/{}/repos/{}/pull-requests/{}",
        encode_segment(query.project),
        encode_segment(query.repo),
        query.number,
    );
    let (unanswered_as_author, unanswered_as_reviewer) =
        activity_counts(get, &base, query.author_slug, query.user_slug, on)?;
    let open_tasks = open_tasks(get, &base, on)?;
    let build = match query.from_commit {
        None => Build::None,
        Some(commit) => {
            let build = fetch_build(get, commit)?;
            on(Progress::Build(build))?;
            build
        }
    };
    let (conflicted, can_merge) = merge_flags(get, &base)?;
    on(Progress::Merge {
        conflicted,
        can_merge,
    })?;
    Ok(Filled {
        unanswered_as_author,
        unanswered_as_reviewer,
        open_tasks,
        build,
        conflicted,
        can_merge,
    })
}

pub(crate) fn write(row: &mut Row, filled: Filled) {
    row.unanswered_as_author = filled.unanswered_as_author;
    row.unanswered_as_reviewer = filled.unanswered_as_reviewer;
    row.open_tasks = filled.open_tasks;
    row.build = filled.build;
    row.conflicted = filled.conflicted;
    row.can_merge = filled.can_merge;
}

fn activity_counts(
    get: &mut dyn FnMut(&str) -> Result<host::Response, InboxFault>,
    base: &str,
    author_slug: &str,
    user_slug: &str,
    on: &mut dyn FnMut(Progress) -> Result<(), InboxFault>,
) -> Result<(u64, u64), InboxFault> {
    let mut start = 0u64;
    let mut author_n = 0u64;
    let mut reviewer_n = 0u64;
    loop {
        let path = format!("{base}/activities?start={start}&limit=25");
        let response = require_ok(get, &path)?;
        let value = parse_body(&response.body)?;
        let (page_author, page_reviewer) =
            count_activities(&value, author_slug, user_slug).map_err(InboxFault::from)?;
        author_n += page_author;
        reviewer_n += page_reviewer;
        on(Progress::Activities {
            author: author_n,
            reviewer: reviewer_n,
        })?;
        match page_end(&value).map_err(InboxFault::from)? {
            End::Last => break,
            End::More(next) => {
                if next <= start {
                    return Err(shape("nextPageStart did not advance").into());
                }
                start = next;
            }
        }
    }
    Ok((author_n, reviewer_n))
}

fn open_tasks(
    get: &mut dyn FnMut(&str) -> Result<host::Response, InboxFault>,
    base: &str,
    on: &mut dyn FnMut(Progress) -> Result<(), InboxFault>,
) -> Result<u64, InboxFault> {
    let response = get(&format!("{base}/blocker-comments?state=OPEN&count=true"))?;
    if response.status == 400 {
        return sum_tasks(get, base, None, on);
    }
    if response.status != 200 {
        return Err(http_fault(response.status, response.retry_after));
    }
    let value = parse_body(&response.body)?;
    if value.get("count").is_some() {
        let open = required_u64(&value, "count").map_err(InboxFault::from)?;
        on(Progress::Tasks { open })?;
        return Ok(open);
    }
    sum_tasks(get, base, Some(value), on)
}

fn sum_tasks(
    get: &mut dyn FnMut(&str) -> Result<host::Response, InboxFault>,
    base: &str,
    first: Option<Value>,
    on: &mut dyn FnMut(Progress) -> Result<(), InboxFault>,
) -> Result<u64, InboxFault> {
    let mut start = 0u64;
    let mut total = 0u64;
    let mut held = first;
    loop {
        let value = if let Some(value) = held.take() {
            value
        } else {
            let path = format!("{base}/blocker-comments?state=OPEN&start={start}&limit=25");
            let response = require_ok(get, &path)?;
            parse_body(&response.body)?
        };
        total += values_len(&value).map_err(InboxFault::from)?;
        on(Progress::Tasks { open: total })?;
        match page_end(&value).map_err(InboxFault::from)? {
            End::Last => break,
            End::More(next) => {
                if next <= start {
                    return Err(shape("nextPageStart did not advance").into());
                }
                start = next;
            }
        }
    }
    Ok(total)
}

fn fetch_build(
    get: &mut dyn FnMut(&str) -> Result<host::Response, InboxFault>,
    commit: &str,
) -> Result<Build, InboxFault> {
    let path = format!("/rest/build-status/1.0/commits/{}", encode_segment(commit));
    let response = get(&path)?;
    if response.status == 404 {
        Ok(Build::None)
    } else if response.status != 200 {
        Err(http_fault(response.status, response.retry_after))
    } else {
        overall(&response.body).map_err(InboxFault::from)
    }
}

fn merge_flags(
    get: &mut dyn FnMut(&str) -> Result<host::Response, InboxFault>,
    base: &str,
) -> Result<(bool, bool), InboxFault> {
    let response = require_ok(get, &format!("{base}/merge"))?;
    let value = parse_body(&response.body)?;
    Ok((
        optional_bool(&value, "conflicted").map_err(InboxFault::from)?,
        optional_bool(&value, "canMerge").map_err(InboxFault::from)?,
    ))
}

fn require_ok(
    get: &mut dyn FnMut(&str) -> Result<host::Response, InboxFault>,
    path: &str,
) -> Result<host::Response, InboxFault> {
    let response = get(path)?;
    if response.status == 200 {
        Ok(response)
    } else {
        Err(http_fault(response.status, response.retry_after))
    }
}

fn count_activities(
    value: &Value,
    author_slug: &str,
    user_slug: &str,
) -> Result<(u64, u64), Error> {
    let mut author_n = 0u64;
    let mut reviewer_n = 0u64;
    for item in array(value, "values")? {
        let Some(comment) = item.get("comment") else {
            continue;
        };
        if resolved(comment) {
            continue;
        }
        let notes = messages(comment);
        if notes.is_empty() {
            continue;
        }
        let latest = latest_note(&notes);
        if latest.slug.eq_ignore_ascii_case(user_slug) {
            continue;
        }
        if !latest.slug.eq_ignore_ascii_case(author_slug) {
            author_n += 1;
        }
        if root_unanswered(&notes, author_slug, user_slug) {
            reviewer_n += 1;
        }
    }
    Ok((author_n, reviewer_n))
}

fn resolved(comment: &Value) -> bool {
    let flagged = comment
        .get("threadResolved")
        .and_then(|value| value.as_bool());
    let state = comment.get("state").and_then(|value| value.as_str());
    flagged == Some(true) || state == Some("RESOLVED")
}

struct Note {
    slug: String,
    at: u64,
}

fn messages(comment: &Value) -> Vec<Note> {
    let mut out = Vec::new();
    let mut stack = vec![comment];
    while let Some(node) = stack.pop() {
        push_note(&mut out, node);
        if let Some(children) = node.get("comments").and_then(|value| value.as_array()) {
            for child in children.iter().rev() {
                stack.push(child);
            }
        }
    }
    out
}

fn push_note(out: &mut Vec<Note>, node: &Value) {
    let slug = node
        .get("author")
        .and_then(|author| author.get("slug"))
        .and_then(|slug| slug.as_str());
    if let Some(slug) = slug {
        let at = node
            .get("createdDate")
            .and_then(|value| value.as_u64())
            .unwrap_or(0);
        out.push(Note {
            slug: slug.to_owned(),
            at,
        });
    }
}

fn latest_note(notes: &[Note]) -> &Note {
    let mut best = &notes[0];
    for note in &notes[1..] {
        if note.at >= best.at {
            best = note;
        }
    }
    best
}

fn root_unanswered(notes: &[Note], author_slug: &str, user_slug: &str) -> bool {
    match notes.first() {
        Some(root) if root.slug.eq_ignore_ascii_case(user_slug) => !notes
            .iter()
            .any(|note| note.at > root.at && note.slug.eq_ignore_ascii_case(author_slug)),
        _ => false,
    }
}

fn overall(body: &[u8]) -> Result<Build, Error> {
    let value = json::parse(body)?;
    let mut rank = 0u8;
    for item in array(&value, "values")? {
        let next = match build_word(item)? {
            Some("FAILED" | "FAILURE" | "ERROR") => 3,
            Some("INPROGRESS" | "IN_PROGRESS" | "RUNNING") => 2,
            Some("SUCCESSFUL" | "SUCCESS") => 1,
            Some(_) | None => 0,
        };
        if next > rank {
            rank = next;
        }
    }
    Ok(match rank {
        3 => Build::Failed,
        2 => Build::InProgress,
        1 => Build::Successful,
        _ => Build::None,
    })
}

fn build_word(item: &Value) -> Result<Option<&str>, Error> {
    let field = match item.get("state") {
        Some(value) => Some(value),
        None => item.get("status"),
    };
    match field {
        None => Ok(None),
        Some(value) => match value.as_str() {
            Some(text) => Ok(Some(text)),
            None => Err(shape("state is not a string")),
        },
    }
}

fn optional_bool(value: &Value, name: &str) -> Result<bool, Error> {
    match value.get(name) {
        None => Ok(false),
        Some(flag) => flag
            .as_bool()
            .ok_or_else(|| shape(&format!("{name} is not a boolean"))),
    }
}

fn values_len(value: &Value) -> Result<u64, Error> {
    Ok(array(value, "values")?.len() as u64)
}

enum End {
    Last,
    More(u64),
}

fn page_end(value: &Value) -> Result<End, Error> {
    match value.get("isLastPage").and_then(|flag| flag.as_bool()) {
        Some(true) => Ok(End::Last),
        Some(false) => Ok(End::More(required_u64(value, "nextPageStart")?)),
        None => Err(shape("isLastPage is not a boolean")),
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

fn array<'a>(value: &'a Value, name: &str) -> Result<&'a [Value], Error> {
    match value.get(name) {
        Some(field) => field
            .as_array()
            .ok_or_else(|| shape(&format!("{name} is not an array"))),
        None => Err(shape(&format!("missing {name}"))),
    }
}

fn parse_body(body: &[u8]) -> Result<Value, InboxFault> {
    json::parse(body)
        .map_err(Error::from)
        .map_err(InboxFault::from)
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

fn shape(message: &str) -> Error {
    Error::Json(json::Error {
        message: message.to_owned(),
        offset: 0,
        line: 1,
        column: 1,
    })
}
