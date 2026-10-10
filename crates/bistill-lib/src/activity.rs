//! Pull-request activities from a Data Center page.
//!
//! The inbox list calls [`page_events`] for each activities reply. The watermark
//! uses `id` when the server sends one, and `createdDate` when it does not.

use crate::Error;
use crate::inbox::{Event, EventKind};
use json::Value;

/// Events on one activities page, in the order the server sent them.
pub(crate) fn page_events(value: &Value, user_slug: &str) -> Result<Vec<Event>, Error> {
    let mut events = Vec::new();
    for item in array(value, "values")? {
        events.push(one_event(item, user_slug)?);
    }
    Ok(events)
}

fn one_event(item: &Value, user_slug: &str) -> Result<Event, Error> {
    if item.as_object().is_none() {
        return Err(shape("activity is not an object"));
    }
    let created_ms = optional_u64(item, "createdDate").unwrap_or(0);
    let id = optional_u64(item, "id").unwrap_or(created_ms);
    let (actor_slug, actor_name) = actor(item);
    let action = item.get("action").and_then(Value::as_str).unwrap_or("");
    let added = added_slugs(item);
    let added_user = added
        .iter()
        .any(|slug| slug.eq_ignore_ascii_case(user_slug));
    let kind = kind_of(action, added_user);
    let (text, thread) = comment_bits(item);
    Ok(Event {
        id,
        created_ms,
        actor_slug,
        actor_name,
        kind,
        text,
        thread,
        added_user,
    })
}

fn kind_of(action: &str, added_user: bool) -> EventKind {
    if added_user {
        EventKind::Added
    } else if action == "COMMENTED" {
        EventKind::Commented
    } else if action == "APPROVED" {
        EventKind::Approved
    } else if action == "UPDATED" {
        EventKind::Pushed
    } else if action == "REOPENED" {
        EventKind::Reopened
    } else {
        EventKind::Other
    }
}

fn actor(item: &Value) -> (String, String) {
    let user = item.get("user");
    let slug = user
        .and_then(|user| user.get("slug"))
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_owned();
    let name = user
        .and_then(|user| user.get("displayName"))
        .and_then(Value::as_str)
        .unwrap_or(&slug)
        .to_owned();
    (slug, name)
}

fn added_slugs(item: &Value) -> Vec<String> {
    let mut slugs = Vec::new();
    if let Some(items) = item.get("addedReviewers").and_then(Value::as_array) {
        for reviewer in items {
            if let Some(slug) = reviewer
                .get("slug")
                .or_else(|| reviewer.get("user").and_then(|user| user.get("slug")))
                .and_then(Value::as_str)
            {
                slugs.push(slug.to_owned());
            }
        }
    }
    slugs
}

fn comment_bits(item: &Value) -> (String, Vec<String>) {
    let Some(comment) = item.get("comment") else {
        return (String::new(), Vec::new());
    };
    let text = comment
        .get("text")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_owned();
    let mut thread = Vec::new();
    collect_slugs(comment, &mut thread);
    (text, thread)
}

fn collect_slugs(node: &Value, slugs: &mut Vec<String>) {
    if let Some(slug) = node
        .get("author")
        .and_then(|author| author.get("slug"))
        .and_then(Value::as_str)
    {
        if !slugs.iter().any(|kept| kept == slug) {
            slugs.push(slug.to_owned());
        }
    }
    if let Some(children) = node.get("comments").and_then(Value::as_array) {
        for child in children {
            collect_slugs(child, slugs);
        }
    }
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

fn optional_u64(value: &Value, name: &str) -> Option<u64> {
    value.get(name).and_then(Value::as_u64)
}

fn shape(message: &str) -> Error {
    Error::Json(json::Error {
        message: message.to_owned(),
        offset: 0,
        line: 1,
        column: 1,
    })
}
