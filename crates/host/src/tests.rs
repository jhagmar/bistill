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
            body: b"nope".to_vec()
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
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
    let port = listener.local_addr().expect("addr").port();
    let body = body.to_vec();
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
            "HTTP/1.1 {status} X\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
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
