//! `bistill ping`, `bistill ls`, `bistill watch`, and the inbox screen.
//!
//! `ping` lists the curl version, TLS, the Bitbucket version, the user, and
//! the inbox count. `ls` prints the two inbox sections and notifies when the
//! snapshot changes. `watch` holds `poll.lock` and polls on this thread.
//! With no subcommand, a terminal on stdout runs that poller on a thread and
//! draws the screen. `--json` prints the raw bodies for `ping` and the
//! snapshot for `ls`.

#![deny(unsafe_code)]

mod app;
mod args;
mod lock;
mod screen;
mod watch;

#[cfg(unix)]
#[path = "open_unix.rs"]
mod open_os;
#[cfg(windows)]
#[path = "open_windows.rs"]
mod open_os;
#[cfg(unix)]
#[path = "zone_unix.rs"]
mod zone_os;
#[cfg(windows)]
#[path = "zone_windows.rs"]
mod zone_os;

#[cfg(unix)]
#[path = "pid_unix.rs"]
mod pid_os;
#[cfg(windows)]
#[path = "pid_windows.rs"]
mod pid_os;

#[cfg(unix)]
#[path = "curl_unix.rs"]
mod curl_bin;
#[cfg(windows)]
#[path = "curl_windows.rs"]
mod curl_bin;
#[cfg(unix)]
#[path = "notify_unix.rs"]
mod notify_bin;
#[cfg(windows)]
#[path = "notify_windows.rs"]
mod notify_bin;

pub use app::{Exit, Prepared, drive, prepare};
pub use open_os::browser;

use args::{Command, Ls, Ping, Watch};
use bistill_lib::{
    Client, CurlFault, Env, Error, InboxFault, Listed, Report, Row, Snapshot, attention_count,
    exit_code, redact_argv, to_json,
};
use std::fs::OpenOptions;
use std::io::Write;
use std::path::Path;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

/// A curl version read and a ping. Tests supply a stand-in.
pub trait Session {
    /// First line of `program -V`.
    fn version(&mut self, program: &str) -> Result<String, Error>;
    /// The four ping GETs.
    fn ping(&mut self, client: &Client) -> Result<Report, Error>;
    /// Both inbox roles and the list snapshot.
    fn list(&mut self, client: &Client, now_ms: u64) -> Result<Listed, Error>;
    /// Show one OS notification.
    fn notify(&mut self, toast: &host::Toast) -> Result<(), host::Error>;
    /// One poll. `Retry-After` is kept on HTTP 429.
    fn poll(&mut self, client: &Client, now_ms: u64) -> Result<Listed, InboxFault>;
    /// Epoch milliseconds for the poll schedule.
    fn now_ms(&mut self) -> u64 {
        unix_ms(SystemTime::now())
    }
    /// Wait before the next poll. `watch` calls this in slices of about a second.
    fn pause(&mut self, duration: Duration) {
        std::thread::sleep(duration);
    }
    /// `false` ends `watch`. The live session keeps polling.
    fn again(&mut self) -> bool {
        true
    }
}

/// [`Session`] that runs `curl` on this machine.
pub struct Live;

impl Session for Live {
    fn version(&mut self, program: &str) -> Result<String, Error> {
        read_version(program)
    }

    fn ping(&mut self, client: &Client) -> Result<Report, Error> {
        bistill_lib::ping(client)
    }

    fn list(&mut self, client: &Client, now_ms: u64) -> Result<Listed, Error> {
        bistill_lib::list_inbox(client, &mut bistill_lib::CurlFetch, now_ms)
    }

    fn notify(&mut self, toast: &host::Toast) -> Result<(), host::Error> {
        host::toast(toast)
    }

    fn poll(&mut self, client: &Client, now_ms: u64) -> Result<Listed, InboxFault> {
        bistill_lib::poll_list(client, &mut bistill_lib::CurlFetch, now_ms)
    }
}

/// Parse `args`, load config, and write the ping result.
pub fn execute(
    args: &[std::ffi::OsString],
    cwd: &Path,
    env: &Env,
    stdout: &mut dyn Write,
    stderr: &mut dyn Write,
    session: &mut dyn Session,
) -> i32 {
    commit(stdout, stderr, &render(args, cwd, env, session))
}

struct Rendered {
    stdout: String,
    stderr: String,
    log: Option<std::path::PathBuf>,
    code: i32,
}

fn render(
    args: &[std::ffi::OsString],
    cwd: &Path,
    env: &Env,
    session: &mut dyn Session,
) -> Rendered {
    match args::parse(args) {
        Ok(Command::Help) => Rendered {
            stdout: args::help_text(),
            stderr: String::new(),
            log: None,
            code: 0,
        },
        Err(usage) => Rendered {
            stdout: String::new(),
            stderr: args::usage_text(&usage),
            log: None,
            code: 1,
        },
        Ok(Command::Ping(ping)) => render_ping(cwd, env, session, &ping),
        Ok(Command::Ls(ls)) => render_ls(cwd, env, session, &ls),
        Ok(Command::Watch(watch)) => render_watch(cwd, env, session, &watch),
    }
}

fn render_watch(cwd: &Path, env: &Env, session: &mut dyn Session, watch: &Watch) -> Rendered {
    let config = match bistill_lib::load(cwd, &watch.flags, env) {
        Ok(config) => config,
        Err(err) => return fail(&err, None),
    };
    let outcome = watch::run(session, &config, watch.verbose);
    Rendered {
        stdout: String::new(),
        stderr: outcome.stderr,
        log: config.log_file,
        code: outcome.code,
    }
}

fn render_ping(cwd: &Path, env: &Env, session: &mut dyn Session, ping: &Ping) -> Rendered {
    let config = match bistill_lib::load(cwd, &ping.flags, env) {
        Ok(config) => config,
        Err(err) => return fail(&err, None),
    };
    let client = Client::new(curl_bin::PROGRAM, &config);
    let mut stderr = String::new();
    if ping.verbose {
        for request in client.requests() {
            stderr.push_str(&argv_line(&request));
            stderr.push('\n');
        }
    }
    let version = match session.version(curl_bin::PROGRAM) {
        Ok(text) => text,
        Err(err) => {
            stderr.push_str(&explain(&err));
            return Rendered {
                stdout: String::new(),
                stderr,
                log: config.log_file,
                code: exit_code(&err),
            };
        }
    };
    let report = match session.ping(&client) {
        Ok(report) => report,
        Err(err) => {
            stderr.push_str(&explain(&err));
            return Rendered {
                stdout: String::new(),
                stderr,
                log: config.log_file,
                code: exit_code(&err),
            };
        }
    };
    let stdout = if ping.json {
        raw_bodies(&report)
    } else {
        plain(&version, &report)
    };
    Rendered {
        stdout,
        stderr,
        log: config.log_file,
        code: 0,
    }
}

fn render_ls(cwd: &Path, env: &Env, session: &mut dyn Session, ls: &Ls) -> Rendered {
    let config = match bistill_lib::load(cwd, &ls.flags, env) {
        Ok(config) => config,
        Err(err) => return fail(&err, None),
    };
    let previous = match bistill_lib::read_snapshot(&config.state_dir) {
        Ok(previous) => previous,
        Err(err) => return fail(&err, config.log_file),
    };
    let client = Client::new(curl_bin::PROGRAM, &config);
    let listed = match session.list(&client, unix_ms(SystemTime::now())) {
        Ok(listed) => listed,
        Err(err) => return fail(&err, config.log_file),
    };
    let changes = bistill_lib::diff(previous.as_ref(), &listed.snapshot);
    if let Err(err) = bistill_lib::write_snapshot(&config.state_dir, &listed.snapshot) {
        return fail(&err, config.log_file);
    }
    let mut stderr = String::new();
    if ls.verbose {
        for request in &listed.requests {
            stderr.push_str(&argv_line(request));
            stderr.push('\n');
        }
    }
    stderr.push_str(&send_notices(session, &changes));
    let stdout = if ls.count {
        format!("{}\n", attention_count(&listed.snapshot))
    } else if ls.json {
        json_text(&listed.snapshot)
    } else {
        human(&listed.snapshot)
    };
    Rendered {
        stdout,
        stderr,
        log: config.log_file,
        code: 0,
    }
}

/// Epoch milliseconds. A clock before the epoch is 0.
pub(crate) fn unix_ms(now: SystemTime) -> u64 {
    match now.duration_since(UNIX_EPOCH) {
        Ok(duration) => duration.as_millis() as u64,
        Err(_) => 0,
    }
}

fn human(snapshot: &Snapshot) -> String {
    if snapshot.needs_review.is_empty() && snapshot.waiting.is_empty() {
        return "Nothing needs your attention.\n".to_owned();
    }
    let mut out = String::new();
    section(&mut out, "Needs review", &snapshot.needs_review);
    section(&mut out, "Waiting on others", &snapshot.waiting);
    out
}

fn section(out: &mut String, title: &str, rows: &[Row]) {
    out.push_str(title);
    out.push('\n');
    for row in rows {
        out.push_str(&format!(
            "{}/{}#{}  {}",
            row.project, row.repo, row.number, row.title
        ));
        if row.draft {
            out.push_str("  draft");
        }
        if row.stale {
            out.push_str("  stale");
        }
        if row.needs_work {
            out.push_str("  needs work");
        }
        out.push('\n');
    }
}

fn json_text(snapshot: &Snapshot) -> String {
    let mut text = to_json(snapshot);
    text.push('\n');
    text
}

fn send_notices(session: &mut dyn Session, changes: &[bistill_lib::Change]) -> String {
    let mut logged = String::new();
    for change in changes {
        let body = bistill_lib::toast_body(change);
        let toast = notify_bin::desktop_toast(&body, &change.html_url);
        if let Err(err) = session.notify(&toast) {
            note(&mut logged, err);
        }
    }
    logged
}

fn note(logged: &mut String, err: host::Error) {
    if logged.is_empty() {
        *logged = explain(&Error::from(err));
    }
}

fn fail(err: &Error, log: Option<std::path::PathBuf>) -> Rendered {
    Rendered {
        stdout: String::new(),
        stderr: explain(err),
        log,
        code: exit_code(err),
    }
}

fn commit(stdout: &mut dyn Write, stderr: &mut dyn Write, rendered: &Rendered) -> i32 {
    if let Some(path) = &rendered.log {
        if let Err(err) = append_log(path, &rendered.stderr) {
            let _ = stderr.write_all(explain(&Error::from(err)).as_bytes());
            return 1;
        }
    }
    if let Err(err) = stdout.write_all(rendered.stdout.as_bytes()) {
        let _ = stderr.write_all(explain(&Error::from(err)).as_bytes());
        return 1;
    }
    if let Err(err) = stderr.write_all(rendered.stderr.as_bytes()) {
        let _ = stdout.write_all(explain(&Error::from(err)).as_bytes());
        return 1;
    }
    rendered.code
}

fn append_log(path: &Path, text: &str) -> std::io::Result<()> {
    let mut file = OpenOptions::new().create(true).append(true).open(path)?;
    file.write_all(text.as_bytes())
}

fn argv_line(request: &host::Request) -> String {
    let mut parts = vec![request.program.clone()];
    parts.extend(redact_argv(&host::arguments(request)));
    parts.join(" ")
}

fn plain(version: &str, report: &Report) -> String {
    format!(
        "{version}\nTLS ok.\n{} {}.\n{} ({}).\n{}\n",
        report.product.display_name,
        report.product.version,
        report.user.display_name,
        report.user.slug,
        report.inbox
    )
}

fn raw_bodies(report: &Report) -> String {
    let mut out = String::new();
    push_body(&mut out, &report.bodies.application_properties);
    push_body(&mut out, &report.bodies.user);
    push_body(&mut out, &report.bodies.inbox_count);
    out
}

fn push_body(out: &mut String, body: &[u8]) {
    out.push_str(&String::from_utf8_lossy(body));
    if !body.ends_with(b"\n") {
        out.push('\n');
    }
}

fn explain(err: &Error) -> String {
    let text = match err {
        Error::Curl(CurlFault::Missing { program }) => format!("{program} must be on PATH."),
        Error::Tls(_) => "curl failed TLS.".to_owned(),
        Error::Http(401) => "Token rejected.".to_owned(),
        other => other.to_string(),
    };
    format!("{text}\n")
}

fn read_version(program: &str) -> Result<String, Error> {
    let output = host::run(
        program,
        &[std::ffi::OsString::from("-V")],
        Duration::from_secs(15),
    )?;
    if output.code != 0 {
        return Err(Error::Curl(CurlFault::Failed {
            program: program.to_owned(),
            message: format!("exited {}", output.code),
        }));
    }
    let text = String::from_utf8_lossy(&output.stdout);
    match text
        .lines()
        .next()
        .map(str::trim)
        .filter(|line| !line.is_empty())
    {
        Some(line) => Ok(line.to_owned()),
        None => Err(Error::Curl(CurlFault::Failed {
            program: program.to_owned(),
            message: "printed no version".to_owned(),
        })),
    }
}

#[cfg(test)]
mod tests;
