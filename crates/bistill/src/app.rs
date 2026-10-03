//! The inbox process.
//!
//! A free lock starts the poller on a thread and draws [`crate::screen`]. A
//! live pid draws the snapshot file, names that pid, and writes `refresh`
//! when the user presses `r`. `q` returns. The caller drops the terminal,
//! which restores the previous screen.

use crate::lock::{self, Acquire};
use crate::screen::{self, Action, Clock, Phase, Role, Screen};
use crate::watch::{self, Board, PollPhase};
use crate::{Session, explain};
use bistill_lib::{Config, Error, Snapshot, SnapshotStatus, read_snapshot};
use std::path::Path;
use std::sync::Mutex;
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
}

/// Load config and the snapshot. A corrupt snapshot is [`Exit`] with code 5.
pub fn prepare(cwd: &Path, env: &bistill_lib::Env) -> Result<Prepared, Exit> {
    let config = match bistill_lib::load(cwd, &bistill_lib::Flags::default(), env) {
        Ok(config) => config,
        Err(err) => return Err(fail(&err)),
    };
    match read_snapshot(&config.state_dir) {
        Ok(snapshot) => Ok(Prepared { config, snapshot }),
        Err(err) => Err(fail(&err)),
    }
}

/// Draw `prepared` on `backend` until `q` or the poller stops.
///
/// The holder polls on a thread. A live lock is a viewer: this function
/// re-reads `snapshot.json` and does not poll.
pub fn drive(
    backend: &mut dyn Backend,
    prepared: Prepared,
    session: &mut (dyn Session + Send),
    open: &mut dyn FnMut(&str) -> Result<(), host::Error>,
) -> Exit {
    let Prepared { config, snapshot } = prepared;
    let board = Mutex::new(Board::new(snapshot));
    match lock::acquire(&config.state_dir) {
        Ok(Acquire::Holder(held)) => {
            std::thread::scope(|scope| {
                scope.spawn(|| {
                    let outcome = watch::poll(session, &config, false, &board);
                    let mut guard = board.lock().unwrap();
                    guard.log.push_str(&outcome.stderr);
                    guard.code = outcome.code;
                    if outcome.code != 0 {
                        guard.stop = true;
                    }
                });
                ui(backend, &board, &config, Seat::Holder, open);
            });
            let code = board.lock().unwrap().code;
            drop(held);
            finish(code, &board, &config)
        }
        Ok(Acquire::Busy { pid }) => {
            let code = ui(backend, &board, &config, Seat::Viewer { pid }, open);
            finish(code, &board, &config)
        }
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

enum Seat {
    Holder,
    Viewer { pid: u32 },
}

fn ui(
    backend: &mut dyn Backend,
    board: &Mutex<Board>,
    config: &Config,
    seat: Seat,
    open: &mut dyn FnMut(&str) -> Result<(), host::Error>,
) -> i32 {
    let mut screen = Screen::new();
    loop {
        if board.lock().unwrap().stop {
            break;
        }
        if let Seat::Viewer { .. } = seat {
            reload(board, &config.state_dir);
        }
        let area = backend.size();
        let mut buffer = Buffer::empty(area.width, area.height);
        let clock = clock();
        let now_ms = clock.now_ms;
        {
            let guard = board.lock().unwrap();
            let role = role(&seat, &guard);
            screen::draw(
                &mut screen,
                &mut buffer,
                guard.snapshot.as_ref(),
                &role,
                clock,
            );
        }
        backend.draw(&buffer);
        let action = match backend.poll(Duration::from_secs(1)) {
            Some(event) => {
                let guard = board.lock().unwrap();
                screen::handle(&mut screen, event, guard.snapshot.as_ref(), now_ms)
            }
            None => {
                std::thread::yield_now();
                Action::None
            }
        };
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
    board.lock().unwrap().code
}

fn reload(board: &Mutex<Board>, state_dir: &Path) {
    if let Ok(snapshot) = read_snapshot(state_dir) {
        board.lock().unwrap().snapshot = snapshot;
    }
}

fn role(seat: &Seat, board: &Board) -> Role {
    match seat {
        Seat::Viewer { pid } => Role::Viewer { pid: *pid },
        Seat::Holder => Role::Holder(holder_phase(board)),
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
