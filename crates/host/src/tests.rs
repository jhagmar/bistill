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
