//! `bistill watch` polls on the calling thread.
//!
//! The holder writes `snapshot.json`, diffs it, and notifies. A live pid in
//! `poll.lock` makes this process exit 1. Failures keep the last rows, record
//! the snapshot status, and wait. The wait starts at `poll_seconds` (already
//! at least 15), doubles, and stops at 10 minutes. HTTP 429 uses `Retry-After`
//! when that delay is present. A `refresh` file is deleted within about a
//! second and the next poll is not sooner than `poll_seconds` after the last one.

use crate::lock::{self, Acquire};
use crate::{Session, explain, send_notices};
use bistill_lib::{
    Client, Config, CurlFault, Error, InboxFault, Snapshot, SnapshotStatus, diff, read_snapshot,
    write_snapshot,
};
use std::path::Path;
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

pub(crate) fn run(session: &mut dyn Session, config: &Config, verbose: bool) -> Outcome {
    let previous = match read_snapshot(&config.state_dir) {
        Ok(previous) => previous,
        Err(err) => {
            return Outcome {
                stderr: explain(&err),
                code: bistill_lib::exit_code(&err),
            };
        }
    };
    let held = match lock::acquire(&config.state_dir) {
        Ok(Acquire::Holder(held)) => held,
        Ok(Acquire::Busy { pid }) => {
            return Outcome {
                stderr: format!("Another bistill is polling (pid {pid}).\n"),
                code: 1,
            };
        }
        Err(err) => {
            return Outcome {
                stderr: explain(&Error::from(err)),
                code: 1,
            };
        }
    };
    let outcome = poll(session, config, verbose, previous);
    drop(held);
    outcome
}

fn poll(
    session: &mut dyn Session,
    config: &Config,
    verbose: bool,
    mut previous: Option<Snapshot>,
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
            match wait(session, &config.state_dir, next_ms, last_ms, floor) {
                Wait::Ready(next) => next_ms = next,
                Wait::Stop => break,
            }
            continue;
        }
        match session.poll(&client, now) {
            Ok(listed) => {
                if verbose {
                    for request in &listed.requests {
                        stderr.push_str(&crate::argv_line(request));
                        stderr.push('\n');
                    }
                }
                let changes = diff(previous.as_ref(), &listed.snapshot);
                if let Err(err) = write_snapshot(&config.state_dir, &listed.snapshot) {
                    return Outcome {
                        stderr: explain(&err),
                        code: bistill_lib::exit_code(&err),
                    };
                }
                stderr.push_str(&send_notices(session, &changes));
                previous = Some(listed.snapshot);
                last_ms = now;
                gap = Gap::Steady;
                next_ms = now.saturating_add(floor);
            }
            Err(fault) => {
                if missing_curl(&fault) {
                    stderr.push_str(&explain(&fault.error));
                    return Outcome { stderr, code: 2 };
                }
                if let Some(snapshot) = previous.as_mut() {
                    apply_status(snapshot, poll_status(&fault.error), now);
                    if let Err(err) = write_snapshot(&config.state_dir, snapshot) {
                        return Outcome {
                            stderr: explain(&err),
                            code: bistill_lib::exit_code(&err),
                        };
                    }
                }
                stderr.push_str(&explain(&fault.error));
                let delay = backoff(&mut gap, floor, retry_delay(&fault));
                last_ms = now;
                next_ms = now.saturating_add(delay);
            }
        }
        if !session.again() {
            break;
        }
    }
    Outcome { stderr, code: 0 }
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
    state_dir: &Path,
    mut target: u64,
    last_ms: u64,
    floor: u64,
) -> Wait {
    loop {
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
        if !session.again() {
            return Wait::Stop;
        }
    }
}

fn consume_refresh(state_dir: &Path) -> bool {
    std::fs::remove_file(state_dir.join("refresh")).is_ok()
}
