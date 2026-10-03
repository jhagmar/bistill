//! Run a named program on this machine, HTTP GET through `curl`, an OS toast, and open a URL.
//!
//! The child is the program named by the caller, started from an argument vector.
//! There is no shell. This crate does not print request headers. The argument
//! vector passed to `curl` does not include `--insecure`. `curl` still honors
//! `http_proxy`, `https_proxy`, and `no_proxy`.

#![deny(unsafe_code)]

use std::ffi::OsString;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

/// Why a program or a GET failed.
#[derive(Debug)]
pub enum Error {
    /// `program` is not on `PATH`.
    Missing { program: String },
    /// The process exceeded its timeout, or `curl` reported one.
    Timeout { program: String },
    /// The process did not start, exited by signal, or finished without a usable result.
    Failed { program: String, message: String },
    /// TLS verification or the TLS handshake failed.
    Tls { message: String },
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Error::Missing { program } => write!(f, "{program} is not on PATH"),
            Error::Timeout { program } => write!(f, "{program} timed out"),
            Error::Failed { program, message } => write!(f, "{program} failed: {message}"),
            Error::Tls { message } => write!(f, "TLS failed: {message}"),
        }
    }
}

impl std::error::Error for Error {}

/// Exit status and captured output of a finished process.
#[derive(Debug)]
pub struct Output {
    /// Process exit code.
    pub code: i32,
    /// Bytes written to stdout.
    pub stdout: Vec<u8>,
    /// Bytes written to stderr.
    pub stderr: Vec<u8>,
}

/// One HTTP GET. `headers` are `Name: value` strings, passed to `curl` as `-H`.
///
/// `program` is `curl` or `curl.exe`. `user_agent` is the `-A` value.
#[derive(Clone)]
pub struct Request {
    /// Program on `PATH`.
    pub program: String,
    /// Request URL.
    pub url: String,
    /// Header lines, each `Name: value`.
    pub headers: Vec<String>,
    /// `User-Agent` value.
    pub user_agent: String,
    /// Limit for `curl --max-time`. The process is killed one second later.
    pub timeout: Duration,
    /// Optional `--cacert` path.
    pub ca_file: Option<PathBuf>,
    /// Pass `--fail-with-body`. The HTTP status is still returned.
    pub fail_with_body: bool,
}

impl std::fmt::Debug for Request {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let headers: Vec<String> = self.headers.iter().map(|header| redact(header)).collect();
        f.debug_struct("Request")
            .field("program", &self.program)
            .field("url", &self.url)
            .field("headers", &headers)
            .field("user_agent", &self.user_agent)
            .field("timeout", &self.timeout)
            .field("ca_file", &self.ca_file)
            .field("fail_with_body", &self.fail_with_body)
            .finish()
    }
}

/// HTTP status and body. 4xx and 5xx are a completed GET.
#[derive(Debug, Eq, PartialEq)]
pub struct Response {
    /// HTTP status code.
    pub status: u16,
    /// Response body.
    pub body: Vec<u8>,
    /// `Retry-After` in seconds, when the header is a delay or an HTTP-date.
    pub retry_after: Option<u64>,
}

/// Run `program` with `args` and stop it after `timeout`.
///
/// A missing program, a timeout, and a failed spawn are distinct errors.
pub fn run(program: &str, args: &[OsString], timeout: Duration) -> Result<Output, Error> {
    let mut command = Command::new(program);
    command
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut child = match command.spawn() {
        Ok(child) => child,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
            return Err(Error::Missing {
                program: program.to_owned(),
            });
        }
        Err(err) => {
            return Err(Error::Failed {
                program: program.to_owned(),
                message: err.to_string(),
            });
        }
    };
    let mut stdout = child.stdout.take().expect("piped stdout");
    let mut stderr = child.stderr.take().expect("piped stderr");
    let stdout_thread = thread::spawn(move || read_to_end(&mut stdout));
    let stderr_thread = thread::spawn(move || read_to_end(&mut stderr));
    let started = Instant::now();
    let status = loop {
        match child.try_wait().expect("wait for child") {
            Some(status) => break status,
            None if started.elapsed() >= timeout => {
                let _ = child.kill();
                let _ = child.wait();
                let _ = stdout_thread.join();
                let _ = stderr_thread.join();
                return Err(Error::Timeout {
                    program: program.to_owned(),
                });
            }
            None => thread::sleep(Duration::from_millis(10)),
        }
    };
    let stdout = stdout_thread.join().expect("stdout thread");
    let stderr = stderr_thread.join().expect("stderr thread");
    let Some(code) = status.code() else {
        return Err(Error::Failed {
            program: program.to_owned(),
            message: "exited by signal".to_owned(),
        });
    };
    Ok(Output {
        code,
        stdout,
        stderr,
    })
}

/// Argument vector for [`get`]. This is the vector `curl` receives.
///
/// ```
/// use std::time::Duration;
/// let request = host::Request {
///     program: "curl".to_owned(),
///     url: "https://git.example.invalid/rest".to_owned(),
///     headers: vec!["Accept: application/json".to_owned()],
///     user_agent: "bistill/0.1.0 (internal)".to_owned(),
///     timeout: Duration::from_secs(15),
///     ca_file: None,
///     fail_with_body: false,
/// };
/// let args = host::arguments(&request);
/// assert!(args.iter().any(|arg| arg == "-sS"));
/// assert!(args.iter().any(|arg| arg == "15"));
/// assert!(!args.iter().any(|arg| arg == "--insecure"));
/// ```
pub fn arguments(request: &Request) -> Vec<OsString> {
    let mut args = vec![
        OsString::from("-sS"),
        OsString::from("--max-time"),
        OsString::from(max_time(request.timeout)),
        OsString::from("-A"),
        OsString::from(&request.user_agent),
    ];
    for header in &request.headers {
        args.push(OsString::from("-H"));
        args.push(OsString::from(header));
    }
    if let Some(path) = &request.ca_file {
        args.push(OsString::from("--cacert"));
        args.push(path.as_os_str().to_owned());
    }
    if request.fail_with_body {
        args.push(OsString::from("--fail-with-body"));
    }
    args.push(OsString::from("-w"));
    args.push(OsString::from("\n%{http_code}"));
    args.push(OsString::from(&request.url));
    args
}

/// GET `request.url` through `curl`.
///
/// Response headers are written beside the body so `Retry-After` can be read.
pub fn get(request: &Request) -> Result<Response, Error> {
    let header_path = header_path();
    let args = with_header_dump(arguments(request), &header_path);
    let kill_after = request.timeout.saturating_add(Duration::from_secs(1));
    let output = run(&request.program, &args, kill_after)?;
    let mut response = interpret(&request.program, &output)?;
    response.retry_after = retry_after_file(&header_path, unix_secs());
    let _ = std::fs::remove_file(&header_path);
    Ok(response)
}

fn header_path() -> PathBuf {
    static NEXT: AtomicU64 = AtomicU64::new(0);
    let n = NEXT.fetch_add(1, Ordering::Relaxed);
    std::env::temp_dir().join(format!("bistill-headers-{}-{n}", std::process::id()))
}

fn with_header_dump(mut args: Vec<OsString>, header_path: &Path) -> Vec<OsString> {
    let at = args.len().saturating_sub(1);
    args.insert(at, OsString::from("-D"));
    args.insert(at + 1, header_path.as_os_str().to_owned());
    args
}

fn retry_after_file(path: &Path, now_unix: u64) -> Option<u64> {
    match std::fs::read_to_string(path) {
        Ok(text) => retry_after_value(&text, now_unix),
        Err(_) => None,
    }
}

pub(crate) fn retry_after_value(headers: &str, now_unix: u64) -> Option<u64> {
    let value = header_value(headers, "retry-after")?;
    if let Ok(seconds) = value.parse::<u64>() {
        Some(seconds)
    } else {
        http_date(value).map(|instant| instant.saturating_sub(now_unix))
    }
}

fn header_value<'a>(headers: &'a str, name: &str) -> Option<&'a str> {
    headers.lines().find_map(|line| {
        let (key, rest) = line.split_once(':')?;
        if key.eq_ignore_ascii_case(name) {
            Some(rest.trim())
        } else {
            None
        }
    })
}

fn http_date(text: &str) -> Option<u64> {
    let mut parts = text.split_whitespace();
    let _weekday = parts.next()?;
    let day: u64 = parts.next()?.trim_end_matches(',').parse().ok()?;
    let month = month_index(parts.next()?)?;
    let year: i64 = parts.next()?.parse().ok()?;
    let time = parts.next()?;
    let mut clock = time.split(':');
    let hour: u64 = clock.next()?.parse().ok()?;
    let minute: u64 = clock.next()?.parse().ok()?;
    let second: u64 = clock.next()?.parse().ok()?;
    if parts.next() == Some("GMT") {
        unix_from(year, month, day, hour, minute, second)
    } else {
        None
    }
}

fn month_index(name: &str) -> Option<u64> {
    const MONTHS: [&str; 12] = [
        "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
    ];
    MONTHS
        .iter()
        .position(|month| month.eq_ignore_ascii_case(name))
        .map(|index| index as u64 + 1)
}

fn unix_from(year: i64, month: u64, day: u64, hour: u64, minute: u64, second: u64) -> Option<u64> {
    if hour > 23 || minute > 59 || second > 59 {
        return None;
    }
    let days = civil_days(year, month, day)?;
    let tod = hour * 3600 + minute * 60 + second;
    Some(days.saturating_mul(86_400).saturating_add(tod))
}

fn civil_days(year: i64, month: u64, day: u64) -> Option<u64> {
    if !(1..=12).contains(&month) || day == 0 || day > 31 {
        return None;
    }
    let shifted = if month <= 2 { year - 1 } else { year };
    let era = shifted.div_euclid(400);
    let yoe = (shifted - era * 400) as u64;
    let mp = if month > 2 { month - 3 } else { month + 9 };
    let doy = (153 * mp + 2) / 5 + day - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    let days = era * 146_097 + doe as i64 - 719_468;
    u64::try_from(days).ok()
}

fn unix_secs() -> u64 {
    secs_at(SystemTime::now())
}

pub(crate) fn secs_at(now: SystemTime) -> u64 {
    match now.duration_since(UNIX_EPOCH) {
        Ok(duration) => duration.as_secs(),
        Err(_) => 0,
    }
}

pub(crate) fn interpret(program: &str, output: &Output) -> Result<Response, Error> {
    if output.code == 28 {
        return Err(Error::Timeout {
            program: program.to_owned(),
        });
    }
    if is_tls(output.code) {
        return Err(Error::Tls {
            message: detail(program, output.code, &output.stderr),
        });
    }
    if let Some((status, body)) = split_status(&output.stdout) {
        return Ok(Response {
            status,
            body,
            retry_after: None,
        });
    }
    Err(Error::Failed {
        program: program.to_owned(),
        message: detail(program, output.code, &output.stderr),
    })
}

/// One OS toast. Each variant carries the fields that program accepts.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Toast {
    /// `notify-send`. `--action` implies `--wait`, so this variant has no click URL.
    NotifySend {
        /// Program on `PATH`.
        program: String,
        /// Notification title.
        title: String,
        /// Notification body.
        body: String,
        /// `--expire-time` in milliseconds, when set.
        expire: Option<Duration>,
        /// Limit for the process.
        timeout: Duration,
    },
    /// PowerShell `Windows.UI.Notifications`. `url` is protocol activation on click.
    PowerShell {
        /// Program on `PATH`.
        program: String,
        /// Notification title.
        title: String,
        /// Notification body.
        body: String,
        /// Opened when the toast is clicked, when set.
        url: Option<String>,
        /// Limit for the process.
        timeout: Duration,
    },
}

/// Argument vector for [`toast`].
pub fn toast_arguments(toast: &Toast) -> Vec<OsString> {
    match toast {
        Toast::NotifySend {
            title,
            body,
            expire,
            ..
        } => notify_send_args(title, body, *expire),
        Toast::PowerShell {
            title, body, url, ..
        } => powershell_command(&powershell_show(title, body, url.as_deref())),
    }
}

/// Show `toast`. A missing program or a non-zero exit is an error.
pub fn toast(toast: &Toast) -> Result<(), Error> {
    let args = toast_arguments(toast);
    match toast {
        Toast::NotifySend {
            program, timeout, ..
        }
        | Toast::PowerShell {
            program, timeout, ..
        } => launch(program, &args, *timeout),
    }
}

/// Open a URL in the registered application.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Open {
    /// `xdg-open` with the URL as its argument.
    XdgOpen {
        /// Program on `PATH`.
        program: String,
        /// URL passed to the program.
        url: String,
        /// Limit for the process.
        timeout: Duration,
    },
    /// PowerShell `Start-Process -Verb Open`.
    WindowsStart {
        /// Program on `PATH`.
        program: String,
        /// URL passed to `Start-Process -FilePath`.
        url: String,
        /// Limit for the process.
        timeout: Duration,
    },
}

/// Argument vector for [`open_url`].
pub fn open_arguments(open: &Open) -> Vec<OsString> {
    match open {
        Open::XdgOpen { url, .. } => vec![OsString::from(url)],
        Open::WindowsStart { url, .. } => powershell_command(&windows_start(url)),
    }
}

/// Open `open`'s URL. A missing program or a non-zero exit is an error.
pub fn open_url(open: &Open) -> Result<(), Error> {
    let args = open_arguments(open);
    match open {
        Open::XdgOpen {
            program, timeout, ..
        }
        | Open::WindowsStart {
            program, timeout, ..
        } => launch(program, &args, *timeout),
    }
}

fn read_to_end(pipe: &mut impl Read) -> Vec<u8> {
    let mut buf = Vec::new();
    let _ = pipe.read_to_end(&mut buf);
    buf
}

fn max_time(timeout: Duration) -> String {
    let millis = timeout.as_millis() as u64;
    let whole = millis / 1000;
    let frac = millis % 1000;
    if frac == 0 {
        return format!("{whole}");
    }
    let mut text = format!("{whole}.{frac:03}");
    while text.ends_with('0') {
        text.pop();
    }
    text
}

fn split_status(stdout: &[u8]) -> Option<(u16, Vec<u8>)> {
    let split = stdout.iter().rposition(|byte| *byte == b'\n')?;
    let code = &stdout[split + 1..];
    if code.len() != 3 {
        return None;
    }
    let mut status: u16 = 0;
    for byte in code {
        if !byte.is_ascii_digit() {
            return None;
        }
        status = status * 10 + u16::from(*byte - b'0');
    }
    if !(100..600).contains(&status) {
        return None;
    }
    Some((status, stdout[..split].to_vec()))
}

fn exited_ok(program: &str, output: &Output) -> Result<(), Error> {
    if output.code == 0 {
        Ok(())
    } else {
        Err(Error::Failed {
            program: program.to_owned(),
            message: detail(program, output.code, &output.stderr),
        })
    }
}

fn launch(program: &str, args: &[OsString], timeout: Duration) -> Result<(), Error> {
    let output = run(program, args, timeout)?;
    exited_ok(program, &output)
}

fn notify_send_args(title: &str, body: &str, expire: Option<Duration>) -> Vec<OsString> {
    let mut args = Vec::new();
    if let Some(expire) = expire {
        args.push(OsString::from("--expire-time"));
        args.push(OsString::from(expire.as_millis().to_string()));
    }
    args.push(OsString::from("--"));
    args.push(OsString::from(title));
    args.push(OsString::from(body));
    args
}

fn powershell_command(script: &str) -> Vec<OsString> {
    vec![
        OsString::from("-NoProfile"),
        OsString::from("-NonInteractive"),
        OsString::from("-Command"),
        OsString::from(script),
    ]
}

fn powershell_show(title: &str, body: &str, url: Option<&str>) -> String {
    let xml = match url {
        Some(url) => format!(
            "<toast activationType=\"protocol\" launch=\"{}\"><visual><binding template=\"ToastGeneric\"><text>{}</text><text>{}</text></binding></visual></toast>",
            xml_escape(url),
            xml_escape(title),
            xml_escape(body)
        ),
        None => format!(
            "<toast><visual><binding template=\"ToastGeneric\"><text>{}</text><text>{}</text></binding></visual></toast>",
            xml_escape(title),
            xml_escape(body)
        ),
    };
    let mut script = String::from(
        "[Windows.UI.Notifications.ToastNotificationManager, Windows.UI.Notifications, ContentType = WindowsRuntime] | Out-Null; ",
    );
    script.push_str(
        "[Windows.Data.Xml.Dom.XmlDocument, Windows.Data.Xml.Dom.XmlDocument, ContentType = WindowsRuntime] | Out-Null; ",
    );
    script.push_str("$xml = New-Object Windows.Data.Xml.Dom.XmlDocument; $xml.LoadXml(");
    script.push_str(&ps_single(&xml));
    script.push_str("); $toast = [Windows.UI.Notifications.ToastNotification]::new($xml); ");
    script.push_str("[Windows.UI.Notifications.ToastNotificationManager]::CreateToastNotifier(");
    script.push_str(&ps_single(POWERSHELL_APP_ID));
    script.push_str(").Show($toast)");
    script
}

fn windows_start(url: &str) -> String {
    let mut script = String::from("Start-Process -FilePath ");
    script.push_str(&ps_single(url));
    script.push_str(" -Verb Open");
    script
}

fn xml_escape(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for ch in text.chars() {
        match ch {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&apos;"),
            _ => out.push(ch),
        }
    }
    out
}

fn ps_single(text: &str) -> String {
    let mut out = String::from("'");
    for ch in text.chars() {
        if ch == '\'' {
            out.push_str("''");
        } else {
            out.push(ch);
        }
    }
    out.push('\'');
    out
}

fn detail(program: &str, code: i32, stderr: &[u8]) -> String {
    let text = String::from_utf8_lossy(stderr);
    let text = text.trim();
    if text.is_empty() {
        format!("{program} exited {code}")
    } else {
        format!("{program} exited {code}: {text}")
    }
}

const POWERSHELL_APP_ID: &str =
    "{1AC14E77-02E7-4E5D-B744-2EB1AE5198B7}\\WindowsPowerShell\\v1.0\\powershell.exe";

fn is_tls(code: i32) -> bool {
    matches!(
        code,
        35 | 51 | 53 | 54 | 58 | 59 | 60 | 64 | 66 | 77 | 80 | 82 | 83 | 90 | 91
    )
}

fn redact(header: &str) -> String {
    let Some((name, _)) = header.split_once(':') else {
        return header.to_owned();
    };
    if name.trim().eq_ignore_ascii_case("authorization") {
        format!("{}: ***", name.trim())
    } else {
        header.to_owned()
    }
}

#[cfg(test)]
mod tests;
