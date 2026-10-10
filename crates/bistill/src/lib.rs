//! The `bistill` command: `ping` and the inbox screen.
//!
//! `ping` prints the curl version, whether TLS worked, the Bitbucket version,
//! who you are, and the inbox count. `bistill` and `bistill tui` on a terminal
//! hold the lock, poll, draw, and own the tray. `--json` prints the raw bodies
//! for `ping`.

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
pub use app::{Exit, Prepared, drive, prepare};
pub use args::Tui;
pub use open_os::browser;

/// `Some` when argv is a bare command or `tui`. The error text is usage.
pub fn tui_request(args: &[std::ffi::OsString]) -> Option<Result<Tui, String>> {
    args::tui_request(args).map(|result| result.map_err(|usage| args::usage_text(&usage)))
}

use args::{Command, Ping};
use bistill_lib::{
    Client, CurlFault, Env, Error, InboxFault, Listed, Report, Row, Snapshot, exit_code,
    redact_argv,
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
    /// One poll. `cached` rows keep their events when `updatedDate` has not moved.
    /// `Retry-After` is kept on HTTP 429.
    fn poll(
        &mut self,
        client: &Client,
        now_ms: u64,
        cached: &[Row],
        publish: &mut dyn FnMut(&Snapshot) -> Result<(), Error>,
    ) -> Result<Listed, InboxFault>;
    /// Epoch milliseconds for the poll schedule.
    fn now_ms(&mut self) -> u64 {
        unix_ms(SystemTime::now())
    }
    /// Wait before the next poll. The poller calls this in slices of about a second.
    fn pause(&mut self, duration: Duration) {
        std::thread::sleep(duration);
    }
    /// `false` ends the poller. The live session keeps polling.
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

    fn poll(
        &mut self,
        client: &Client,
        now_ms: u64,
        cached: &[Row],
        publish: &mut dyn FnMut(&Snapshot) -> Result<(), Error>,
    ) -> Result<Listed, InboxFault> {
        bistill_lib::poll_list(client, &mut bistill_lib::CurlFetch, now_ms, cached, publish)
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
        Ok(Command::Tui(_)) => Rendered {
            stdout: String::new(),
            stderr: "Open a terminal to start the inbox.\n".to_owned(),
            log: None,
            code: 1,
        },
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

/// Epoch milliseconds. A clock before the epoch is 0.
pub(crate) fn unix_ms(now: SystemTime) -> u64 {
    match now.duration_since(UNIX_EPOCH) {
        Ok(duration) => duration.as_millis() as u64,
        Err(_) => 0,
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
