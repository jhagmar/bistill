//! The lock holder polls on the thread that called it.
//!
//! That process writes `snapshot.json`. When a poll fails, the last rows stay,
//! the snapshot records why, and the loop waits. The wait starts at
//! `poll_seconds` (at least 15 seconds), doubles, and stops growing at 10
//! minutes. HTTP 429 uses `Retry-After` when the response includes it. A
//! `refresh` file is removed within about a second, and the next poll still
//! waits until `poll_seconds` after the previous one.
//!
//! [`Board`] is what the screen draws while this loop runs.

use crate::{Session, explain};
use bistill_lib::{
    Client, Config, CurlFault, Error, InboxFault, Row, Snapshot, SnapshotStatus, write_snapshot,
};
use std::path::Path;
use std::sync::Mutex;
use std::time::Duration;

const CAP_MS: u64 = 10 * 60 * 1000;

enum Gap {
    First,
    Steady,
    Backoff(u64),
}

enum Wait {
    Ready(u64),
    Stop,
}

pub(crate) struct Outcome {
    pub stderr: String,
    pub code: i32,
}

/// Rows applied so far, and the phase the screen draws.
pub(crate) struct Board {
    /// Rows applied so far. Absent until the first inbox page.
    pub snapshot: Option<Snapshot>,
    /// `Fetching`, `Ready`, or a backoff after a failed poll.
    pub phase: PollPhase,
    /// The screen sets this when the user quits. The loop returns.
    pub stop: bool,
    /// Exit code once the loop has returned. `0` while it is still running.
    pub code: i32,
    /// Poll and open errors. The screen writes this after it restores the terminal.
    pub log: String,
    /// Display line for [`SnapshotStatus::Error`].
    pub note: String,
}

/// Phase stored on [`Board`].
pub(crate) enum PollPhase {
    /// A fetch is in flight. Known rows stay in `snapshot`.
    Fetching,
    /// The latest poll was applied.
    Ready,
    /// The next poll waits until `next_attempt_ms`.
    Backoff {
        /// Snapshot status written for this failure.
        status: SnapshotStatus,
        /// Earliest epoch milliseconds for the next poll.
        next_attempt_ms: u64,
    },
}

impl Board {
    pub(crate) fn new(snapshot: Option<Snapshot>) -> Self {
        Board {
            snapshot,
            phase: PollPhase::Ready,
            stop: false,
            code: 0,
            log: String::new(),
            note: String::new(),
        }
    }
}

pub(crate) fn poll(
    session: &mut dyn Session,
    config: &Config,
    verbose: bool,
    board: &Mutex<Board>,
) -> Outcome {
    let mut stderr = String::new();
    let mut gap = Gap::First;
    let floor = config.poll_seconds.get().saturating_mul(1000);
    let mut next_ms = 0u64;
    let mut last_ms = 0u64;
    let client = Client::new(crate::curl_bin::PROGRAM, config);
    loop {
        let now = session.now_ms();
        if now < next_ms {
            match wait(session, board, &config.state_dir, next_ms, last_ms, floor) {
                Wait::Ready(next) => next_ms = next,
                Wait::Stop => break,
            }
            continue;
        }
        set_phase(board, PollPhase::Fetching);
        let cached = cached_rows(&board.lock().unwrap().snapshot);
        let write_error = Mutex::new(None);
        let mut publish = |snapshot: &Snapshot| match write_snapshot(&config.state_dir, snapshot) {
            Ok(()) => {
                board.lock().unwrap().snapshot = Some(snapshot.clone());
                Ok(())
            }
            Err(err) => {
                *write_error.lock().unwrap() = Some(explain(&err));
                Err(err)
            }
        };
        match session.poll(&client, now, &cached, &mut publish) {
            Ok(listed) => {
                if verbose {
                    for request in &listed.requests {
                        stderr.push_str(&crate::argv_line(request));
                        stderr.push('\n');
                    }
                }
                {
                    let mut guard = board.lock().unwrap();
                    guard.note.clear();
                    guard.snapshot = Some(listed.snapshot);
                    guard.phase = PollPhase::Ready;
                }
                last_ms = now;
                gap = Gap::Steady;
                next_ms = now.saturating_add(floor);
            }
            Err(fault) => {
                if missing_curl(&fault) {
                    stderr.push_str(&explain(&fault.error));
                    return Outcome { stderr, code: 2 };
                }
                if let Some(message) = write_error.lock().unwrap().take() {
                    stderr.push_str(&message);
                    return Outcome { stderr, code: 1 };
                }
                let status = poll_status(&fault.error);
                let delay = backoff(&mut gap, floor, retry_delay(&fault));
                let note = explain(&fault.error);
                {
                    let mut guard = board.lock().unwrap();
                    if let Some(snapshot) = guard.snapshot.as_mut() {
                        apply_status(snapshot, status, now);
                        if let Err(err) = write_snapshot(&config.state_dir, snapshot) {
                            return Outcome {
                                stderr: explain(&err),
                                code: bistill_lib::exit_code(&err),
                            };
                        }
                    }
                    guard.note = note.trim_end().to_owned();
                    guard.phase = PollPhase::Backoff {
                        status,
                        next_attempt_ms: now.saturating_add(delay),
                    };
                }
                stderr.push_str(&note);
                last_ms = now;
                next_ms = now.saturating_add(delay);
            }
        }
        if stopped(board) || !session.again() {
            break;
        }
    }
    Outcome { stderr, code: 0 }
}

fn cached_rows(snapshot: &Option<Snapshot>) -> Vec<Row> {
    match snapshot {
        Some(snapshot) => {
            let mut rows = snapshot.needs_review.clone();
            rows.extend(snapshot.waiting.iter().cloned());
            rows
        }
        None => Vec::new(),
    }
}

fn set_phase(board: &Mutex<Board>, phase: PollPhase) {
    board.lock().unwrap().phase = phase;
}

fn stopped(board: &Mutex<Board>) -> bool {
    board.lock().unwrap().stop
}

fn missing_curl(fault: &InboxFault) -> bool {
    matches!(fault.error, Error::Curl(CurlFault::Missing { .. }))
}

fn retry_delay(fault: &InboxFault) -> Option<u64> {
    if matches!(fault.error, Error::Http(429)) {
        fault.retry_after_ms
    } else {
        None
    }
}

pub(crate) fn poll_status(err: &Error) -> SnapshotStatus {
    match err {
        Error::Http(401) => SnapshotStatus::Auth,
        Error::Http(429) => SnapshotStatus::RateLimited,
        Error::Tls(_) => SnapshotStatus::Tls,
        Error::Curl(CurlFault::Timeout { .. }) => SnapshotStatus::Unreachable,
        Error::Curl(CurlFault::Failed { .. }) => SnapshotStatus::Unreachable,
        Error::Io(_) => SnapshotStatus::Unreachable,
        _ => SnapshotStatus::Error,
    }
}

fn apply_status(snapshot: &mut Snapshot, status: SnapshotStatus, now_ms: u64) {
    if snapshot.status != status {
        snapshot.status = status;
        snapshot.status_since_ms = now_ms;
    }
}

fn backoff(gap: &mut Gap, floor: u64, retry_after_ms: Option<u64>) -> u64 {
    let delay = if let Some(retry) = retry_after_ms {
        retry.min(CAP_MS)
    } else {
        match *gap {
            Gap::Backoff(prev) => prev.saturating_mul(2).min(CAP_MS),
            Gap::First | Gap::Steady => floor,
        }
    };
    *gap = Gap::Backoff(delay);
    delay
}

fn wait(
    session: &mut dyn Session,
    board: &Mutex<Board>,
    state_dir: &Path,
    mut target: u64,
    last_ms: u64,
    floor: u64,
) -> Wait {
    loop {
        if stopped(board) || !session.again() {
            return Wait::Stop;
        }
        let now = session.now_ms();
        if now >= target {
            return Wait::Ready(target);
        }
        let slice = (target - now).min(1_000);
        session.pause(Duration::from_millis(slice));
        if consume_refresh(state_dir) {
            let earliest = last_ms.saturating_add(floor);
            if earliest < target {
                target = earliest;
            }
        }
    }
}

fn consume_refresh(state_dir: &Path) -> bool {
    std::fs::remove_file(state_dir.join("refresh")).is_ok()
}
