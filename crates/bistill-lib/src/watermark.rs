//! Per pull request, how far the user has read, and whether a reviewer row is ignored.
//!
//! The file is `watermarks.json` in the state directory, separate from
//! `snapshot.json`. The process that draws the screen is the one that writes it.
//! The first time a pull request is seen, its watermark jumps to the newest
//! event so the inbox already on screen does not light the tray.

use crate::Error;
use crate::inbox::{Build, Event, EventKind, Row};
use crate::list::Snapshot;
use json::Value;
use std::collections::BTreeMap;
use std::fs;
use std::path::Path;

/// One pull request's read cursor.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct Mark {
    /// Newest activity id the user has caught up to.
    pub activity_id: u64,
    /// Left out of the tray count. Needs review only.
    pub ignored: bool,
    /// Set once, so a later poll can tell new events from the original inbox.
    pub primed: bool,
    /// Build was failed at the last read. A later failure can light the tray.
    pub seen_failed: bool,
    /// The pull request was conflicted at the last read.
    pub seen_conflict: bool,
}

/// The watermark file.
#[derive(Clone, Debug, Default)]
pub struct Store {
    /// Keyed by pull request id.
    pub marks: BTreeMap<String, Mark>,
}

/// Read `watermarks.json`. A missing file is an empty store.
pub fn read_store(dir: &Path) -> Result<Store, Error> {
    let path = dir.join("watermarks.json");
    match fs::read(&path) {
        Ok(bytes) => parse_store(&bytes),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(Store::default()),
        Err(err) => Err(Error::Io(err)),
    }
}

/// Write `watermarks.json`.
pub fn write_store(dir: &Path, store: &Store) -> Result<(), Error> {
    fs::create_dir_all(dir)?;
    let path = dir.join("watermarks.json");
    let bytes = json::to_vec(&store_value(store));
    fs::write(path, bytes)?;
    Ok(())
}

/// Mark every pull request that has never been primed.
pub fn prime(store: &mut Store, snapshot: &Snapshot) {
    for row in snapshot.needs_review.iter().chain(snapshot.waiting.iter()) {
        let mark = store.marks.entry(row.id.clone()).or_default();
        if mark.primed {
            if row.build != Build::Failed {
                mark.seen_failed = false;
            }
            if !row.conflicted {
                mark.seen_conflict = false;
            }
        } else {
            mark.activity_id = newest(row);
            mark.primed = true;
            mark.seen_failed = row.build == Build::Failed;
            mark.seen_conflict = row.conflicted;
        }
    }
}

/// Move the watermark to the newest event and record the current build and merge.
pub fn mark_read(store: &mut Store, row: &Row) {
    let mark = store.marks.entry(row.id.clone()).or_default();
    mark.activity_id = newest(row);
    mark.primed = true;
    mark.seen_failed = row.build == Build::Failed;
    mark.seen_conflict = row.conflicted;
}

/// Toggle ignored on a Needs review row. Waiting rows stay as they are.
pub fn toggle_ignore(store: &mut Store, row: &Row, needs_review: bool) -> bool {
    if needs_review {
        let mark = store.marks.entry(row.id.clone()).or_default();
        mark.ignored = !mark.ignored;
        mark.primed = true;
        true
    } else {
        false
    }
}

/// Pull requests that should light the tray.
pub fn unread_count(store: &Store, snapshot: &Snapshot) -> u64 {
    let mut count = 0u64;
    for row in &snapshot.needs_review {
        if row_unread(store, row, false, &snapshot.user_slug) {
            count += 1;
        }
    }
    for row in &snapshot.waiting {
        if row_unread(store, row, true, &snapshot.user_slug) {
            count += 1;
        }
    }
    count
}

/// The activity id this pull request is caught up to. Missing means not primed.
pub fn caught_up(store: &Store, id: &str) -> Option<u64> {
    store
        .marks
        .get(id)
        .filter(|mark| mark.primed)
        .map(|mark| mark.activity_id)
}

/// Whether the row is ignored.
pub fn is_ignored(store: &Store, id: &str) -> bool {
    store.marks.get(id).is_some_and(|mark| mark.ignored)
}

fn row_unread(store: &Store, row: &Row, author_tab: bool, user_slug: &str) -> bool {
    let Some(mark) = store.marks.get(&row.id) else {
        return false;
    };
    if !mark.primed || (mark.ignored && !author_tab) {
        return false;
    }
    let event_hit = row
        .events
        .iter()
        .any(|event| event.id > mark.activity_id && qualifies(event, author_tab, user_slug));
    let build_hit = author_tab && row.build == Build::Failed && !mark.seen_failed;
    let conflict_hit = author_tab && row.conflicted && !mark.seen_conflict;
    event_hit || build_hit || conflict_hit
}

fn qualifies(event: &Event, author_tab: bool, user_slug: &str) -> bool {
    if event.actor_slug.eq_ignore_ascii_case(user_slug) {
        return false;
    }
    if author_tab {
        matches!(event.kind, EventKind::Commented | EventKind::Approved)
    } else {
        match event.kind {
            EventKind::Added => event.added_user,
            EventKind::Pushed | EventKind::Reopened => true,
            EventKind::Commented => mentions(event, user_slug) || in_thread(event, user_slug),
            EventKind::Approved | EventKind::Other => false,
        }
    }
}

fn mentions(event: &Event, user_slug: &str) -> bool {
    let needle = format!("@{user_slug}");
    event
        .text
        .to_ascii_lowercase()
        .contains(&needle.to_ascii_lowercase())
}

fn in_thread(event: &Event, user_slug: &str) -> bool {
    event
        .thread
        .iter()
        .any(|slug| slug.eq_ignore_ascii_case(user_slug))
}

fn newest(row: &Row) -> u64 {
    row.events.iter().map(|event| event.id).max().unwrap_or(0)
}

fn parse_store(bytes: &[u8]) -> Result<Store, Error> {
    let value = json::parse(bytes)?;
    if value.as_object().is_none() {
        return Err(shape("watermarks are not an object"));
    }
    let mut marks = BTreeMap::new();
    for item in array(&value, "items")? {
        let id = req_string(item, "id")?;
        marks.insert(
            id,
            Mark {
                activity_id: req_u64(item, "activity_id")?,
                ignored: req_bool(item, "ignored")?,
                primed: req_bool(item, "primed")?,
                seen_failed: req_bool(item, "seen_failed")?,
                seen_conflict: req_bool(item, "seen_conflict")?,
            },
        );
    }
    Ok(Store { marks })
}

fn store_value(store: &Store) -> Value {
    let items = store
        .marks
        .iter()
        .map(|(id, mark)| {
            object(vec![
                ("id", string(id)),
                ("activity_id", number(mark.activity_id)),
                ("ignored", boolean(mark.ignored)),
                ("primed", boolean(mark.primed)),
                ("seen_failed", boolean(mark.seen_failed)),
                ("seen_conflict", boolean(mark.seen_conflict)),
            ])
        })
        .collect();
    object(vec![("items", Value::Array(items))])
}

fn array<'a>(value: &'a Value, name: &str) -> Result<&'a [Value], Error> {
    match value.get(name) {
        Some(field) => match field.as_array() {
            Some(items) => Ok(items),
            None => Err(shape(&format!("{name} is not an array"))),
        },
        None => Err(shape(&format!("missing {name}"))),
    }
}

fn req_string(value: &Value, name: &str) -> Result<String, Error> {
    match value.get(name).and_then(Value::as_str) {
        Some(text) => Ok(text.to_owned()),
        None => Err(shape(&format!("missing {name}"))),
    }
}

fn req_u64(value: &Value, name: &str) -> Result<u64, Error> {
    match value.get(name).and_then(Value::as_u64) {
        Some(number) => Ok(number),
        None => Err(shape(&format!("missing {name}"))),
    }
}

fn req_bool(value: &Value, name: &str) -> Result<bool, Error> {
    match value.get(name).and_then(Value::as_bool) {
        Some(flag) => Ok(flag),
        None => Err(shape(&format!("missing {name}"))),
    }
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

fn shape(message: &str) -> Error {
    Error::Json(json::Error {
        message: message.to_owned(),
        offset: 0,
        line: 1,
        column: 1,
    })
}
