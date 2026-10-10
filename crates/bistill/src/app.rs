//! The inbox while it is on screen.
//!
//! If the lock is free, this starts the poller on a thread and draws
//! [`crate::screen`]. A live lock means another bistill is already running,
//! and this call returns exit 1 with that pid. `q` returns. The caller then
//! drops the terminal, which puts the previous screen back.

use crate::lock::{self, Acquire};
use crate::screen::{self, Action, Clock, Pending, Phase, Role, Screen};
use crate::watch::{self, Board, PollPhase};
use crate::{Session, explain};
use bistill_lib::{
    Config, Error, Row, Snapshot, SnapshotStatus, caught_up, is_ignored, mark_read, prime,
    read_snapshot, read_store, toggle_ignore, unread_count, write_store,
};
use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Mutex, mpsc};
use std::time::{Duration, SystemTime};
use tui::{Backend, Buffer};

/// Why `prepare` or `drive` stopped.
pub struct Exit {
    /// Process status.
    pub code: i32,
    /// Lines to print after the terminal is restored.
    pub stderr: String,
}

/// Config and the snapshot on disk, ready to draw.
pub struct Prepared {
    config: Config,
    snapshot: Option<Snapshot>,
    verbose: bool,
}

/// Load config and the snapshot. A corrupt snapshot is [`Exit`] with code 5.
pub fn prepare(
    cwd: &Path,
    flags: &bistill_lib::Flags,
    env: &bistill_lib::Env,
    verbose: bool,
) -> Result<Prepared, Exit> {
    let config = match bistill_lib::load(cwd, flags, env) {
        Ok(config) => config,
        Err(err) => return Err(fail(&err)),
    };
    match read_snapshot(&config.state_dir) {
        Ok(snapshot) => Ok(Prepared {
            config,
            snapshot,
            verbose,
        }),
        Err(err) => Err(fail(&err)),
    }
}

/// Draw `prepared` on `backend` until `q` or the poller stops.
///
/// The holder polls on a thread. A live lock exits 1 and names that pid.
pub fn drive(
    backend: &mut (dyn Backend + Send),
    prepared: Prepared,
    session: &mut (dyn Session + Send),
    open: &mut dyn FnMut(&str) -> Result<(), host::Error>,
) -> Exit {
    let Prepared {
        config,
        snapshot,
        verbose,
    } = prepared;
    let board = Mutex::new(Board::new(snapshot));
    match lock::acquire(&config.state_dir) {
        Ok(Acquire::Holder(held)) => {
            std::thread::scope(|scope| {
                scope.spawn(|| {
                    let outcome = watch::poll(session, &config, verbose, &board);
                    let mut guard = board.lock().unwrap();
                    guard.log.push_str(&outcome.stderr);
                    guard.code = outcome.code;
                    if outcome.code != 0 {
                        guard.stop = true;
                    }
                });
                ui(backend, &board, &config, open);
            });
            let code = board.lock().unwrap().code;
            drop(held);
            finish(code, &board, &config)
        }
        Ok(Acquire::Busy { pid }) => Exit {
            stderr: format!("Another bistill is polling (pid {pid}).\n"),
            code: 1,
        },
        Err(err) => fail(&Error::from(err)),
    }
}

fn finish(code: i32, board: &Mutex<Board>, config: &Config) -> Exit {
    let stderr = board.lock().unwrap().log.clone();
    if let Some(path) = &config.log_file {
        if let Err(err) = append(path, &stderr) {
            return fail(&Error::from(err));
        }
    }
    Exit { code, stderr }
}

fn fail(err: &Error) -> Exit {
    Exit {
        code: bistill_lib::exit_code(err),
        stderr: explain(err),
    }
}

fn ui(
    backend: &mut (dyn Backend + Send),
    board: &Mutex<Board>,
    config: &Config,
    open: &mut dyn FnMut(&str) -> Result<(), host::Error>,
) -> i32 {
    let mut screen = Screen::new();
    let mut tray_logged = false;
    let mut seen_fetch = None;
    let shared = Mutex::new(&mut *backend);
    let (tx, rx) = mpsc::channel();
    let running = AtomicBool::new(true);
    std::thread::scope(|scope| {
        scope.spawn(|| {
            while running.load(Ordering::Relaxed) {
                let event = {
                    let mut guard = shared.lock().unwrap();
                    guard.poll(Duration::from_millis(30))
                };
                let _ = tx.send(event);
                if event.is_none() {
                    std::thread::sleep(Duration::from_millis(2));
                }
            }
        });
        loop {
            if board.lock().unwrap().stop {
                break;
            }
            let clock = clock();
            let now_ms = clock.now_ms;
            let area = {
                let guard = shared.lock().unwrap();
                guard.size()
            };
            let mut buffer = Buffer::empty(area.width, area.height);
            {
                let guard = board.lock().unwrap();
                let role = Role::Holder(holder_phase(&guard));
                screen::draw(
                    &mut screen,
                    &mut buffer,
                    guard.snapshot.as_ref(),
                    &role,
                    clock,
                );
            }
            {
                let mut guard = shared.lock().unwrap();
                guard.draw(&buffer);
            }
            sync_tray(
                board,
                config,
                &mut screen,
                &mut seen_fetch,
                &mut tray_logged,
            );
            let action = match rx.recv_timeout(Duration::from_millis(50)) {
                Ok(Some(event)) => {
                    let guard = board.lock().unwrap();
                    screen::handle(&mut screen, event, guard.snapshot.as_ref(), now_ms)
                }
                Ok(None) | Err(_) => Action::None,
            };
            apply_pending(&mut screen, board, config, &mut tray_logged);
            match action {
                Action::Quit => {
                    board.lock().unwrap().stop = true;
                    break;
                }
                Action::Refresh => {
                    let _ = std::fs::write(config.state_dir.join("refresh"), b"");
                }
                Action::Open(url) => {
                    if let Err(err) = open(&url) {
                        board
                            .lock()
                            .unwrap()
                            .log
                            .push_str(&explain(&Error::from(err)));
                    }
                }
                Action::None => {}
            }
        }
        running.store(false, Ordering::Relaxed);
    });
    board.lock().unwrap().code
}

fn sync_tray(
    board: &Mutex<Board>,
    config: &Config,
    screen: &mut Screen,
    seen_fetch: &mut Option<u64>,
    tray_logged: &mut bool,
) {
    let snapshot = board.lock().unwrap().snapshot.clone();
    let Some(snapshot) = snapshot else {
        return;
    };
    if *seen_fetch == Some(snapshot.fetched_ms) {
        return;
    }
    *seen_fetch = Some(snapshot.fetched_ms);
    let mut store = match read_store(&config.state_dir) {
        Ok(store) => store,
        Err(err) => {
            board.lock().unwrap().log.push_str(&explain(&err));
            return;
        }
    };
    prime(&mut store, &snapshot);
    let count = unread_count(&store, &snapshot);
    if let Err(err) = write_store(&config.state_dir, &store) {
        board.lock().unwrap().log.push_str(&explain(&err));
    }
    show_tray(count, &mut board.lock().unwrap().log, tray_logged);
    let mut floors = BTreeMap::new();
    let mut ignored = BTreeSet::new();
    for row in snapshot.needs_review.iter().chain(snapshot.waiting.iter()) {
        if let Some(id) = caught_up(&store, &row.id) {
            floors.insert(row.id.clone(), id);
        }
        if is_ignored(&store, &row.id) {
            ignored.insert(row.id.clone());
        }
    }
    screen.set_marks(floors, ignored);
}

fn apply_pending(
    screen: &mut Screen,
    board: &Mutex<Board>,
    config: &Config,
    tray_logged: &mut bool,
) {
    let Some(pending) = screen.take_pending() else {
        return;
    };
    let snapshot = board
        .lock()
        .unwrap()
        .snapshot
        .clone()
        .expect("a key names a row from the snapshot on screen");
    let mut store = match read_store(&config.state_dir) {
        Ok(store) => store,
        Err(err) => {
            board.lock().unwrap().log.push_str(&explain(&err));
            return;
        }
    };
    match pending {
        Pending::Read(id) => {
            if let Some(row) = find_row(&snapshot, &id) {
                mark_read(&mut store, row);
            }
        }
        Pending::Ignore(id) => apply_ignore(&mut store, &snapshot, &id),
    }
    let count = unread_count(&store, &snapshot);
    if let Err(err) = write_store(&config.state_dir, &store) {
        board.lock().unwrap().log.push_str(&explain(&err));
    }
    show_tray(count, &mut board.lock().unwrap().log, tray_logged);
    let mut floors = BTreeMap::new();
    let mut ignored = BTreeSet::new();
    for row in snapshot.needs_review.iter().chain(snapshot.waiting.iter()) {
        if let Some(mark) = caught_up(&store, &row.id) {
            floors.insert(row.id.clone(), mark);
        }
        if is_ignored(&store, &row.id) {
            ignored.insert(row.id.clone());
        }
    }
    screen.set_marks(floors, ignored);
}

pub(crate) fn apply_ignore(store: &mut bistill_lib::Store, snapshot: &Snapshot, id: &str) {
    let needs = snapshot.needs_review.iter().any(|item| item.id == id);
    if let Some(row) = find_row(snapshot, id) {
        toggle_ignore(store, row, needs);
    }
}

fn find_row<'a>(snapshot: &'a Snapshot, id: &str) -> Option<&'a Row> {
    snapshot
        .needs_review
        .iter()
        .chain(snapshot.waiting.iter())
        .find(|row| row.id == id)
}

fn show_tray(count: u64, log: &mut String, logged: &mut bool) {
    record_tray(count, log, logged, host::set_tray);
}

pub(crate) fn record_tray(
    count: u64,
    log: &mut String,
    logged: &mut bool,
    set: fn(u64) -> Result<(), host::Error>,
) {
    if let Err(err) = set(count) {
        if !*logged {
            *logged = true;
            log.push_str(&err.to_string());
            log.push('\n');
        }
    }
}

fn holder_phase(board: &Board) -> Phase {
    let since = board
        .snapshot
        .as_ref()
        .map(|snapshot| snapshot.status_since_ms)
        .unwrap_or(0);
    match &board.phase {
        PollPhase::Fetching => Phase::Fetching,
        PollPhase::Ready => Phase::Ready,
        PollPhase::Backoff {
            status,
            next_attempt_ms,
        } => screen_phase(*status, since, &board.note, *next_attempt_ms),
    }
}

pub(crate) fn screen_phase(
    status: SnapshotStatus,
    since_ms: u64,
    note: &str,
    _next_attempt_ms: u64,
) -> Phase {
    match status {
        SnapshotStatus::Fetching => Phase::Fetching,
        SnapshotStatus::Ok => Phase::Ready,
        SnapshotStatus::Auth => Phase::Auth,
        SnapshotStatus::Tls => Phase::Tls,
        SnapshotStatus::Unreachable => Phase::Unreachable { since_ms },
        SnapshotStatus::RateLimited => Phase::RateLimited,
        SnapshotStatus::Error => Phase::Failed {
            message: note.to_owned(),
        },
    }
}

fn clock() -> Clock {
    Clock {
        now_ms: crate::unix_ms(SystemTime::now()),
        offset_secs: crate::zone_os::local_offset_secs(),
    }
}

fn append(path: &Path, text: &str) -> std::io::Result<()> {
    use std::io::Write;
    let mut file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)?;
    file.write_all(text.as_bytes())
}
