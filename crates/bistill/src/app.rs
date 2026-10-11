//! The inbox while it is on screen.
//!
//! If the lock is free, this starts the poller on a thread and draws
//! [`crate::screen`]. A live lock means another bistill is already running,
//! and this call returns exit 1 with that pid. `q` returns. The caller then
//! drops the terminal, which puts the previous screen back.
//!
//! An input thread blocks in `poll` on the terminal without the draw lock,
//! then wakes the draw loop with the event. The poller sends a publish on
//! that same channel. The draw loop paints, then waits until the next event
//! or the next visible age change. A tray update runs on its own thread.

use crate::lock::{self, Acquire};
use crate::refresh_os::{self, RefreshWatch};
use crate::screen::{self, Action, Clock, Pending, Phase, Role, Screen};
use crate::watch::{self, Board, PollPhase, Wake};
use crate::{Nudge, Session, explain};
use bistill_lib::{
    Config, Error, Row, Snapshot, SnapshotStatus, caught_up, is_ignored, mark_read, prime,
    read_snapshot, read_store, toggle_ignore, unread_count, write_store,
};
use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, mpsc};
use std::time::{Duration, SystemTime};
use tui::{Backend, Buffer, Event};

/// What the draw thread should do after waiting.
pub(crate) enum Step {
    /// An input event, a publish, or a failed poll.
    Wake(Wake),
    /// A relative time on screen can change.
    Age,
    /// The wake channel is closed.
    Gone,
}

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
    let board = Arc::new(Mutex::new(Board::new(snapshot)));
    let config = Arc::new(config);
    match lock::acquire(&config.state_dir) {
        Ok(Acquire::Holder(held)) => {
            let (wake_tx, wake_rx) = mpsc::channel();
            let (nudge_tx, nudge_rx) = mpsc::channel();
            let refresh = refresh_os::open(&config.state_dir).ok();
            std::thread::scope(|scope| {
                let paint = wake_tx.clone();
                let board_poll = Arc::clone(&board);
                let config_poll = Arc::clone(&config);
                scope.spawn(move || {
                    let outcome = watch::poll(
                        session,
                        &config_poll,
                        verbose,
                        &board_poll,
                        &nudge_rx,
                        &paint,
                    );
                    let mut guard = board_poll.lock().unwrap();
                    guard.log.push_str(&outcome.stderr);
                    guard.code = outcome.code;
                    if outcome.code != 0 {
                        guard.stop = true;
                        drop(guard);
                        let _ = paint.send(Wake::Stop);
                    }
                });
                if let Some(watch) = refresh.as_ref() {
                    let nudge = nudge_tx.clone();
                    scope.spawn(move || relay_refresh(watch, &nudge));
                }
                ui(backend, &board, &config, open, wake_tx, wake_rx, &nudge_tx);
                if let Some(watch) = refresh.as_ref() {
                    watch.cancel();
                }
                let _ = nudge_tx.send(Nudge::Quit);
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
    wake_tx: mpsc::Sender<Wake>,
    wake_rx: mpsc::Receiver<Wake>,
    nudge_tx: &mpsc::Sender<Nudge>,
) -> i32 {
    let mut screen = Screen::new();
    let mut seen_fetch = None;
    let shared = Mutex::new(&mut *backend);
    let (tray_tx, tray_rx) = mpsc::channel();
    let running = AtomicBool::new(true);
    let parking = {
        let guard = shared.lock().unwrap();
        guard.parking()
    };
    std::thread::scope(|scope| {
        scope.spawn(|| {
            host::run_tray(tray_rx, &mut |message| {
                board.lock().unwrap().log.push_str(message);
            });
        });
        let input_tx = wake_tx;
        let input_parking = Arc::clone(&parking);
        let shared_thread = &shared;
        let running_thread = &running;
        scope.spawn(move || {
            let mut seen = 0u64;
            while running_thread.load(Ordering::Relaxed) {
                let event = {
                    let mut guard = shared_thread.lock().unwrap();
                    guard.poll(Duration::ZERO)
                };
                if let Some(event) = event {
                    let _ = input_tx.send(Wake::Input(event));
                } else {
                    seen = input_parking.park(seen);
                }
            }
        });
        loop {
            let clock = clock();
            paint_frame(&shared, board, &mut screen, clock);
            sync_tray(board, config, &mut screen, &mut seen_fetch, &tray_tx);
            let limit = {
                let guard = board.lock().unwrap();
                screen::next_age(guard.snapshot.as_ref(), clock.now_ms)
            };
            if take_wakes(
                &mut screen,
                board,
                config,
                open,
                &Out {
                    tray: &tray_tx,
                    nudge: nudge_tx,
                },
                &wake_rx,
                limit,
            ) {
                break;
            }
        }
        running.store(false, Ordering::Relaxed);
        parking.unpark();
        // The tray thread ends when this sender drops. The scope joins it after
        // this closure returns, so the drop has to happen first.
        drop(tray_tx);
    });
    board.lock().unwrap().code
}

pub(crate) struct Out<'a> {
    pub(crate) tray: &'a mpsc::Sender<u64>,
    pub(crate) nudge: &'a mpsc::Sender<Nudge>,
}

/// `true` when the screen should stop.
pub(crate) fn take_wakes(
    screen: &mut Screen,
    board: &Mutex<Board>,
    config: &Config,
    open: &mut dyn FnMut(&str) -> Result<(), host::Error>,
    out: &Out<'_>,
    wake_rx: &mpsc::Receiver<Wake>,
    limit: Option<Duration>,
) -> bool {
    let mut step = next_step(wake_rx, limit);
    loop {
        match step {
            Step::Gone => return true,
            Step::Age => return false,
            Step::Wake(Wake::Stop) => return true,
            Step::Wake(Wake::Republish) => {}
            Step::Wake(Wake::Input(event)) => {
                let now = clock().now_ms;
                if apply_event(screen, board, config, open, out, event, now) {
                    return true;
                }
            }
        }
        step = match wake_rx.try_recv() {
            Ok(wake) => Step::Wake(wake),
            Err(_) => return false,
        };
    }
}

/// The next reason to paint. `limit` is the time until a relative age changes.
pub(crate) fn next_step(wake: &mpsc::Receiver<Wake>, limit: Option<Duration>) -> Step {
    match limit {
        Some(limit) => match wake.recv_timeout(limit) {
            Ok(wake) => Step::Wake(wake),
            Err(mpsc::RecvTimeoutError::Timeout) => Step::Age,
            Err(mpsc::RecvTimeoutError::Disconnected) => Step::Gone,
        },
        None => match wake.recv() {
            Ok(wake) => Step::Wake(wake),
            Err(_) => Step::Gone,
        },
    }
}

fn relay_refresh(watch: &RefreshWatch, nudge: &mpsc::Sender<Nudge>) {
    while let Ok(true) = watch.wait() {
        let _ = nudge.send(Nudge::Refresh);
    }
}

fn paint_frame(
    backend: &Mutex<&mut (dyn Backend + Send)>,
    board: &Mutex<Board>,
    screen: &mut Screen,
    clock: Clock,
) {
    let area = {
        let guard = backend.lock().unwrap();
        guard.size()
    };
    let mut buffer = Buffer::empty(area.width, area.height);
    {
        let guard = board.lock().unwrap();
        let role = Role::Holder(holder_phase(&guard));
        screen::draw(screen, &mut buffer, guard.snapshot.as_ref(), &role, clock);
    }
    let mut guard = backend.lock().unwrap();
    guard.draw(&buffer);
}

fn apply_event(
    screen: &mut Screen,
    board: &Mutex<Board>,
    config: &Config,
    open: &mut dyn FnMut(&str) -> Result<(), host::Error>,
    out: &Out<'_>,
    event: Event,
    now_ms: u64,
) -> bool {
    let action = {
        let guard = board.lock().unwrap();
        screen::handle(screen, event, guard.snapshot.as_ref(), now_ms)
    };
    apply_pending(screen, board, config, out.tray);
    match action {
        Action::Quit => {
            board.lock().unwrap().stop = true;
            let _ = out.nudge.send(Nudge::Quit);
            true
        }
        Action::Refresh => {
            let _ = std::fs::write(config.state_dir.join("refresh"), b"");
            let _ = out.nudge.send(Nudge::Refresh);
            false
        }
        Action::Open(url) => {
            if let Err(err) = open(&url) {
                board
                    .lock()
                    .unwrap()
                    .log
                    .push_str(&explain(&Error::from(err)));
            }
            false
        }
        Action::None => false,
    }
}

fn sync_tray(
    board: &Mutex<Board>,
    config: &Config,
    screen: &mut Screen,
    seen_fetch: &mut Option<u64>,
    tray: &mpsc::Sender<u64>,
) {
    let fetched = board
        .lock()
        .unwrap()
        .snapshot
        .as_ref()
        .map(|snapshot| snapshot.fetched_ms);
    if fetched.is_none() {
        return;
    }
    if *seen_fetch == fetched {
        return;
    }
    let snapshot = board
        .lock()
        .unwrap()
        .snapshot
        .clone()
        .expect("fetched_ms was read from this snapshot");
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
    enqueue_tray(tray, count);
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
    tray: &mpsc::Sender<u64>,
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
    enqueue_tray(tray, count);
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

fn enqueue_tray(tray: &mpsc::Sender<u64>, count: u64) {
    let _ = tray.send(count);
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
