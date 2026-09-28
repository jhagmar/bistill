use super::*;
use bistill_lib::{Bodies, CurlFault, InboxCount, JsonError, Product, User};
use std::ffi::OsString;
use std::fs;
use std::io::{Read, Write};
use std::net::TcpListener;
use std::path::{Path, PathBuf};
use std::thread;
use std::time::{Duration, Instant};

fn args(words: &[&str]) -> Vec<OsString> {
    let mut out = vec![OsString::from("bistill")];
    out.extend(words.iter().map(OsString::from));
    out
}

fn env_token() -> Env {
    let mut env = Env::new();
    env.base_url = Some("https://git.example.invalid".to_owned());
    env.username = Some("jcitizen".to_owned());
    env.token = Some("secret-token".to_owned());
    env
}

fn temp(name: &str) -> PathBuf {
    let path = std::env::temp_dir().join(format!("bistill-bin-{name}-{}", std::process::id()));
    if path.exists() {
        fs::remove_dir_all(&path).unwrap();
    }
    fs::create_dir_all(&path).unwrap();
    fs::write(
        path.join("bistill.conf"),
        "state_dir = /tmp/bistill-state\n",
    )
    .unwrap();
    path
}

fn report() -> Report {
    Report {
        product: Product {
            display_name: "Bitbucket".to_owned(),
            version: "8.19.0".to_owned(),
        },
        user: User {
            slug: "jcitizen".to_owned(),
            display_name: "Jane Citizen".to_owned(),
        },
        inbox: InboxCount::Split {
            reviewer: 2,
            author: 1,
        },
        bodies: Bodies {
            application_properties: b"{\"displayName\":\"Bitbucket\"}".to_vec(),
            user: b"{\"slug\":\"jcitizen\"}".to_vec(),
            inbox_count: b"{\"reviewer\":2,\"author\":1}\n".to_vec(),
        },
    }
}

struct Script {
    version: Option<Result<String, Error>>,
    report: Option<Result<Report, Error>>,
    origin: String,
}

impl Session for Script {
    fn version(&mut self, program: &str) -> Result<String, Error> {
        assert_eq!(program, curl_bin::PROGRAM);
        self.version.take().expect("version")
    }

    fn ping(&mut self, client: &Client) -> Result<Report, Error> {
        self.origin = client.requests()[0].url.clone();
        self.report.take().expect("ping")
    }
}

fn run(words: &[&str], cwd: &Path, env: &Env, script: Script) -> (i32, String, String, String) {
    let mut script = script;
    let mut stdout = Vec::new();
    let mut stderr = Vec::new();
    let code = execute(
        &args(words),
        cwd,
        env,
        &mut stdout,
        &mut stderr,
        &mut script,
    );
    (
        code,
        String::from_utf8(stdout).unwrap(),
        String::from_utf8(stderr).unwrap(),
        script.origin,
    )
}

fn ready(report: Result<Report, Error>) -> Script {
    Script {
        version: Some(Ok("curl 8.5.0".to_owned())),
        report: Some(report),
        origin: String::new(),
    }
}

#[test]
fn help_lists_ping_options() {
    let cwd = temp("help");
    let (code, stdout, stderr, _) = run(&["--help"], &cwd, &env_token(), ready(Ok(report())));
    assert_eq!(code, 0);
    assert!(stdout.contains("bistill ping"));
    assert!(stdout.contains("--json"));
    assert!(stdout.contains("--verbose"));
    assert!(!stdout.contains("The Bitbucket list you were missing."));
    assert!(stderr.is_empty());
    let (again, text, _, _) = run(&["-h"], &cwd, &env_token(), ready(Ok(report())));
    assert_eq!(again, 0);
    assert_eq!(text, stdout);
    let (from_ping, ping_help, _, _) = run(
        &["ping", "--json", "--help"],
        &cwd,
        &env_token(),
        ready(Ok(report())),
    );
    assert_eq!(from_ping, 0);
    assert_eq!(ping_help, stdout);
    fs::remove_dir_all(&cwd).unwrap();
}

#[test]
fn usage_names_the_next_step() {
    let cwd = temp("usage");
    let env = env_token();
    let (code, stdout, stderr, _) = run(&[], &cwd, &env, ready(Ok(report())));
    assert_eq!(code, 1);
    assert!(stdout.is_empty());
    assert!(stderr.contains("Run ping."));
    assert!(stderr.contains("Usage: bistill ping"));
    let (_, _, unknown, _) = run(&["ls"], &cwd, &env, ready(Ok(report())));
    assert!(unknown.contains("Unknown argument."));
    let (_, _, missing, _) = run(&["ping", "--url"], &cwd, &env, ready(Ok(report())));
    assert!(missing.contains("--url needs a value."));
    let (_, _, dashed, _) = run(
        &["ping", "--url", "--json"],
        &cwd,
        &env,
        ready(Ok(report())),
    );
    assert!(dashed.contains("--url needs a value."));
    let (_, _, flag, _) = run(&["ping", "--color"], &cwd, &env, ready(Ok(report())));
    assert!(flag.contains("Unknown argument."));
    fs::remove_dir_all(&cwd).unwrap();
}

#[cfg(unix)]
#[test]
fn usage_rejects_non_utf8_arguments() {
    use std::os::unix::ffi::OsStringExt;
    let cwd = temp("utf8");
    let bad = OsString::from_vec(vec![0xff]);
    let mut argv = args(&["ping"]);
    argv.push(bad.clone());
    let mut stdout = Vec::new();
    let mut stderr = Vec::new();
    let mut script = ready(Ok(report()));
    let code = execute(
        &argv,
        &cwd,
        &env_token(),
        &mut stdout,
        &mut stderr,
        &mut script,
    );
    assert_eq!(code, 1);
    assert!(String::from_utf8(stderr).unwrap().contains("UTF-8"));
    let mut argv = args(&["ping", "--user"]);
    argv.push(bad);
    let mut stderr = Vec::new();
    let mut script = ready(Ok(report()));
    let code = execute(
        &argv,
        &cwd,
        &env_token(),
        &mut Vec::new(),
        &mut stderr,
        &mut script,
    );
    assert_eq!(code, 1);
    assert!(String::from_utf8(stderr).unwrap().contains("UTF-8"));
    fs::remove_dir_all(&cwd).unwrap();
}

#[test]
fn ping_prints_plain_text() {
    let cwd = temp("plain");
    let (code, stdout, stderr, origin) = run(&["ping"], &cwd, &env_token(), ready(Ok(report())));
    assert_eq!(code, 0);
    assert_eq!(
        stdout,
        "curl 8.5.0\nTLS ok.\nBitbucket 8.19.0.\nJane Citizen (jcitizen).\nreviewer 2, author 1\n"
    );
    assert!(stderr.is_empty());
    assert!(origin.ends_with('/'));
    assert!(!stdout.contains("secret-token"));
    fs::remove_dir_all(&cwd).unwrap();
}

#[test]
fn ping_json_prints_raw_bodies() {
    let cwd = temp("json");
    let (code, stdout, stderr, _) =
        run(&["ping", "--json"], &cwd, &env_token(), ready(Ok(report())));
    assert_eq!(code, 0);
    assert_eq!(
        stdout,
        "{\"displayName\":\"Bitbucket\"}\n{\"slug\":\"jcitizen\"}\n{\"reviewer\":2,\"author\":1}\n"
    );
    assert!(!stdout.contains("TLS ok."));
    assert!(stderr.is_empty());
    fs::remove_dir_all(&cwd).unwrap();
}

#[test]
fn verbose_redacts_the_token_and_writes_the_log() {
    let cwd = temp("verbose");
    let log = cwd.join("bistill.log");
    fs::write(
        cwd.join("bistill.conf"),
        format!(
            "state_dir = /tmp/bistill-state\nlog_file = {}\n",
            log.display()
        ),
    )
    .unwrap();
    let (code, stdout, stderr, origin) = run(
        &[
            "ping",
            "--verbose",
            "--url",
            "https://override.example",
            "--user",
            "jcitizen",
        ],
        &cwd,
        &env_token(),
        ready(Err(Error::Http(401))),
    );
    assert_eq!(code, 11);
    assert!(stdout.is_empty());
    assert!(stderr.contains("Bearer ***"));
    assert!(!stderr.contains("secret-token"));
    assert!(stderr.contains("Token rejected."));
    assert!(origin.contains("https://override.example/"));
    let logged = fs::read_to_string(&log).unwrap();
    assert_eq!(logged, stderr);
    fs::remove_dir_all(&cwd).unwrap();
}

#[test]
fn flags_supply_the_token_file_and_config() {
    let cwd = temp("flags");
    let token = cwd.join("token");
    fs::write(&token, "secret-token\n").unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&token, fs::Permissions::from_mode(0o600)).unwrap();
    }
    let config = cwd.join("explicit.conf");
    fs::write(&config, "state_dir = /tmp/bistill-state\n").unwrap();
    let env = Env::new();
    let (code, stdout, stderr, _) = run(
        &[
            "ping",
            "--config",
            config.to_str().unwrap(),
            "--token-file",
            token.to_str().unwrap(),
            "--url",
            "https://git.example.invalid",
            "--user",
            "jcitizen",
            "--json",
        ],
        &cwd,
        &env,
        ready(Ok(report())),
    );
    assert_eq!(code, 0, "{stderr}");
    assert!(stdout.contains("displayName"));
    assert!(!stdout.contains("secret-token"));
    assert!(!stderr.contains("secret-token"));
    fs::remove_dir_all(&cwd).unwrap();
}

#[test]
fn ping_maps_failures() {
    let cwd = temp("fail");
    let env = env_token();
    let cases = [
        (
            ready_version(Err(Error::Curl(CurlFault::Missing {
                program: "curl".to_owned(),
            }))),
            2,
            "curl must be on PATH.",
        ),
        (
            ready_version(Err(Error::Tls("verify".to_owned()))),
            3,
            "curl failed TLS.",
        ),
        (
            ready_version(Err(Error::Curl(CurlFault::Timeout {
                program: "curl".to_owned(),
            }))),
            4,
            "timed out",
        ),
        (
            ready_version(Err(Error::Curl(CurlFault::Failed {
                program: "curl".to_owned(),
                message: "exited 7".to_owned(),
            }))),
            1,
            "curl failed: exited 7",
        ),
        (
            ready(Err(Error::Json(JsonError {
                message: "truncated".to_owned(),
                offset: 1,
                line: 1,
                column: 2,
            }))),
            5,
            "truncated",
        ),
        (ready(Err(Error::Http(403))), 12, "HTTP 403"),
        (ready(Err(Error::Http(404))), 13, "HTTP 404"),
        (ready(Err(Error::Http(500))), 10, "HTTP 500"),
        (
            ready(Err(Error::Auth(
                "slug other does not match username jcitizen".to_owned(),
            ))),
            1,
            "does not match",
        ),
        (
            ready(Err(Error::Io(std::io::Error::other("disk")))),
            1,
            "disk",
        ),
    ];
    for (script, code, needle) in cases {
        let (got, stdout, stderr, _) = run(&["ping"], &cwd, &env, script);
        assert_eq!(got, code, "{stderr}");
        assert!(stdout.is_empty());
        assert!(stderr.contains(needle), "{stderr}");
        assert!(!stderr.contains("secret-token"));
    }
    let (got, _, stderr, _) = run(&["ping"], &cwd, &Env::new(), ready(Ok(report())));
    assert_eq!(got, 1);
    assert!(stderr.contains("missing base_url"));
    fs::remove_dir_all(&cwd).unwrap();
}

fn ready_version(err: Result<String, Error>) -> Script {
    Script {
        version: Some(err),
        report: None,
        origin: String::new(),
    }
}

#[test]
fn a_directory_log_file_exits_1() {
    let cwd = temp("logdir");
    let dir = cwd.join("logs");
    fs::create_dir(&dir).unwrap();
    fs::write(
        cwd.join("bistill.conf"),
        format!(
            "state_dir = /tmp/bistill-state\nlog_file = {}\n",
            dir.display()
        ),
    )
    .unwrap();
    let (code, stdout, stderr, _) = run(&["ping"], &cwd, &env_token(), ready(Ok(report())));
    assert_eq!(code, 1, "{stderr}");
    assert!(stdout.is_empty());
    assert!(stderr.contains("directory"), "{stderr}");
    fs::remove_dir_all(&cwd).unwrap();
}

struct FailWrite;

impl Write for FailWrite {
    fn write(&mut self, _buf: &[u8]) -> std::io::Result<usize> {
        Err(std::io::Error::other("full"))
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

#[test]
fn write_failures_exit_1() {
    let cwd = temp("write");
    let env = env_token();
    let mut script = ready(Ok(report()));
    let mut stderr = Vec::new();
    let code = execute(
        &args(&["--help"]),
        &cwd,
        &env,
        &mut FailWrite,
        &mut stderr,
        &mut script,
    );
    assert_eq!(code, 1);
    assert!(String::from_utf8(stderr).unwrap().contains("full"));
    let mut script = ready(Ok(report()));
    let mut stdout = Vec::new();
    let code = execute(
        &args(&[]),
        &cwd,
        &env,
        &mut stdout,
        &mut FailWrite,
        &mut script,
    );
    assert_eq!(code, 1);
    assert!(String::from_utf8(stdout).unwrap().contains("full"));
    fs::remove_dir_all(&cwd).unwrap();
}

#[test]
fn curl_version_distinguishes_missing_empty_and_failure() {
    let mut live = Live;
    let missing = live.version("bistill-missing-curl").unwrap_err();
    assert!(
        matches!(missing, Error::Curl(CurlFault::Missing { .. })),
        "{missing}"
    );
    let empty = live.version("true").unwrap_err();
    assert!(
        matches!(empty, Error::Curl(CurlFault::Failed { .. })),
        "{empty}"
    );
    let failed = live.version("false").unwrap_err();
    assert!(
        matches!(failed, Error::Curl(CurlFault::Failed { .. })),
        "{failed}"
    );
    let ok = live.version(curl_bin::PROGRAM).unwrap();
    assert!(ok.starts_with("curl "));
}

#[test]
fn ping_through_curl_reads_a_local_server() {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
    let port = listener.local_addr().expect("addr").port();
    listener.set_nonblocking(true).expect("nonblocking");
    let app = b"{\"displayName\":\"Bitbucket\",\"version\":\"8.19.0\"}";
    let user = b"{\"slug\":\"jcitizen\",\"displayName\":\"Jane Citizen\"}";
    let count = b"{\"count\":3}";
    let thread = thread::spawn(move || {
        let deadline = Instant::now() + Duration::from_secs(10);
        let mut done = 0;
        while done < 4 && Instant::now() < deadline {
            match listener.accept() {
                Ok((mut sock, _)) => {
                    let _ = sock.set_read_timeout(Some(Duration::from_secs(2)));
                    let mut buf = [0u8; 8192];
                    let n = sock.read(&mut buf).unwrap_or(0);
                    let req = String::from_utf8_lossy(&buf[..n]);
                    let body: &[u8] = if req.contains("application-properties") {
                        app
                    } else if req.contains("/users/") {
                        user
                    } else if req.contains("pull-requests/count") {
                        count
                    } else {
                        b"ok"
                    };
                    let head = format!(
                        "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                        body.len()
                    );
                    let _ = sock.write_all(head.as_bytes());
                    let _ = sock.write_all(body);
                    done += 1;
                }
                Err(err) if err.kind() == std::io::ErrorKind::WouldBlock => {
                    thread::sleep(Duration::from_millis(10));
                }
                Err(_) => break,
            }
        }
    });
    let cwd = temp("live");
    let mut env = env_token();
    env.base_url = Some(format!("http://127.0.0.1:{port}"));
    let mut stdout = Vec::new();
    let mut stderr = Vec::new();
    let code = execute(
        &args(&["ping"]),
        &cwd,
        &env,
        &mut stdout,
        &mut stderr,
        &mut Live,
    );
    let text = String::from_utf8(stdout).unwrap();
    assert_eq!(code, 0, "{} {text}", String::from_utf8_lossy(&stderr));
    assert!(text.contains("TLS ok."));
    assert!(text.contains("Bitbucket 8.19.0."));
    assert!(text.contains("Jane Citizen (jcitizen)."));
    assert!(text.contains('\n'));
    assert!(text.lines().next().unwrap().starts_with("curl "));
    assert!(text.ends_with("3\n"));
    thread.join().expect("server");
    fs::remove_dir_all(&cwd).unwrap();
}
