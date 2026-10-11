use super::*;
use std::ffi::OsString;
use std::io::{Read, Write};
use std::net::TcpListener;
use std::path::PathBuf;
use std::time::Duration;

fn sample(url: &str) -> Request {
    Request {
        program: "curl".to_owned(),
        url: url.to_owned(),
        headers: vec![
            "Accept: application/json".to_owned(),
            "Authorization: Bearer secret-token".to_owned(),
        ],
        user_agent: "bistill/0.1.0 (internal)".to_owned(),
        timeout: Duration::from_secs(15),
        ca_file: None,
        fail_with_body: false,
    }
}

fn owned(args: &[&str]) -> Vec<OsString> {
    args.iter().map(OsString::from).collect()
}

fn output(code: i32, stdout: &[u8], stderr: &str) -> Output {
    Output {
        code,
        stdout: stdout.to_vec(),
        stderr: stderr.as_bytes().to_vec(),
    }
}

#[test]
fn arguments_match_the_curl_contract() {
    let mut request = sample("https://git.example.invalid/rest");
    request.timeout = Duration::from_millis(1500);
    request.ca_file = Some(PathBuf::from("/corp/ca.pem"));
    request.fail_with_body = true;
    let args = arguments(&request);
    let text: Vec<String> = args
        .iter()
        .map(|arg| arg.to_string_lossy().into_owned())
        .collect();
    assert_eq!(
        text,
        vec![
            "-sS",
            "--max-time",
            "1.5",
            "-A",
            "bistill/0.1.0 (internal)",
            "-H",
            "Accept: application/json",
            "-H",
            "Authorization: Bearer secret-token",
            "--cacert",
            "/corp/ca.pem",
            "--fail-with-body",
            "-w",
            "\n%{http_code}",
            "https://git.example.invalid/rest",
        ]
    );
    assert!(!text.iter().any(|arg| arg == "--insecure"));
    assert_eq!(max_time(Duration::from_secs(15)), "15");
    assert_eq!(max_time(Duration::from_millis(200)), "0.2");
    assert_eq!(max_time(Duration::from_millis(1001)), "1.001");
    let debug = format!("{request:?}");
    assert!(!debug.contains("secret-token"));
    assert!(debug.contains("Authorization: ***"));
    assert_eq!(
        redact("Accept: application/json"),
        "Accept: application/json"
    );
    assert_eq!(redact("no-colon"), "no-colon");
}

#[test]
fn classifies_curl_results() {
    let ok = interpret("curl", &output(0, b"hi\n200", "")).unwrap();
    assert_eq!(ok.status, 200);
    assert_eq!(ok.body, b"hi");
    let kept = interpret("curl", &output(22, b"nope\n404", "not found")).unwrap();
    assert_eq!(
        kept,
        Response {
            status: 404,
            body: b"nope".to_vec(),
            retry_after: None,
        }
    );
    let empty = interpret("curl", &output(0, b"\n204", "")).unwrap();
    assert_eq!(empty.body, b"");
    assert!(interpret("curl", &output(0, b"\n000", "")).is_err());
    assert!(interpret("curl", &output(0, b"\n20", "")).is_err());
    assert!(interpret("curl", &output(0, b"\n20x", "")).is_err());
    assert!(interpret("curl", &output(0, b"200", "")).is_err());
    assert!(matches!(
        interpret("curl", &output(28, b"", "timed out")),
        Err(Error::Timeout { .. })
    ));
    assert!(matches!(
        interpret("curl", &output(60, b"", "cert")),
        Err(Error::Tls { .. })
    ));
    assert!(matches!(
        interpret("curl", &output(7, b"", "")),
        Err(Error::Failed { .. })
    ));
    assert!(is_tls(35));
    assert!(!is_tls(7));
    let tls = interpret("curl", &output(60, b"", "cert problem")).unwrap_err();
    assert!(tls.to_string().contains("TLS failed"));
    assert!(format!("{tls:?}").contains("Tls"));
    let failed = interpret("curl", &output(7, b"", "")).unwrap_err();
    assert!(failed.to_string().contains("curl exited 7"));
    let missing = Error::Missing {
        program: "curl".to_owned(),
    };
    assert!(missing.to_string().contains("not on PATH"));
    let _dyn: &dyn std::error::Error = &missing;
}

#[test]
fn spawn_missing_failed_and_timeout() {
    let missing = run("bistill-host-missing-bin", &[], Duration::from_secs(1)).unwrap_err();
    assert!(matches!(missing, Error::Missing { .. }));
    let failed = run("/", &[], Duration::from_secs(1)).unwrap_err();
    assert!(matches!(failed, Error::Failed { .. }));
    assert!(failed.to_string().contains("failed"));
    let timed = run("sleep", &owned(&["5"]), Duration::from_millis(200)).unwrap_err();
    assert!(matches!(timed, Error::Timeout { .. }));
    assert!(timed.to_string().contains("timed out"));
    let signaled = run("sh", &owned(&["-c", "kill -9 $$"]), Duration::from_secs(2)).unwrap_err();
    assert!(matches!(signaled, Error::Failed { .. }));
    let ok = run("true", &[], Duration::from_secs(2)).unwrap();
    assert_eq!(ok.code, 0);
}

struct Served {
    port: u16,
    request: std::sync::mpsc::Receiver<Vec<u8>>,
    thread: thread::JoinHandle<()>,
}

fn serve(status: u16, body: &[u8], hold: bool) -> Served {
    serve_extra(status, body, hold, "")
}

fn serve_extra(status: u16, body: &[u8], hold: bool, extra: &str) -> Served {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
    let port = listener.local_addr().expect("addr").port();
    let body = body.to_vec();
    let extra = extra.to_owned();
    let (tx, request) = std::sync::mpsc::channel();
    let thread = thread::spawn(move || {
        let (mut sock, _) = listener.accept().expect("accept");
        let _ = sock.set_read_timeout(Some(Duration::from_secs(2)));
        let mut buf = vec![0u8; 8192];
        let n = sock.read(&mut buf).unwrap_or(0);
        let _ = tx.send(buf[..n].to_vec());
        if hold {
            thread::sleep(Duration::from_secs(2));
            return;
        }
        let head = format!(
            "HTTP/1.1 {status} X\r\n{extra}Content-Length: {}\r\nConnection: close\r\n\r\n",
            body.len()
        );
        let _ = sock.write_all(head.as_bytes());
        let _ = sock.write_all(&body);
    });
    Served {
        port,
        request,
        thread,
    }
}

#[test]
fn get_reads_status_headers_and_body() {
    let cases = [
        (200u16, &b"a\nb\n"[..]),
        (404, &b"missing"[..]),
        (500, &b""[..]),
        (204, &b""[..]),
    ];
    for (status, body) in cases {
        let served = serve(status, body, false);
        let mut request = sample(&format!("http://127.0.0.1:{}/item", served.port));
        request.timeout = Duration::from_secs(2);
        request.fail_with_body = status == 404;
        let response = get(&request).unwrap_or_else(|err| panic!("{err}"));
        assert_eq!(response.status, status, "{status}");
        assert_eq!(response.body, body);
        assert_eq!(response.retry_after, None);
        let bytes = served.request.recv().expect("request");
        let seen = String::from_utf8_lossy(&bytes).to_ascii_lowercase();
        assert!(seen.contains("authorization: bearer secret-token"));
        assert!(seen.contains("user-agent: bistill/0.1.0 (internal)"));
        assert!(seen.contains("accept: application/json"));
        served.thread.join().expect("server");
    }
}

#[test]
fn get_reports_tls_timeout_and_bad_ca() {
    let plain = serve(200, b"ok", false);
    let mut request = sample(&format!("https://127.0.0.1:{}", plain.port));
    request.timeout = Duration::from_secs(2);
    request.headers.clear();
    let err = get(&request).unwrap_err();
    assert!(matches!(err, Error::Tls { .. }), "{err}");
    let _ = plain.thread.join();

    let hung = serve(200, b"ok", true);
    let mut request = sample(&format!("http://127.0.0.1:{}/", hung.port));
    request.timeout = Duration::from_millis(400);
    request.headers.clear();
    let err = get(&request).unwrap_err();
    assert!(matches!(err, Error::Timeout { .. }), "{err}");
    let _ = hung.thread.join();

    let mut request = sample("http://127.0.0.1:9/");
    request.timeout = Duration::from_secs(2);
    request.ca_file = Some(PathBuf::from("/no/such/bistill-ca.pem"));
    request.headers.clear();
    let err = get(&request).unwrap_err();
    assert!(
        matches!(err, Error::Tls { .. } | Error::Failed { .. }),
        "{err}"
    );

    let mut request = sample("http://127.0.0.1:9/");
    request.program = "bistill-host-missing-curl".to_owned();
    request.timeout = Duration::from_secs(1);
    assert!(matches!(get(&request), Err(Error::Missing { .. })));

    let limited = serve_extra(429, b"slow", false, "Retry-After: 15\r\n");
    let mut request = sample(&format!("http://127.0.0.1:{}/", limited.port));
    request.timeout = Duration::from_secs(2);
    let response = get(&request).unwrap();
    assert_eq!(response.status, 429);
    assert_eq!(response.body, b"slow");
    assert_eq!(response.retry_after, Some(15));
    let _ = limited.thread.join();
}

#[test]
fn retry_after_reads_a_delay_or_an_http_date() {
    assert_eq!(
        retry_after_file(std::path::Path::new("/no/such/bistill-headers"), 0),
        None
    );
    assert_eq!(retry_after_value("Accept: text\n", 0), None);
    assert_eq!(retry_after_value("Retry-After: 15\n", 0), Some(15));
    assert_eq!(retry_after_value("retry-after: 0\n", 10), Some(0));
    assert_eq!(retry_after_value("Retry-After:\n", 0), None);
    assert_eq!(retry_after_value("Retry-After: nope\n", 0), None);
    assert_eq!(
        retry_after_value(
            "Retry-After: Sun, 06 Nov 1994 08:49:37 GMT\n",
            784111777 - 10
        ),
        Some(10)
    );
    assert_eq!(
        retry_after_value("Retry-After: Sun, 06 Nov 1994 08:49:37 GMT\n", 784111777),
        Some(0)
    );
    assert_eq!(
        retry_after_value(
            "Retry-After: Sun, 06 Nov 1994 08:49:37 GMT\n",
            784111777 + 5
        ),
        Some(0)
    );
    assert_eq!(
        retry_after_value("Retry-After: Thu, 01 Jan 1970 00:00:00 GMT\n", 0),
        Some(0)
    );
    assert_eq!(
        retry_after_value("Retry-After: Thu, 01 Jan 1970 24:00:00 GMT\n", 0),
        None
    );
    assert_eq!(
        retry_after_value("Retry-After: Thu, 32 Jan 1970 00:00:00 GMT\n", 0),
        None
    );
    assert_eq!(
        retry_after_value("Retry-After: Thu, 01 Foo 1970 00:00:00 GMT\n", 0),
        None
    );
    assert_eq!(
        retry_after_value("Retry-After: Thu, 01 Jan 1970 00:00:00 UTC\n", 0),
        None
    );
    assert_eq!(
        retry_after_value("Retry-After: Thu, 01 Jan 1969 00:00:00 GMT\n", 0),
        None
    );
    assert_eq!(secs_at(std::time::UNIX_EPOCH + Duration::from_secs(5)), 5);
    let early = std::time::UNIX_EPOCH
        .checked_sub(Duration::from_secs(1))
        .unwrap();
    assert_eq!(secs_at(early), 0);
    let _ = format!("{:?}", sample("https://git.example.invalid"));
}

fn notify(expire: Option<Duration>) -> Toast {
    Toast::NotifySend {
        program: "true".to_owned(),
        title: "Bistill".to_owned(),
        body: "PRJ/repo#12 needs review".to_owned(),
        expire,
        timeout: Duration::from_secs(2),
    }
}

fn powershell(url: Option<&str>) -> Toast {
    Toast::PowerShell {
        program: "true".to_owned(),
        title: "Bistill".to_owned(),
        body: "PRJ/repo#12 needs review".to_owned(),
        url: url.map(str::to_owned),
        timeout: Duration::from_secs(2),
    }
}

fn strings(args: &[OsString]) -> Vec<String> {
    args.iter()
        .map(|arg| arg.to_string_lossy().into_owned())
        .collect()
}

#[test]
fn toast_and_open_arguments_match_the_contract() {
    let notify_toast = Toast::NotifySend {
        program: "notify-send".to_owned(),
        title: "A&B<C>D\"E'F".to_owned(),
        body: "-lead".to_owned(),
        expire: Some(Duration::from_millis(1500)),
        timeout: Duration::from_secs(2),
    };
    let notify_args = strings(&toast_arguments(&notify_toast));
    assert_eq!(
        notify_args,
        vec!["--expire-time", "1500", "--", "A&B<C>D\"E'F", "-lead",]
    );
    assert!(
        !notify_args
            .iter()
            .any(|arg| arg == "--action" || arg == "--wait")
    );
    assert!(format!("{notify_toast:?}").contains("NotifySend"));
    let plain = strings(&toast_arguments(&notify(None)));
    assert_eq!(plain, vec!["--", "Bistill", "PRJ/repo#12 needs review"]);

    let powershell_toast = Toast::PowerShell {
        program: "powershell.exe".to_owned(),
        title: "A&B<C>D\"E'F".to_owned(),
        body: "-lead".to_owned(),
        url: Some("https://git.example.invalid/a?b=1&c=2".to_owned()),
        timeout: Duration::from_secs(2),
    };
    let script = strings(&toast_arguments(&powershell_toast));
    assert_eq!(&script[..3], ["-NoProfile", "-NonInteractive", "-Command"]);
    let command = &script[3];
    assert!(command.contains("activationType=\"protocol\""));
    assert!(command.contains("launch=\"https://git.example.invalid/a?b=1&amp;c=2\""));
    assert!(command.contains("<text>A&amp;B&lt;C&gt;D&quot;E&apos;F</text>"));
    assert!(command.contains("<text>-lead</text>"));
    assert!(command.contains(
        "CreateToastNotifier('{1AC14E77-02E7-4E5D-B744-2EB1AE5198B7}\\WindowsPowerShell\\v1.0\\powershell.exe')"
    ));
    assert!(format!("{powershell_toast:?}").contains("PowerShell"));

    let no_click = strings(&toast_arguments(&powershell(None)));
    assert!(!no_click[3].contains("activationType"));
    assert!(no_click[3].contains("<text>Bistill</text>"));

    let open = Open::XdgOpen {
        program: "xdg-open".to_owned(),
        url: "https://git.example.invalid/pr".to_owned(),
        timeout: Duration::from_secs(2),
    };
    assert_eq!(
        strings(&open_arguments(&open)),
        vec!["https://git.example.invalid/pr"]
    );
    assert!(format!("{open:?}").contains("XdgOpen"));
    let start = Open::WindowsStart {
        program: "powershell.exe".to_owned(),
        url: "https://git.example.invalid/it's".to_owned(),
        timeout: Duration::from_secs(2),
    };
    assert_eq!(
        strings(&open_arguments(&start)),
        vec![
            "-NoProfile",
            "-NonInteractive",
            "-Command",
            "Start-Process -FilePath 'https://git.example.invalid/it''s' -Verb Open",
        ]
    );
    assert!(format!("{start:?}").contains("WindowsStart"));
}

#[test]
fn toast_and_open_report_spawn_results() {
    assert!(toast(&notify(None)).is_ok());
    assert!(toast(&powershell(Some("https://git.example.invalid/pr"))).is_ok());

    let failed_toast = Toast::NotifySend {
        program: "false".to_owned(),
        title: "Bistill".to_owned(),
        body: "PRJ/repo#12 needs review".to_owned(),
        expire: None,
        timeout: Duration::from_secs(2),
    };
    let failed = toast(&failed_toast).unwrap_err();
    assert!(failed.to_string().contains("false exited 1"));
    let missing_toast = Toast::NotifySend {
        program: "bistill-host-missing-notify".to_owned(),
        title: "Bistill".to_owned(),
        body: "PRJ/repo#12 needs review".to_owned(),
        expire: None,
        timeout: Duration::from_secs(2),
    };
    assert!(matches!(toast(&missing_toast), Err(Error::Missing { .. })));

    let open = Open::XdgOpen {
        program: "true".to_owned(),
        url: "https://git.example.invalid/pr".to_owned(),
        timeout: Duration::from_secs(2),
    };
    assert!(open_url(&open).is_ok());
    assert!(
        open_url(&Open::WindowsStart {
            program: "true".to_owned(),
            url: "https://git.example.invalid/pr".to_owned(),
            timeout: Duration::from_secs(2),
        })
        .is_ok()
    );
    let failed = open_url(&Open::WindowsStart {
        program: "false".to_owned(),
        url: "https://git.example.invalid/pr".to_owned(),
        timeout: Duration::from_secs(2),
    })
    .unwrap_err();
    assert!(matches!(failed, Error::Failed { .. }));
    assert!(matches!(
        open_url(&Open::XdgOpen {
            program: "bistill-host-missing-open".to_owned(),
            url: "https://git.example.invalid/pr".to_owned(),
            timeout: Duration::from_secs(2),
        }),
        Err(Error::Missing { .. })
    ));
}

struct Pipe {
    inbound: Vec<u8>,
    at: usize,
    written: Vec<u8>,
    fail_write: bool,
    fail_read: bool,
    fail_read_after: bool,
    writes: usize,
    fail_write_at: Option<usize>,
    /// 0 = eof, 1 = timed out, 2 = would block, 3 = other error.
    end: u8,
}

fn field_bytes(fields_len: u32, fields: &[u8]) -> Vec<u8> {
    let mut raw = tray_unix::script_header(b'l', 0, fields_len);
    raw.extend_from_slice(fields);
    raw
}

fn pipe(inbound: &[u8]) -> Pipe {
    Pipe {
        inbound: inbound.to_vec(),
        at: 0,
        written: Vec::new(),
        fail_write: false,
        fail_read: false,
        fail_read_after: false,
        writes: 0,
        fail_write_at: None,
        end: 0,
    }
}

impl Read for Pipe {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        if self.fail_read {
            return Err(std::io::Error::other("closed"));
        }
        if self.at >= self.inbound.len() {
            if self.fail_read_after || self.end == 3 {
                return Err(std::io::Error::other("closed"));
            }
            if self.end == 1 {
                return Err(std::io::Error::new(std::io::ErrorKind::TimedOut, "idle"));
            }
            if self.end == 2 {
                return Err(std::io::Error::new(std::io::ErrorKind::WouldBlock, "idle"));
            }
            Ok(0)
        } else {
            let n = buf.len().min(self.inbound.len() - self.at);
            buf[..n].copy_from_slice(&self.inbound[self.at..self.at + n]);
            self.at += n;
            Ok(n)
        }
    }
}

impl Write for Pipe {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        if self.fail_write || self.fail_write_at == Some(self.writes) {
            Err(std::io::Error::other("closed"))
        } else {
            self.writes += 1;
            self.written.extend_from_slice(buf);
            Ok(buf.len())
        }
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

#[test]
fn tray_registers_and_reports_a_missing_watcher() {
    assert_eq!(tray_tip(0), "Nothing new");
    assert_eq!(tray_tip(1), "1 unread");
    assert_eq!(tray_tip(4), "4 unread");
    let (quit, quit_rx) = std::sync::mpsc::channel();
    drop(quit);
    tray_unix::run(quit_rx, &mut |_| {});
    let (tx, rx) = std::sync::mpsc::channel();
    tx.send(1).unwrap();
    tx.send(2).unwrap();
    drop(tx);
    let mut log = String::new();
    tray_unix::run_at(rx, None, std::path::Path::new("/dev/null"), &mut |line| {
        log.push_str(line);
    });
    assert_eq!(log.matches("no status icon watcher").count(), 1);
    let mut missing = pipe(b"");
    assert!(tray_unix::handshake(&mut missing).is_err());
    let mut rejected = pipe(b"REJECTED\r\n");
    assert!(tray_unix::handshake(&mut rejected).is_err());
    let mut closed = pipe(b"");
    closed.fail_write = true;
    assert!(tray_unix::handshake(&mut closed).is_err());
    let mut dead = pipe(b"OK 1\r\n");
    dead.fail_read = true;
    assert!(tray_unix::handshake(&mut dead).is_err());
    let mut late = pipe(b"OK 1\r\n");
    late.fail_read_after = true;
    assert!(tray_unix::handshake(&mut late).is_err());
    let mut long = pipe(&vec![b'A'; 513]);
    assert!(tray_unix::handshake(&mut long).is_err());
    let mut begin = pipe(b"OK 1\r\n");
    begin.fail_write_at = Some(1);
    assert!(tray_unix::handshake(&mut begin).is_err());
    let mut hello_write = pipe(b"OK 1\r\n");
    hello_write.fail_write_at = Some(2);
    assert!(tray_unix::handshake(&mut hello_write).is_err());
    let mut register_write = pipe(b"OK 1\r\n");
    register_write
        .inbound
        .extend(tray_unix::script_return(1, ":1.7"));
    register_write.fail_write_at = Some(3);
    assert!(tray_unix::handshake(&mut register_write).is_err());
    let mut hello_err = pipe(b"OK 1\r\n");
    hello_err.inbound.extend(tray_unix::script_error(1));
    assert!(tray_unix::handshake(&mut hello_err).is_err());
    let mut watcher = pipe(b"OK 1\r\n");
    watcher.inbound.extend(tray_unix::script_return(1, ":1.7"));
    watcher.inbound.extend(tray_unix::script_error(2));
    assert!(tray_unix::handshake(&mut watcher).is_err());
    let mut timed = pipe(b"OK 1\r\n");
    timed.end = 1;
    assert!(tray_unix::handshake(&mut timed).is_err());
    let mut blocked = pipe(b"");
    blocked.end = 2;
    assert!(tray_unix::pull(&mut blocked).unwrap().is_none());
    let mut other = pipe(b"");
    other.end = 3;
    assert!(tray_unix::pull(&mut other).is_err());
    let mut garbage = pipe(b"xxxxxxxxxxxxxxxx");
    assert!(tray_unix::pull(&mut garbage).is_err());
    let mut huge_body = pipe(&tray_unix::script_header(b'l', 2_000_000, 0));
    assert!(tray_unix::pull(&mut huge_body).is_err());
    let mut huge_fields = pipe(&tray_unix::script_header(b'l', 0, 2_000_000));
    assert!(tray_unix::pull(&mut huge_fields).is_err());
    let mut short_fields = pipe(&tray_unix::script_header(b'l', 0, 8));
    assert!(tray_unix::pull(&mut short_fields).is_err());
    let mut slow_fields = pipe(&tray_unix::script_header(b'l', 0, 8));
    slow_fields.end = 1;
    assert!(tray_unix::pull(&mut slow_fields).is_err());
    let mut odd = pipe(&tray_unix::script_odd());
    assert!(tray_unix::pull(&mut odd).is_ok());
    let mut bad = pipe(&tray_unix::script_bad());
    assert!(tray_unix::pull(&mut bad).is_err());
    let mut ignored = pipe(&tray_unix::script_odd());
    let msg = tray_unix::pull(&mut ignored).unwrap().unwrap();
    assert_ne!(msg.kind, 1);

    let dir = std::env::temp_dir().join(format!("bistill-tray-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let tty = dir.join("tty");
    std::fs::write(&tty, b"").unwrap();
    tray_unix::raise_terminal(&tty);
    assert!(
        std::fs::read(&tty)
            .unwrap()
            .windows(2)
            .any(|pair| pair == b"\x1b[")
    );
    tray_unix::raise_terminal(dir.join("missing").as_path());

    let mut closed = pipe(b"");
    assert!(tray_unix::pull(&mut closed).is_err());
    let mut mid = pipe(&tray_unix::script_header(b'l', 0, 0)[..8]);
    mid.end = 3;
    assert!(tray_unix::pull(&mut mid).is_err());
    let mut pad_fail = pipe(&{
        let mut bytes = tray_unix::script_header(b'l', 0, 1);
        bytes.push(0);
        bytes
    });
    pad_fail.end = 3;
    assert!(tray_unix::pull(&mut pad_fail).is_err());
    let mut body_fail = pipe(&tray_unix::script_header(b'l', 4, 0));
    body_fail.end = 3;
    assert!(tray_unix::pull(&mut body_fail).is_err());
    assert!(tray_unix::decode(&field_bytes(8, &[8, 1, b'g', 0, 0, 0, 0]), 16).is_err());
    assert!(tray_unix::decode(&field_bytes(8, &[8, 1, b'g', 0, 0, 0, 0, 0]), 16).is_err());
    assert!(tray_unix::decode(&field_bytes(8, &[5, 1, b'u', 0, 0, 0]), 16).is_err());
    assert!(tray_unix::decode(&field_bytes(8, &[8, 50]), 16).is_err());
    let mut long_string = vec![6, 1, b's', 0];
    long_string.extend_from_slice(&1000u32.to_le_bytes());
    assert!(tray_unix::decode(&field_bytes(8, &long_string), 16).is_err());
    let (tx, rx) = std::sync::mpsc::channel();
    tx.send(1).unwrap();
    drop(tx);
    let mut log = String::new();
    tray_unix::run_at(
        rx,
        Some("tcp:host=127.0.0.1"),
        std::path::Path::new("/dev/null"),
        &mut |line| log.push_str(line),
    );
    assert!(log.contains("no unix path"));
    let (tx, rx) = std::sync::mpsc::channel();
    tx.send(1).unwrap();
    drop(tx);
    let mut log = String::new();
    tray_unix::run_at(
        rx,
        Some("unix:path=/tmp/bistill-no-such-bus"),
        std::path::Path::new("/dev/null"),
        &mut |line| log.push_str(line),
    );
    assert!(log.contains("tray failed"));
    tray_unix::updates_fail_when_the_socket_stops_reading();

    let listener = std::os::unix::net::UnixListener::bind(dir.join("bus")).unwrap();
    let (ready_tx, ready_rx) = std::sync::mpsc::channel();
    let server = std::thread::spawn(move || {
        let (mut sock, _) = listener.accept().unwrap();
        let _ = sock.set_read_timeout(Some(std::time::Duration::from_secs(2)));
        let mut byte = [0u8; 1];
        loop {
            let n = sock.read(&mut byte).unwrap();
            if n == 0 || byte[0] == b'\n' {
                break;
            }
        }
        sock.write_all(b"OK 1\r\n").unwrap();
        loop {
            let n = sock.read(&mut byte).unwrap();
            if n == 0 || byte[0] == b'\n' {
                break;
            }
        }
        let hello = tray_unix::pull(&mut sock).unwrap().unwrap();
        assert_eq!(hello.member, "Hello");
        sock.write_all(&tray_unix::script_return(1, ":1.42"))
            .unwrap();
        let register = tray_unix::pull(&mut sock).unwrap().unwrap();
        assert_eq!(register.member, "RegisterStatusNotifierItem");
        sock.write_all(&tray_unix::script_odd()).unwrap();
        sock.write_all(&tray_unix::script_call(
            "Introspect",
            "s",
            &tray_unix::script_string(""),
        ))
        .unwrap();
        sock.write_all(&tray_unix::script_return(2, "")).unwrap();
        let early = tray_unix::pull(&mut sock).unwrap().unwrap();
        assert_eq!(early.kind, 2);
        let get_all = tray_unix::script_call(
            "GetAll",
            "s",
            &tray_unix::script_string("org.kde.StatusNotifierItem"),
        );
        sock.write_all(&get_all).unwrap();
        let listed = tray_unix::pull(&mut sock).unwrap().unwrap();
        assert_eq!(listed.kind, 2);
        assert!(listed.raw.windows(9).any(|item| item == b"mail-read"));
        assert!(listed.raw.windows(11).any(|item| item == b"Nothing new"));
        for name in [
            "Category",
            "Id",
            "Title",
            "Status",
            "IconName",
            "IconPixmap",
            "ToolTip",
            "Nope",
        ] {
            let body = tray_unix::script_two_strings("org.kde.StatusNotifierItem", name);
            sock.write_all(&tray_unix::script_call("Get", "ss", &body))
                .unwrap();
            let got = tray_unix::pull(&mut sock).unwrap().unwrap();
            if name == "Nope" {
                assert_eq!(got.kind, 3);
            } else {
                assert_eq!(got.kind, 2, "{name}");
            }
        }
        sock.write_all(&tray_unix::script_call(
            "Introspect",
            "s",
            &tray_unix::script_string(""),
        ))
        .unwrap();
        assert_eq!(tray_unix::pull(&mut sock).unwrap().unwrap().kind, 2);
        sock.write_all(&tray_unix::script_call(
            "Activate",
            "s",
            &tray_unix::script_string(""),
        ))
        .unwrap();
        assert_eq!(tray_unix::pull(&mut sock).unwrap().unwrap().kind, 2);
        sock.write_all(&tray_unix::script_call(
            "ContextMenu",
            "s",
            &tray_unix::script_string(""),
        ))
        .unwrap();
        assert_eq!(tray_unix::pull(&mut sock).unwrap().unwrap().kind, 3);
        sock.write_all(&tray_unix::script_odd()).unwrap();
        ready_tx.send(()).unwrap();
        let mut buf = [0u8; 8192];
        let mut got = Vec::new();
        let start = std::time::Instant::now();
        while start.elapsed() < std::time::Duration::from_millis(500) {
            match sock.read(&mut buf) {
                Ok(0) => break,
                Ok(n) => {
                    got.extend_from_slice(&buf[..n]);
                    if got.windows(11).any(|item| item == b"mail-unread") {
                        break;
                    }
                }
                Err(err)
                    if err.kind() == std::io::ErrorKind::TimedOut
                        || err.kind() == std::io::ErrorKind::WouldBlock => {}
                Err(err) => panic!("tray read {err}"),
            }
        }
        assert!(got.windows(7).any(|item| item == b"NewIcon"));
        assert!(got.windows(11).any(|item| item == b"mail-unread"));
        drop(sock);
    });
    let address = format!("unix:path={}", dir.join("bus").display());
    let tty_path = tty.clone();
    let (tx, rx) = std::sync::mpsc::channel();
    let runner = std::thread::spawn(move || {
        let mut log = String::new();
        tray_unix::run_at(rx, Some(&address), &tty_path, &mut |line| {
            log.push_str(line)
        });
        log
    });
    tx.send(0).unwrap();
    ready_rx
        .recv_timeout(std::time::Duration::from_secs(3))
        .unwrap();
    assert!(
        std::fs::read(&tty)
            .unwrap()
            .windows(2)
            .any(|pair| pair == b"\x1b[")
    );
    tx.send(4).unwrap();
    std::thread::sleep(std::time::Duration::from_millis(400));
    drop(tx);
    let _ = runner.join().unwrap();
    server.join().unwrap();
    std::fs::remove_dir_all(&dir).unwrap();
}

#[cfg(unix)]
#[test]
fn tray_wait_wakes_on_the_count_pipe() {
    use std::io::{self, Read};
    use std::os::unix::io::AsRawFd;
    use std::os::unix::net::UnixStream;

    assert_eq!(tray_unix::ready_flags(true, None), (true, false));
    assert_eq!(tray_unix::ready_flags(false, None), (false, true));
    assert_eq!(
        tray_unix::ready_flags(false, Some((false, false))),
        (false, false)
    );
    assert_eq!(
        tray_unix::ready_flags(true, Some((true, false))),
        (true, false)
    );
    assert_eq!(
        tray_unix::ready_flags(true, Some((false, true))),
        (false, true)
    );
    assert_eq!(tray_unix::poll_once(None, i32::MAX), Some((false, false)));
    assert_eq!(
        tray_unix::poll_once(Some(i32::MAX), i32::MAX),
        Some((false, false))
    );
    let (idle, _idle_w) = UnixStream::pair().unwrap();
    assert!(tray_unix::poll_timed(None, idle.as_raw_fd(), 0).is_none());

    let (mut poke_r, mut poke_w) = UnixStream::pair().unwrap();
    poke_r.set_nonblocking(true).unwrap();
    poke_w.write_all(&[1, 0]).unwrap();
    let flags = tray_unix::poll_once(None, poke_r.as_raw_fd()).unwrap();
    assert_eq!(flags, (false, true));
    assert!(tray_unix::drain_poke(&mut poke_r));
    let (bus_r, mut bus_w) = UnixStream::pair().unwrap();
    let (quiet, _quiet_w) = UnixStream::pair().unwrap();
    bus_w.write_all(&[1]).unwrap();
    let both = tray_unix::poll_once(Some(bus_r.as_raw_fd()), quiet.as_raw_fd()).unwrap();
    assert_eq!(both, (true, false));

    struct Script {
        reads: Vec<io::Result<Vec<u8>>>,
    }
    impl Read for Script {
        fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
            match self.reads.remove(0) {
                Ok(bytes) => {
                    let n = bytes.len().min(buf.len());
                    buf[..n].copy_from_slice(&bytes[..n]);
                    Ok(n)
                }
                Err(err) => Err(err),
            }
        }
    }
    let mut open = Script {
        reads: vec![
            Ok(vec![1]),
            Err(io::Error::new(io::ErrorKind::WouldBlock, "later")),
        ],
    };
    assert!(!tray_unix::drain_poke(&mut open));
    let mut stop = Script {
        reads: vec![
            Ok(vec![0]),
            Err(io::Error::new(io::ErrorKind::WouldBlock, "later")),
        ],
    };
    assert!(tray_unix::drain_poke(&mut stop));
    let mut end = Script {
        reads: vec![Ok(Vec::new())],
    };
    assert!(tray_unix::drain_poke(&mut end));
    let mut broken = Script {
        reads: vec![Err(io::Error::other("closed"))],
    };
    assert!(tray_unix::drain_poke(&mut broken));
    tray_unix::poll_without_a_tray();
}
