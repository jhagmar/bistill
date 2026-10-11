use super::*;
use bistill_lib::{Bodies, CurlFault, InboxCount, JsonError, Listed, Product, Row, User};
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
        format!("state_dir = {}\n", path.display()),
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
    listed: Option<Result<Listed, Error>>,
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

    fn poll(
        &mut self,
        client: &Client,
        now_ms: u64,
        _cached: &[Row],
        publish: &mut dyn FnMut(&Snapshot) -> Result<(), Error>,
    ) -> Result<Listed, bistill_lib::InboxFault> {
        let _ = now_ms;
        self.origin = client.requests()[0].url.clone();
        let listed = self.listed.take().expect("list");
        listed
            .and_then(|listed| {
                publish(&listed.snapshot)?;
                Ok(listed)
            })
            .map_err(|error| bistill_lib::InboxFault {
                error,
                retry_after_ms: None,
            })
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
        listed: None,
        origin: String::new(),
    }
}

#[test]
fn help_lists_ping_options() {
    let cwd = temp("help");
    let (code, stdout, stderr, _) = run(&["--help"], &cwd, &env_token(), ready(Ok(report())));
    assert_eq!(code, 0);
    assert!(stdout.contains("bistill ping"));
    assert!(stdout.contains("bistill tui"));
    assert!(stdout.contains("--json"));
    assert!(stdout.contains("--verbose"));
    assert!(!stdout.contains("bistill watch"));
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
    assert!(stderr.contains("Run ping, or start the inbox"));
    assert!(stderr.contains("Usage: bistill ping"));
    let (_, _, unknown, _) = run(&["serve"], &cwd, &env, ready(Ok(report())));
    assert!(unknown.contains("Unknown argument."));
    let (_, _, counted, _) = run(&["ping", "--count"], &cwd, &env, ready(Ok(report())));
    assert!(counted.contains("Unknown argument."));
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
    let (_, _, watch_json, _) = run(&["watch", "--json"], &cwd, &env, ready(Ok(report())));
    assert!(watch_json.contains("Unknown argument."));
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
        listed: None,
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

fn sample_row(
    project: &str,
    repo: &str,
    number: u64,
    title: &str,
    draft: bool,
    stale: bool,
    needs_work: bool,
) -> Row {
    Row {
        id: format!("{project}/{repo}/{number}"),
        project: project.to_owned(),
        repo: repo.to_owned(),
        number,
        title: title.to_owned(),
        author: "Pat".to_owned(),
        from_branch: "feature".to_owned(),
        to_branch: "main".to_owned(),
        reviewers: Vec::new(),
        created_ms: 1,
        updated_ms: 1,
        html_url: format!("https://git.example.invalid/pull/{number}"),
        draft,
        stale,
        needs_work,
        unanswered_as_author: 0,
        unanswered_as_reviewer: 0,
        open_tasks: 0,
        enrichment: bistill_lib::Enrichment::Ready,
        build: bistill_lib::Build::None,
        conflicted: false,
        can_merge: false,
        fingerprint: String::new(),
        events: Vec::new(),
        events_loaded: false,
    }
}

fn sample_snapshot(needs_review: Vec<Row>, waiting: Vec<Row>) -> Snapshot {
    let mut snapshot = Snapshot {
        fetched_ms: 1,
        user_slug: "jcitizen".to_owned(),
        user_name: "Jane Citizen".to_owned(),
        bitbucket_version: "8.19.0".to_owned(),
        bitbucket_name: "Bitbucket".to_owned(),
        status: bistill_lib::SnapshotStatus::Ok,
        status_since_ms: 1,
        truncated: 0,
        poll_seconds: 60,
        needs_review,
        waiting,
    };
    bistill_lib::stamp(&mut snapshot);
    snapshot
}

#[test]
fn ls_and_watch_are_unknown_commands() {
    let cwd = temp("ls");
    let env = env_token();
    let (code, stdout, stderr, _) = run(&["ls"], &cwd, &env, ready(Ok(report())));
    assert_eq!(code, 1);
    assert!(stdout.is_empty());
    assert!(stderr.contains("Unknown argument."));
    let (watch_code, _, watch_err, _) = run(&["watch"], &cwd, &env, ready(Ok(report())));
    assert_eq!(watch_code, 1);
    assert!(watch_err.contains("Unknown argument."));
    let (tui_code, _, tui_err, _) = run(&["tui"], &cwd, &env, ready(Ok(report())));
    assert_eq!(tui_code, 1);
    assert!(tui_err.contains("Open a terminal"));
    fs::remove_dir_all(&cwd).unwrap();
}

#[test]
fn ls_refuses_a_file_as_state_dir() {
    let cwd = temp("ls-file-state");
    let file = cwd.join("not-dir");
    fs::write(&file, "x").unwrap();
    fs::write(
        cwd.join("bistill.conf"),
        format!("state_dir = {}\n", file.display()),
    )
    .unwrap();
    let mut session = poller(&cwd, Vec::new());
    let (code, stderr) = run_watch(&cwd, &mut session, false);
    assert_eq!(code, 1, "{stderr}");
    assert!(!stderr.is_empty());
    fs::remove_dir_all(&cwd).unwrap();
}

#[cfg(unix)]
#[test]
fn ls_refuses_a_read_only_state_dir() {
    use std::os::unix::fs::PermissionsExt;
    let cwd = temp("ls-ro-state");
    let dir = cwd.join("state");
    fs::create_dir(&dir).unwrap();
    fs::write(
        cwd.join("bistill.conf"),
        format!("state_dir = {}\n", dir.display()),
    )
    .unwrap();
    fs::set_permissions(&dir, fs::Permissions::from_mode(0o555)).unwrap();
    let mut session = poller(&cwd, Vec::new());
    let (code, stderr) = run_watch(&cwd, &mut session, false);
    fs::set_permissions(&dir, fs::Permissions::from_mode(0o700)).unwrap();
    assert_eq!(code, 1, "{stderr}");
    fs::remove_dir_all(&cwd).unwrap();
}

#[test]
fn tui_request_reads_the_bare_command_and_the_subcommand() {
    let bare = match tui_request(&args(&[])) {
        Some(Ok(tui)) => tui,
        _ => panic!("bare"),
    };
    assert!(!bare.verbose);
    let tui = match tui_request(&args(&[
        "tui",
        "--verbose",
        "--url",
        "https://git.example.invalid",
    ])) {
        Some(Ok(tui)) => tui,
        _ => panic!("tui"),
    };
    assert!(tui.verbose);
    assert_eq!(
        tui.flags.base_url.as_deref(),
        Some("https://git.example.invalid")
    );
    let err = match tui_request(&args(&["tui", "--nope"])) {
        Some(Err(err)) => err,
        Some(Ok(_)) => panic!("bad flag"),
        None => panic!("missing"),
    };
    assert!(err.contains("Unknown argument."));
    assert!(tui_request(&args(&["ping"])).is_none());
    assert!(tui_request(&args(&["tui", "--help"])).is_none());
}

#[test]
fn script_poll_publishes_or_returns_the_listed_error() {
    let cwd = temp("script-poll");
    let config = bistill_lib::load(&cwd, &bistill_lib::Flags::default(), &env_token()).unwrap();
    let client = Client::new(curl_bin::PROGRAM, &config);
    let mut script = Script {
        version: None,
        report: None,
        listed: Some(Ok(Listed {
            snapshot: sample_snapshot(Vec::new(), Vec::new()),
            requests: Vec::new(),
        })),
        origin: String::new(),
    };
    let mut published = false;
    match script.poll(&client, 0, &[], &mut |_| {
        published = true;
        Ok(())
    }) {
        Ok(_) => assert!(published),
        Err(_) => panic!("publish"),
    }
    script.listed = Some(Err(Error::Http(401)));
    match script.poll(&client, 0, &[], &mut |_| Ok(())) {
        Err(err) => assert!(matches!(err.error, Error::Http(401))),
        Ok(_) => panic!("http"),
    }
    script.listed = Some(Ok(Listed {
        snapshot: sample_snapshot(Vec::new(), Vec::new()),
        requests: Vec::new(),
    }));
    match script.poll(&client, 0, &[], &mut |_| {
        Err(Error::Io(std::io::Error::other("disk")))
    }) {
        Err(err) => assert!(matches!(err.error, Error::Io(_))),
        Ok(_) => panic!("io"),
    }
    fs::remove_dir_all(&cwd).unwrap();
}

#[test]
fn unix_ms_is_zero_before_the_epoch() {
    assert_eq!(unix_ms(std::time::SystemTime::UNIX_EPOCH), 0);
    let before = std::time::SystemTime::UNIX_EPOCH
        .checked_sub(Duration::from_secs(1))
        .unwrap();
    assert_eq!(unix_ms(before), 0);
}

#[test]
fn live_list_reads_http_status() {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
    let port = listener.local_addr().expect("addr").port();
    listener.set_nonblocking(true).expect("nonblocking");
    let thread = thread::spawn(move || {
        let deadline = Instant::now() + Duration::from_secs(5);
        while Instant::now() < deadline {
            match listener.accept() {
                Ok((mut sock, _)) => {
                    let _ = sock.set_read_timeout(Some(Duration::from_secs(2)));
                    let mut buf = [0u8; 1024];
                    let _ = sock.read(&mut buf);
                    let head = "HTTP/1.1 500 ERR\r\nContent-Length: 2\r\nConnection: close\r\n\r\n";
                    let _ = sock.write_all(head.as_bytes());
                    let _ = sock.write_all(b"no");
                    break;
                }
                Err(err) if err.kind() == std::io::ErrorKind::WouldBlock => {
                    thread::sleep(Duration::from_millis(10));
                }
                Err(_) => break,
            }
        }
    });
    let cwd = temp("live-list");
    let mut env = env_token();
    env.base_url = Some(format!("http://127.0.0.1:{port}"));
    let config = bistill_lib::load(&cwd, &bistill_lib::Flags::default(), &env).unwrap();
    let client = Client::new(curl_bin::PROGRAM, &config);
    let err = match Live.poll(&client, 0, &[], &mut |_| Ok(())) {
        Err(err) => err.error,
        Ok(_) => panic!("expected HTTP 500"),
    };
    assert!(matches!(err, Error::Http(500)), "{err}");
    thread.join().expect("server");
    fs::remove_dir_all(&cwd).unwrap();
}

#[test]
fn ls_refuses_a_corrupt_snapshot() {
    let cwd = temp("corrupt-snapshot");
    fs::write(cwd.join("snapshot.json"), "{").unwrap();
    let mut session = poller(&cwd, Vec::new());
    let (code, stderr) = run_watch(&cwd, &mut session, false);
    assert_eq!(code, 5, "{stderr}");
    fs::remove_dir_all(&cwd).unwrap();
}

const SCREEN_NOW: u64 = 1_700_000_000_000;

fn screen_clock(offset_secs: i32) -> screen::Clock {
    screen::Clock {
        now_ms: SCREEN_NOW,
        offset_secs,
    }
}

fn draw_screen(
    screen: &mut screen::Screen,
    width: u16,
    height: u16,
    snapshot: Option<&Snapshot>,
    role: &screen::Role,
    offset_secs: i32,
) -> tui::Buffer {
    let mut buffer = tui::Buffer::empty(width, height);
    screen::draw(
        screen,
        &mut buffer,
        snapshot,
        role,
        screen_clock(offset_secs),
    );
    buffer
}

fn glyph_char(glyph: &tui::Glyph) -> char {
    match glyph {
        tui::Glyph::Single(text) | tui::Glyph::Wide(text) => text.chars().next().unwrap_or(' '),
        tui::Glyph::Tail => ' ',
    }
}

fn find_style(buffer: &tui::Buffer, word: &str) -> tui::Style {
    style_of(buffer, word).into_iter().next().unwrap()
}

fn style_of(buffer: &tui::Buffer, word: &str) -> Vec<tui::Style> {
    let mut found = Vec::new();
    for y in 0..buffer.height() {
        let text = row_text(buffer, y);
        let mut start = 0usize;
        while let Some(at) = text[start..].find(word) {
            let column = text[..start + at].chars().count();
            let x = u16::try_from(column).unwrap();
            found.push(buffer.get(x, y).unwrap().style);
            start += at + word.len();
        }
    }
    found
}

fn row_text(buffer: &tui::Buffer, y: u16) -> String {
    (0..buffer.width())
        .map(|x| glyph_char(&buffer.get(x, y).unwrap().glyph))
        .collect()
}

fn grid_text(buffer: &tui::Buffer) -> String {
    (0..buffer.height())
        .map(|y| row_text(buffer, y))
        .collect::<Vec<_>>()
        .join("\n")
}

fn side_text(buffer: &tui::Buffer, from: u16) -> String {
    let mut lines = Vec::new();
    for y in 0..buffer.height() {
        let mut line = String::new();
        for x in from..buffer.width() {
            line.push(glyph_char(&buffer.get(x, y).unwrap().glyph));
        }
        lines.push(line);
    }
    lines.join("\n")
}

fn row_of(buffer: &tui::Buffer, needle: &str) -> Option<u16> {
    (0..buffer.height()).find(|&y| row_text(buffer, y).contains(needle))
}

fn press_at(column: u16, row: u16) -> tui::Event {
    tui::Event::Press {
        button: tui::MouseButton::Left,
        column,
        row,
    }
}

fn key(code: tui::KeyCode) -> tui::Event {
    tui::Event::Key(code)
}

fn act(
    screen: &mut screen::Screen,
    event: tui::Event,
    snapshot: Option<&Snapshot>,
    now_ms: u64,
) -> screen::Action {
    screen::handle(screen, event, snapshot, now_ms)
}

fn reviewer(name: &str, status: bistill_lib::ReviewStatus) -> bistill_lib::Reviewer {
    bistill_lib::Reviewer {
        name: name.to_owned(),
        slug: name.to_ascii_lowercase(),
        status,
    }
}

fn inbox_rows() -> (Vec<Row>, Vec<Row>) {
    let mut fix = sample_row("PRJ", "repo", 12, "Fix the pipe", false, false, false);
    fix.updated_ms = SCREEN_NOW - 45_000;
    fix.author = "Pat".to_owned();
    fix.reviewers = (0..24)
        .map(|index| {
            let status = match index % 3 {
                0 => bistill_lib::ReviewStatus::Unapproved,
                1 => bistill_lib::ReviewStatus::NeedsWork,
                _ => bistill_lib::ReviewStatus::Approved,
            };
            reviewer(&format!("R{index}"), status)
        })
        .collect();
    fix.unanswered_as_author = 2;
    fix.unanswered_as_reviewer = 3;
    fix.open_tasks = 4;
    fix.build = bistill_lib::Build::Failed;
    fix.conflicted = true;
    fix.can_merge = true;

    let mut draft = sample_row("PRJ", "widget", 13, "Draft note", true, false, false);
    draft.updated_ms = SCREEN_NOW - 180_000;
    draft.author = "Sam".to_owned();

    let mut ship = sample_row("PRJ", "repo", 15, "Ship the valve", false, false, false);
    ship.updated_ms = SCREEN_NOW - 2 * 3_600_000;
    ship.build = bistill_lib::Build::InProgress;

    let mut quiet = sample_row("PRJ", "repo", 16, "Quiet change", false, false, false);
    quiet.updated_ms = SCREEN_NOW - 5 * 86_400_000;

    let mut pending = sample_row("PRJ", "repo", 17, "Pending row", false, false, false);
    pending.updated_ms = SCREEN_NOW;
    pending.author = "Quinn".to_owned();
    pending.enrichment = bistill_lib::Enrichment::Pending;
    pending.build = bistill_lib::Build::Failed;
    pending.conflicted = true;

    let mut marked = sample_row("PRJ", "repo", 18, "Mark conflicts", false, false, false);
    marked.updated_ms = SCREEN_NOW;
    marked.build = bistill_lib::Build::Successful;
    marked.conflicted = true;

    let mut stale = sample_row("PRJ", "pipe", 21, "Waiting on Rio", false, true, false);
    stale.author = "Rio".to_owned();
    let mut work = sample_row("PRJ", "mine", 22, "Personal repo", false, false, true);
    work.author = "Ada".to_owned();

    (
        vec![fix, draft, ship, quiet, pending, marked],
        vec![stale, work],
    )
}

fn inbox() -> Snapshot {
    let (needs, waiting) = inbox_rows();
    sample_snapshot(needs, waiting)
}

#[test]
fn screen_draws_both_layouts_and_status_lines() {
    let _ = screen::Screen::default();
    let mut snapshot = inbox();
    snapshot.truncated = 3;
    let holder = screen::Role::Holder(screen::Phase::Fetching);
    let mut screen = screen::Screen::new();
    let wide = draw_screen(&mut screen, 160, 24, Some(&snapshot), &holder, 0);
    let top = row_text(&wide, 0);
    assert!(top.contains("Needs review"));
    assert!(top.contains("Waiting"));
    assert!(row_text(&wide, 1).contains("repo#12"));
    let shown = grid_text(&wide);
    assert!(shown.contains("and 3 more"));
    assert!(shown.contains("No activity yet."));
    assert!(shown.contains("PRJ/repo#12"));
    assert!(shown.contains("Fix the pipe"));
    assert!(shown.contains("45s"));
    assert!(shown.contains("3m"));
    assert!(shown.contains("2h"));
    assert!(shown.contains("5d"));
    assert!(shown.contains("0s"));
    assert!(shown.contains("failed"));
    assert!(shown.contains("draft"));
    assert!(shown.contains("successful"));
    assert!(shown.contains("in progress"));
    assert!(shown.contains("1/6"));
    assert!(row_text(&wide, 23).contains("Fetching from Bitbucket..."));
    let picked = wide.get(1, 2).unwrap().style;
    assert_eq!(picked.fg, Some(tui::Color::BrightWhite));
    assert_eq!(picked.bg, Some(tui::Color::Blue));
    assert!(!picked.reverse);
    assert_eq!(find_style(&wide, "draft").fg, Some(tui::Color::Magenta));
    assert_eq!(find_style(&wide, "in progress").fg, Some(tui::Color::Cyan));
    assert_eq!(
        find_style(&wide, "Waiting").fg,
        Some(tui::Color::BrightBlack)
    );
    assert_eq!(
        find_style(&wide, "PRJ/widget#13").fg,
        Some(tui::Color::BrightBlack)
    );
    assert_eq!(find_style(&wide, "1/6").fg, Some(tui::Color::BrightBlack));
    assert_eq!(
        find_style(&wide, "and 3 more").fg,
        Some(tui::Color::BrightYellow)
    );
    assert!(find_style(&wide, "and 3 more").bold);
    assert_eq!(wide.get(0, 23).unwrap().style.fg, Some(tui::Color::Cyan));
    let keys_at = u16::try_from(row_text(&wide, 23).find("j/k").unwrap()).unwrap();
    assert!(wide.get(keys_at, 23).unwrap().style.fg.is_none());
    assert!(wide.get(keys_at, 23).unwrap().style.bold);
    let mut probe = screen::Screen::new();
    let _ = draw_screen(&mut probe, 160, 24, Some(&snapshot), &holder, 0);
    assert_eq!(
        act(&mut probe, key(tui::KeyCode::Char('j')), Some(&snapshot), 0),
        screen::Action::None
    );
    let shifted = draw_screen(&mut probe, 160, 24, Some(&snapshot), &holder, 0);
    assert!(row_text(&shifted, 1).contains("widget#13"));
    assert!(side_text(&shifted, 80).contains("No activity yet."));
    assert!(
        style_of(&shifted, "failed")
            .iter()
            .any(|style| { style.fg == Some(tui::Color::BrightRed) && style.bold })
    );
    let mut ignored = std::collections::BTreeSet::new();
    ignored.insert("PRJ/repo/16".to_owned());
    probe.set_marks(std::collections::BTreeMap::new(), ignored);
    let badged = draw_screen(&mut probe, 160, 24, Some(&snapshot), &holder, 0);
    assert_eq!(
        find_style(&badged, "ignored").fg,
        Some(tui::Color::BrightYellow)
    );
    assert_eq!(
        act(&mut probe, key(tui::KeyCode::Tab), Some(&snapshot), 0),
        screen::Action::None
    );
    let _ = draw_screen(&mut probe, 160, 24, Some(&snapshot), &holder, 0);
    assert_eq!(
        act(&mut probe, key(tui::KeyCode::Char('j')), Some(&snapshot), 0),
        screen::Action::None
    );
    let waiting = draw_screen(&mut probe, 160, 24, Some(&snapshot), &holder, 0);
    assert_eq!(
        find_style(&waiting, "stale").fg,
        Some(tui::Color::BrightYellow)
    );
    assert_eq!(
        act(&mut probe, key(tui::KeyCode::Char('k')), Some(&snapshot), 0),
        screen::Action::None
    );
    let needs_work = draw_screen(&mut probe, 160, 24, Some(&snapshot), &holder, 0);
    assert!(
        style_of(&needs_work, "needs work")
            .iter()
            .any(|style| { style.fg == Some(tui::Color::BrightRed) })
    );
    let mut ship = screen::Screen::new();
    let _ = draw_screen(&mut ship, 160, 24, Some(&snapshot), &holder, 0);
    for _ in 0..2 {
        assert_eq!(
            act(&mut ship, key(tui::KeyCode::Char('j')), Some(&snapshot), 0),
            screen::Action::None
        );
    }
    let building = draw_screen(&mut ship, 160, 24, Some(&snapshot), &holder, 0);
    assert!(row_text(&building, 1).contains("repo#15"));
    assert!(side_text(&building, 80).contains("No activity yet."));
    for _ in 0..3 {
        assert_eq!(
            act(&mut ship, key(tui::KeyCode::Char('j')), Some(&snapshot), 0),
            screen::Action::None
        );
    }
    let merged = draw_screen(&mut ship, 160, 24, Some(&snapshot), &holder, 0);
    assert!(row_text(&merged, 1).contains("repo#18"));

    let again = draw_screen(&mut screen, 160, 24, Some(&snapshot), &holder, 0);
    assert!(grid_text(&again).contains("1/6"));

    let stacked = draw_screen(
        &mut screen::Screen::new(),
        80,
        24,
        Some(&snapshot),
        &holder,
        0,
    );
    let detail = row_of(&stacked, "No activity yet.").unwrap();
    assert!(detail > 1);
    assert!(!row_text(&stacked, 1).contains("No activity yet."));

    let ready = screen::Role::Holder(screen::Phase::Ready);
    let ready_grid = draw_screen(
        &mut screen::Screen::new(),
        160,
        24,
        Some(&snapshot),
        &ready,
        0,
    );
    assert!(row_text(&ready_grid, 23).contains("j/k move"));

    let auth = screen::Role::Holder(screen::Phase::Auth);
    let auth_grid = draw_screen(
        &mut screen::Screen::new(),
        160,
        24,
        Some(&snapshot),
        &auth,
        0,
    );
    assert!(row_text(&auth_grid, 23).contains("Token rejected."));
    assert_eq!(
        auth_grid.get(0, 23).unwrap().style.fg,
        Some(tui::Color::BrightRed)
    );

    let tls = screen::Role::Holder(screen::Phase::Tls);
    let tls_grid = draw_screen(
        &mut screen::Screen::new(),
        160,
        24,
        Some(&snapshot),
        &tls,
        0,
    );
    assert!(row_text(&tls_grid, 23).contains("curl failed TLS."));

    let down = screen::Role::Holder(screen::Phase::Unreachable {
        since_ms: 1_000_000_000_000,
    });
    let down_grid = draw_screen(
        &mut screen::Screen::new(),
        160,
        24,
        Some(&snapshot),
        &down,
        0,
    );
    assert!(row_text(&down_grid, 23).contains("Bitbucket unreachable (since 2001-09-09 01:46)."));

    let limited = screen::Role::Holder(screen::Phase::RateLimited);
    let limited_grid = draw_screen(
        &mut screen::Screen::new(),
        160,
        24,
        Some(&snapshot),
        &limited,
        0,
    );
    assert!(row_text(&limited_grid, 23).contains("Rate limited."));
    assert_eq!(
        limited_grid.get(0, 23).unwrap().style.fg,
        Some(tui::Color::BrightYellow)
    );

    let loading = draw_screen(&mut screen::Screen::new(), 160, 24, None, &holder, 0);
    let loading_text = grid_text(&loading);
    assert!(row_text(&loading, 23).contains("Fetching from Bitbucket..."));
    assert!(loading_text.contains("0/0"));
    assert!(!loading_text.contains("Nothing needs your attention."));

    let empty = sample_snapshot(Vec::new(), Vec::new());
    let empty_grid = draw_screen(&mut screen::Screen::new(), 160, 24, Some(&empty), &ready, 0);
    assert!(grid_text(&empty_grid).contains("Nothing needs your attention."));
    assert_eq!(
        find_style(&empty_grid, "Nothing").fg,
        Some(tui::Color::Green)
    );

    let mut tiny = screen::Screen::new();
    let _ = draw_screen(&mut tiny, 0, 4, Some(&snapshot), &holder, 0);
    let _ = draw_screen(&mut tiny, 4, 0, Some(&snapshot), &holder, 0);
    let short = draw_screen(&mut tiny, 40, 2, Some(&snapshot), &holder, 0);
    assert!(!grid_text(&short).contains("Waiting"));
    assert_eq!(
        act(&mut tiny, press_at(0, 1), Some(&snapshot), 0),
        screen::Action::None
    );

    for width in [20, 40, 100] {
        let _ = draw_screen(
            &mut screen::Screen::new(),
            width,
            12,
            Some(&snapshot),
            &holder,
            0,
        );
    }
}

#[test]
fn screen_keys_filter_help_and_mouse() {
    let snapshot = inbox();
    let ready = screen::Role::Holder(screen::Phase::Ready);
    let mut screen = screen::Screen::new();
    assert_eq!(
        act(
            &mut screen,
            key(tui::KeyCode::Char('j')),
            Some(&snapshot),
            0
        ),
        screen::Action::None
    );
    let grid = draw_screen(&mut screen, 160, 24, Some(&snapshot), &ready, 0);
    assert!(grid_text(&grid).contains("1/6"));
    assert!(wide_cell_is_bold(&grid));

    assert_eq!(
        act(
            &mut screen,
            key(tui::KeyCode::Char('k')),
            Some(&snapshot),
            0
        ),
        screen::Action::None
    );
    assert_eq!(
        act(&mut screen, key(tui::KeyCode::Down), Some(&snapshot), 0),
        screen::Action::None
    );
    assert_eq!(
        act(&mut screen, key(tui::KeyCode::Up), Some(&snapshot), 0),
        screen::Action::None
    );
    let back = draw_screen(&mut screen, 160, 24, Some(&snapshot), &ready, 0);
    assert!(grid_text(&back).contains("1/6"));

    let _ = act(
        &mut screen,
        key(tui::KeyCode::Char('j')),
        Some(&snapshot),
        0,
    );
    let draft = draw_screen(&mut screen, 160, 24, Some(&snapshot), &ready, 0);
    assert!(grid_text(&draft).contains("2/6"));
    assert!(side_text(&draft, 80).contains("widget#13"));
    assert!(side_text(&draft, 80).contains("No activity yet."));
    assert!(!side_text(&draft, 80).contains("build "));

    let _ = act(
        &mut screen,
        key(tui::KeyCode::Char('j')),
        Some(&snapshot),
        0,
    );
    let ship = draw_screen(&mut screen, 160, 24, Some(&snapshot), &ready, 0);
    assert!(side_text(&ship, 80).contains("repo#15"));
    assert!(side_text(&ship, 80).contains("No activity yet."));

    let _ = act(
        &mut screen,
        key(tui::KeyCode::Char('j')),
        Some(&snapshot),
        0,
    );
    let quiet = draw_screen(&mut screen, 160, 24, Some(&snapshot), &ready, 0);
    assert!(side_text(&quiet, 80).contains("repo#16"));
    assert!(side_text(&quiet, 80).contains("No activity yet."));
    assert!(!side_text(&quiet, 80).contains("Quiet change"));
    assert!(!side_text(&quiet, 80).contains("unanswered"));

    let _ = act(
        &mut screen,
        key(tui::KeyCode::Char('j')),
        Some(&snapshot),
        0,
    );
    let pending = draw_screen(&mut screen, 160, 24, Some(&snapshot), &ready, 0);
    let pending_line = (0..pending.height())
        .map(|y| row_text(&pending, y))
        .find(|line| line.contains("PRJ/repo#17"))
        .unwrap();
    assert!(!pending_line.contains("failed"));
    assert!(!pending_line.contains("conflicted"));
    assert!(!side_text(&pending, 80).contains("unanswered"));
    assert!(!side_text(&pending, 80).contains("build "));

    let _ = act(
        &mut screen,
        key(tui::KeyCode::Char('j')),
        Some(&snapshot),
        0,
    );
    let marked = draw_screen(&mut screen, 160, 24, Some(&snapshot), &ready, 0);
    assert!(grid_text(&marked).contains("6/6"));
    assert!(side_text(&marked, 80).contains("repo#18"));
    assert!(side_text(&marked, 80).contains("No activity yet."));
    assert!(!side_text(&marked, 80).contains("successful"));
    assert!(!side_text(&marked, 80).contains("conflicted"));
    assert_eq!(
        act(&mut screen, key(tui::KeyCode::Enter), Some(&snapshot), 0),
        screen::Action::Open("https://git.example.invalid/pull/18".to_owned())
    );
    let _ = act(
        &mut screen,
        key(tui::KeyCode::Char('j')),
        Some(&snapshot),
        0,
    );

    let only = sample_snapshot(
        vec![sample_row("PRJ", "repo", 1, "Only", false, false, false)],
        Vec::new(),
    );
    let clamped = draw_screen(&mut screen, 160, 24, Some(&only), &ready, 0);
    assert!(grid_text(&clamped).contains("1/1"));
    let empty = sample_snapshot(Vec::new(), Vec::new());
    assert_eq!(
        act(&mut screen, key(tui::KeyCode::Enter), Some(&empty), 0),
        screen::Action::None
    );
    let cleared = draw_screen(&mut screen, 160, 24, Some(&empty), &ready, 0);
    assert!(grid_text(&cleared).contains("Nothing needs your attention."));
    assert_eq!(
        act(&mut screen, key(tui::KeyCode::Char('j')), Some(&empty), 0),
        screen::Action::None
    );
    assert_eq!(
        act(&mut screen, key(tui::KeyCode::Enter), None, 0),
        screen::Action::None
    );

    let mut screen = screen::Screen::new();
    let _ = draw_screen(&mut screen, 160, 24, Some(&snapshot), &ready, 0);
    assert_eq!(
        act(
            &mut screen,
            key(tui::KeyCode::Char('/')),
            Some(&snapshot),
            0
        ),
        screen::Action::None
    );
    assert_eq!(
        act(
            &mut screen,
            key(tui::KeyCode::Backspace),
            Some(&snapshot),
            0
        ),
        screen::Action::None
    );
    assert_eq!(
        act(&mut screen, key(tui::KeyCode::Left), Some(&snapshot), 0),
        screen::Action::None
    );
    assert_eq!(
        act(&mut screen, press_at(1, 3), Some(&snapshot), 10),
        screen::Action::None
    );
    for ch in ['z', 'z', 'z'] {
        assert_eq!(
            act(&mut screen, key(tui::KeyCode::Char(ch)), Some(&snapshot), 0),
            screen::Action::None
        );
    }
    let editing = draw_screen(
        &mut screen,
        160,
        24,
        Some(&snapshot),
        &screen::Role::Holder(screen::Phase::Auth),
        0,
    );
    assert!(row_text(&editing, 23).contains("/zzz"));
    assert!(!row_text(&editing, 23).contains("Token rejected."));
    assert_eq!(
        act(&mut screen, key(tui::KeyCode::Esc), Some(&snapshot), 0),
        screen::Action::None
    );
    let restored = draw_screen(
        &mut screen,
        160,
        24,
        Some(&snapshot),
        &screen::Role::Holder(screen::Phase::Auth),
        0,
    );
    assert!(row_text(&restored, 23).contains("Token rejected."));
    assert!(grid_text(&restored).contains("1/6"));

    for _ in 0..8 {
        let _ = act(
            &mut screen,
            key(tui::KeyCode::Char('j')),
            Some(&snapshot),
            0,
        );
    }
    type_filter(&mut screen, "WIDGET", &snapshot);
    let filtered = draw_screen(&mut screen, 160, 24, Some(&snapshot), &ready, 0);
    assert!(grid_text(&filtered).contains("1/1"));
    assert!(grid_text(&filtered).contains("Draft note"));
    assert!(!grid_text(&filtered).contains("Fix the pipe"));

    assert_eq!(
        act(
            &mut screen,
            key(tui::KeyCode::Char('/')),
            Some(&snapshot),
            0
        ),
        screen::Action::None
    );
    let reused = draw_screen(&mut screen, 160, 24, Some(&snapshot), &ready, 0);
    assert!(row_text(&reused, 23).contains("/WIDGET"));
    assert_eq!(
        act(&mut screen, key(tui::KeyCode::Esc), Some(&snapshot), 0),
        screen::Action::None
    );

    type_filter(&mut screen, "sam", &snapshot);
    let by_author = draw_screen(&mut screen, 160, 24, Some(&snapshot), &ready, 0);
    assert!(grid_text(&by_author).contains("Draft note"));
    type_filter(&mut screen, "pipe", &snapshot);
    let by_repo = draw_screen(&mut screen, 160, 24, Some(&snapshot), &ready, 0);
    assert!(grid_text(&by_repo).contains("Fix the pipe"));
    assert_eq!(
        act(&mut screen, key(tui::KeyCode::Tab), Some(&snapshot), 0),
        screen::Action::None
    );
    let waiting = draw_screen(&mut screen, 160, 24, Some(&snapshot), &ready, 0);
    let waiting_text = grid_text(&waiting);
    assert!(waiting_text.contains("PRJ/pipe#21"));
    assert!(waiting_text.contains("stale"));
    assert!(waiting_text.contains("1/1"));

    type_filter(&mut screen, "zzz", &snapshot);
    let none = draw_screen(&mut screen, 160, 24, Some(&snapshot), &ready, 0);
    let none_text = grid_text(&none);
    assert!(none_text.contains("0/0"));
    assert!(!none_text.contains("Nothing needs your attention."));
    type_filter(&mut screen, "PRJ", &snapshot);
    let projects = draw_screen(&mut screen, 160, 24, Some(&snapshot), &ready, 0);
    assert!(grid_text(&projects).contains("0/0"));

    type_filter(&mut screen, "", &snapshot);
    let all = draw_screen(&mut screen, 160, 24, Some(&snapshot), &ready, 0);
    assert!(grid_text(&all).contains("1/2"));

    assert_eq!(
        act(
            &mut screen,
            key(tui::KeyCode::Char('?')),
            Some(&snapshot),
            0
        ),
        screen::Action::None
    );
    let help = draw_screen(&mut screen, 160, 24, Some(&snapshot), &ready, 0);
    assert!(grid_text(&help).contains("Quit"));
    assert_eq!(
        act(
            &mut screen,
            key(tui::KeyCode::Char('r')),
            Some(&snapshot),
            0
        ),
        screen::Action::Refresh
    );
    assert_eq!(
        act(
            &mut screen,
            key(tui::KeyCode::Char('j')),
            Some(&snapshot),
            0
        ),
        screen::Action::None
    );
    assert_eq!(
        act(
            &mut screen,
            tui::Event::Release {
                button: tui::MouseButton::Left,
                column: 1,
                row: 2,
            },
            Some(&snapshot),
            0
        ),
        screen::Action::None
    );
    assert_eq!(
        act(&mut screen, press_at(1, 3), Some(&snapshot), 50),
        screen::Action::None
    );
    let closed = draw_screen(&mut screen, 160, 24, Some(&snapshot), &ready, 0);
    assert!(!grid_text(&closed).contains("Quit"));
    assert_eq!(
        act(
            &mut screen,
            key(tui::KeyCode::Char('?')),
            Some(&snapshot),
            0
        ),
        screen::Action::None
    );
    assert_eq!(
        act(
            &mut screen,
            key(tui::KeyCode::Char('?')),
            Some(&snapshot),
            0
        ),
        screen::Action::None
    );
    assert_eq!(
        act(
            &mut screen,
            key(tui::KeyCode::Char('?')),
            Some(&snapshot),
            0
        ),
        screen::Action::None
    );
    assert_eq!(
        act(&mut screen, key(tui::KeyCode::Esc), Some(&snapshot), 0),
        screen::Action::None
    );

    assert_eq!(
        act(
            &mut screen,
            key(tui::KeyCode::Char('J')),
            Some(&snapshot),
            0
        ),
        screen::Action::None
    );
    assert_eq!(
        act(
            &mut screen,
            tui::Event::Resize {
                width: 80,
                height: 24
            },
            Some(&snapshot),
            0
        ),
        screen::Action::None
    );
    assert_eq!(
        act(
            &mut screen,
            key(tui::KeyCode::Char('r')),
            Some(&snapshot),
            0
        ),
        screen::Action::Refresh
    );
    assert_eq!(
        act(
            &mut screen,
            key(tui::KeyCode::Char('q')),
            Some(&snapshot),
            0
        ),
        screen::Action::Quit
    );

    let mut screen = screen::Screen::new();
    let _ = draw_screen(&mut screen, 160, 24, Some(&snapshot), &ready, 0);
    assert_eq!(
        act(&mut screen, press_at(1, 2), Some(&snapshot), 1_000),
        screen::Action::None
    );
    assert_eq!(
        act(
            &mut screen,
            tui::Event::Press {
                button: tui::MouseButton::Middle,
                column: 1,
                row: 2,
            },
            Some(&snapshot),
            1_400
        ),
        screen::Action::Open("https://git.example.invalid/pull/12".to_owned())
    );
    assert_eq!(
        act(&mut screen, press_at(1, 2), Some(&snapshot), 2_000),
        screen::Action::None
    );
    assert_eq!(
        act(&mut screen, press_at(1, 2), Some(&snapshot), 2_401),
        screen::Action::None
    );
    assert_eq!(
        act(&mut screen, press_at(1, 2), Some(&snapshot), 3_000),
        screen::Action::None
    );
    assert_eq!(
        act(&mut screen, press_at(1, 3), Some(&snapshot), 3_200),
        screen::Action::None
    );
    assert_eq!(
        act(&mut screen, press_at(1, 21), Some(&snapshot), 3_300),
        screen::Action::None
    );
    assert_eq!(
        act(&mut screen, press_at(81, 4), Some(&snapshot), 3_400),
        screen::Action::None
    );
    let focused = draw_screen(&mut screen, 160, 24, Some(&snapshot), &ready, 0);
    assert!(focused.get(81, 1).unwrap().style.bold);
    assert!(!focused.get(1, 1).unwrap().style.bold);
    assert_eq!(
        act(&mut screen, press_at(0, 23), Some(&snapshot), 3_500),
        screen::Action::None
    );
    assert_eq!(
        act(&mut screen, press_at(200, 4), Some(&snapshot), 3_600),
        screen::Action::None
    );
    assert_eq!(
        act(&mut screen, press_at(14, 0), Some(&snapshot), 3_700),
        screen::Action::None
    );
    let waiting = draw_screen(&mut screen, 160, 24, Some(&snapshot), &ready, 0);
    assert!(grid_text(&waiting).contains("stale"));
    assert!(grid_text(&waiting).contains("needs work"));
    assert_eq!(
        act(&mut screen, press_at(1, 0), Some(&snapshot), 3_800),
        screen::Action::None
    );
    let needs = draw_screen(&mut screen, 160, 24, Some(&snapshot), &ready, 0);
    assert!(grid_text(&needs).contains("Needs review"));
    assert_eq!(
        act(&mut screen, key(tui::KeyCode::Tab), Some(&snapshot), 0),
        screen::Action::None
    );
    assert_eq!(
        act(&mut screen, key(tui::KeyCode::Tab), Some(&snapshot), 0),
        screen::Action::None
    );
}

fn wide_cell_is_bold(buffer: &tui::Buffer) -> bool {
    buffer.get(1, 1).unwrap().style.bold
}

fn type_filter(screen: &mut screen::Screen, text: &str, snapshot: &Snapshot) {
    assert_eq!(
        act(screen, key(tui::KeyCode::Char('/')), Some(snapshot), 0),
        screen::Action::None
    );
    for _ in 0..32 {
        assert_eq!(
            act(screen, key(tui::KeyCode::Backspace), Some(snapshot), 0),
            screen::Action::None
        );
    }
    for ch in text.chars() {
        assert_eq!(
            act(screen, key(tui::KeyCode::Char(ch)), Some(snapshot), 0),
            screen::Action::None
        );
    }
    assert_eq!(
        act(screen, key(tui::KeyCode::Enter), Some(snapshot), 0),
        screen::Action::None
    );
}

#[test]
fn screen_scrolls_the_focused_pane() {
    let snapshot = inbox();
    let ready = screen::Role::Holder(screen::Phase::Ready);
    let mut screen = screen::Screen::new();
    let _ = draw_screen(&mut screen, 160, 24, Some(&snapshot), &ready, 0);
    assert_eq!(
        act(
            &mut screen,
            tui::Event::Wheel {
                direction: tui::Wheel::Down,
                column: 1,
                row: 2
            },
            Some(&snapshot),
            0
        ),
        screen::Action::None
    );
    assert_eq!(
        act(
            &mut screen,
            tui::Event::Wheel {
                direction: tui::Wheel::Up,
                column: 1,
                row: 2
            },
            Some(&snapshot),
            0
        ),
        screen::Action::None
    );
    let stayed = draw_screen(&mut screen, 160, 24, Some(&snapshot), &ready, 0);
    assert!(side_text(&stayed, 0).contains("PRJ/repo#12"));

    let mut short = screen::Screen::new();
    let _ = draw_screen(&mut short, 160, 8, Some(&snapshot), &ready, 0);
    for _ in 0..3 {
        let _ = act(
            &mut short,
            tui::Event::Wheel {
                direction: tui::Wheel::Down,
                column: 1,
                row: 2,
            },
            Some(&snapshot),
            0,
        );
    }
    let scrolled = draw_screen(&mut short, 160, 8, Some(&snapshot), &ready, 0);
    assert!(table_has(&scrolled, "PRJ/repo#16"));
    assert!(!table_has(&scrolled, "PRJ/repo#12"));
    let _ = act(
        &mut short,
        tui::Event::Wheel {
            direction: tui::Wheel::Down,
            column: 1,
            row: 2,
        },
        Some(&snapshot),
        0,
    );
    let held = draw_screen(&mut short, 160, 8, Some(&snapshot), &ready, 0);
    assert!(!table_has(&held, "PRJ/repo#12"));
    for _ in 0..4 {
        let _ = act(
            &mut short,
            tui::Event::Wheel {
                direction: tui::Wheel::Up,
                column: 1,
                row: 2,
            },
            Some(&snapshot),
            0,
        );
    }
    let top = draw_screen(&mut short, 160, 8, Some(&snapshot), &ready, 0);
    assert!(table_has(&top, "PRJ/repo#12"));

    let mut fed = sample_row("PRJ", "repo", 12, "Fix the pipe", false, false, false);
    fed.events = feed_events();
    let snapshot = sample_snapshot(vec![fed], Vec::new());
    let mut screen = screen::Screen::new();
    let _ = draw_screen(&mut screen, 160, 12, Some(&snapshot), &ready, 0);
    assert_eq!(
        act(&mut screen, press_at(81, 4), Some(&snapshot), 10),
        screen::Action::None
    );
    let head = draw_screen(&mut screen, 160, 12, Some(&snapshot), &ready, 0);
    let head_side = side_text(&head, 80);
    assert!(head_side.contains("You commented"));
    assert!(!head_side.contains("tail of the feed"));
    let _ = act(
        &mut screen,
        tui::Event::Wheel {
            direction: tui::Wheel::Down,
            column: 90,
            row: 6,
        },
        Some(&snapshot),
        0,
    );
    let stepped = draw_screen(&mut screen, 160, 12, Some(&snapshot), &ready, 0);
    assert_ne!(side_text(&stepped, 80), head_side);
    for _ in 0..40 {
        let _ = act(
            &mut screen,
            tui::Event::Wheel {
                direction: tui::Wheel::Down,
                column: 90,
                row: 6,
            },
            Some(&snapshot),
            0,
        );
    }
    let tail = draw_screen(&mut screen, 160, 12, Some(&snapshot), &ready, 0);
    assert!(side_text(&tail, 80).contains("tail of the feed"));
    assert!(!side_text(&tail, 80).contains("You commented"));
    let before = side_text(&tail, 80);
    let _ = act(
        &mut screen,
        tui::Event::Wheel {
            direction: tui::Wheel::Down,
            column: 90,
            row: 6,
        },
        Some(&snapshot),
        0,
    );
    let same = draw_screen(&mut screen, 160, 12, Some(&snapshot), &ready, 0);
    assert_eq!(side_text(&same, 80), before);
    for _ in 0..40 {
        let _ = act(
            &mut screen,
            tui::Event::Wheel {
                direction: tui::Wheel::Up,
                column: 90,
                row: 6,
            },
            Some(&snapshot),
            0,
        );
    }
    let back = draw_screen(&mut screen, 160, 12, Some(&snapshot), &ready, 0);
    assert!(side_text(&back, 80).contains("You commented"));
}

fn table_has(buffer: &tui::Buffer, needle: &str) -> bool {
    (0..buffer.height()).any(|y| {
        let mut line = String::new();
        for x in 0..80.min(buffer.width()) {
            line.push(glyph_char(&buffer.get(x, y).unwrap().glyph));
        }
        line.contains(needle)
    })
}

#[test]
fn screen_formats_clock_text() {
    let _ = format!("{:?}", screen::Action::None);
    let _ = format!("{:?}", screen::Action::Quit);
    let _ = format!("{:?}", screen::Action::Refresh);
    let _ = format!(
        "{:?}",
        screen::Action::Open("https://example.invalid".to_owned())
    );
    assert_eq!(screen::relative(SCREEN_NOW, SCREEN_NOW), "0s");
    assert_eq!(screen::relative(SCREEN_NOW - 59_000, SCREEN_NOW), "59s");
    assert_eq!(screen::relative(SCREEN_NOW - 60_000, SCREEN_NOW), "1m");
    assert_eq!(screen::relative(SCREEN_NOW - 3_599_000, SCREEN_NOW), "59m");
    assert_eq!(screen::relative(SCREEN_NOW - 3_600_000, SCREEN_NOW), "1h");
    assert_eq!(screen::relative(SCREEN_NOW - 86_399_000, SCREEN_NOW), "23h");
    assert_eq!(screen::relative(SCREEN_NOW - 86_400_000, SCREEN_NOW), "1d");
    assert_eq!(screen::relative(SCREEN_NOW + 1_000, SCREEN_NOW), "0s");
    assert_eq!(screen::absolute(0, 0), "1970-01-01 00:00");
    assert_eq!(screen::absolute(0, -60), "1970-01-01 00:00");
    assert_eq!(screen::absolute(0, 3_600), "1970-01-01 01:00");
    assert_eq!(screen::absolute(1_000_000_000_000, 0), "2001-09-09 01:46");
    assert_eq!(screen::ago(SCREEN_NOW, SCREEN_NOW), "just now");
    assert_eq!(screen::ago(SCREEN_NOW - 59_000, SCREEN_NOW), "just now");
    assert_eq!(screen::ago(SCREEN_NOW - 60_000, SCREEN_NOW), "1m ago");
    assert_eq!(
        screen::ago(SCREEN_NOW - 3 * 3_600_000, SCREEN_NOW),
        "3h ago"
    );
    assert_eq!(
        screen::ago(SCREEN_NOW - 5 * 86_400_000, SCREEN_NOW),
        "5d ago"
    );
    assert_eq!(screen::ago(SCREEN_NOW + 1_000, SCREEN_NOW), "just now");
}

fn activity(
    id: u64,
    age_ms: u64,
    slug: &str,
    name: &str,
    kind: bistill_lib::EventKind,
    text: &str,
) -> bistill_lib::Event {
    bistill_lib::Event {
        id,
        created_ms: SCREEN_NOW.saturating_sub(age_ms),
        actor_slug: slug.to_owned(),
        actor_name: name.to_owned(),
        kind,
        text: text.to_owned(),
        thread: Vec::new(),
        added_user: false,
    }
}

fn feed_events() -> Vec<bistill_lib::Event> {
    use bistill_lib::EventKind;
    vec![
        activity(
            10,
            30_000,
            "jcitizen",
            "Jane",
            EventKind::Commented,
            "Looks good",
        ),
        activity(
            9,
            3 * 3_600_000,
            "sam",
            "Sam",
            EventKind::Approved,
            "Ship it",
        ),
        activity(8, 0, "jcitizen", "Jane", EventKind::Approved, ""),
        activity(7, 120_000, "pat", "Pat", EventKind::Pushed, "valve"),
        activity(6, 0, "jcitizen", "Jane", EventKind::Pushed, ""),
        activity(5, 5 * 86_400_000, "pat", "Pat", EventKind::Reopened, ""),
        activity(4, 0, "jcitizen", "Jane", EventKind::Reopened, ""),
        activity(3, 0, "pat", "Pat", EventKind::Added, ""),
        activity(2, 0, "pat", "Pat", EventKind::Other, "noise"),
        activity(
            1,
            9 * 86_400_000,
            "pat",
            "Pat",
            EventKind::Commented,
            "tail of the feed",
        ),
    ]
}

#[test]
fn screen_keeps_the_pull_request_when_rows_move() {
    use bistill_lib::EventKind;
    let ready = screen::Role::Holder(screen::Phase::Ready);
    let fetching = screen::Role::Holder(screen::Phase::Fetching);
    let mut fix = sample_row("PRJ", "repo", 12, "Fix the pipe", false, false, false);
    fix.events = vec![activity(
        4,
        3 * 3_600_000,
        "sam",
        "Sam",
        EventKind::Approved,
        "Looks good",
    )];
    let mut draft = sample_row("PRJ", "widget", 13, "Draft note", true, false, false);
    draft.events = vec![
        activity(
            9,
            30_000,
            "pat",
            "Pat",
            EventKind::Commented,
            "Please look at the valve",
        ),
        activity(1, 0, "jcitizen", "Jane", EventKind::Commented, "mine"),
    ];
    let mut screen = screen::Screen::new();
    let mut floors = std::collections::BTreeMap::new();
    floors.insert(draft.id.clone(), 2);
    screen.set_marks(floors, std::collections::BTreeSet::new());
    let ordered = sample_snapshot(vec![fix.clone(), draft.clone()], Vec::new());
    let _ = draw_screen(&mut screen, 160, 24, Some(&ordered), &ready, 0);
    assert_eq!(
        act(&mut screen, key(tui::KeyCode::Char('j')), Some(&ordered), 0),
        screen::Action::None
    );
    let picked = draw_screen(&mut screen, 160, 24, Some(&ordered), &ready, 0);
    assert!(side_text(&picked, 80).contains("widget#13"));
    assert!(side_text(&picked, 80).contains("Comment from Pat"));
    assert!(side_text(&picked, 80).contains("Please look at the valve"));
    assert!(side_text(&picked, 80).contains("You commented"));
    assert!(side_text(&picked, 80).contains("new"));
    assert!(!side_text(&picked, 80).contains("Draft note"));
    assert!(!side_text(&picked, 80).contains("http"));
    assert_eq!(find_style(&picked, "new").fg, Some(tui::Color::Green));
    assert!(find_style(&picked, "new").bold);
    let swapped = sample_snapshot(vec![draft.clone(), fix.clone()], Vec::new());
    let stayed = draw_screen(&mut screen, 160, 24, Some(&swapped), &ready, 0);
    assert!(side_text(&stayed, 80).contains("widget#13"));
    assert!(side_text(&stayed, 80).contains("Comment from Pat"));
    assert_eq!(find_style(&stayed, "Draft note").bg, Some(tui::Color::Blue));
    let gone = sample_snapshot(vec![fix.clone()], Vec::new());
    let held = draw_screen(&mut screen, 160, 24, Some(&gone), &fetching, 0);
    assert!(side_text(&held, 80).contains("widget#13"));
    assert!(side_text(&held, 80).contains("Please look at the valve"));
    assert_ne!(find_style(&held, "Fix the pipe").bg, Some(tui::Color::Blue));
    let adopted = draw_screen(&mut screen, 160, 24, Some(&gone), &ready, 0);
    assert!(side_text(&adopted, 80).contains("repo#12"));
    assert!(side_text(&adopted, 80).contains("Approved by Sam"));
    assert!(side_text(&adopted, 80).contains("3h ago"));
    assert_eq!(
        find_style(&adopted, "Fix the pipe").bg,
        Some(tui::Color::Blue)
    );

    let narrow = rule_parts(true, "3h ago", 3);
    assert_eq!(narrow, vec!["new".to_owned()]);
    assert!(rule_parts(true, "3h ago", 0).is_empty());
    let dashes = rule_parts(false, "", 4);
    assert!(dashes.iter().any(|text| text.contains('─')));
    assert_eq!(
        screen::verb_line(
            &activity(1, 0, "x", "Pat", EventKind::Other, "noise"),
            "jcitizen"
        ),
        ""
    );
    assert!(screen::wrap_text("", 0).is_empty());
    assert_eq!(
        screen::wrap_text("ab", 1),
        vec!["a".to_owned(), "b".to_owned()]
    );
    assert_eq!(
        screen::wrap_text("a\n\nb", 10),
        vec!["a".to_owned(), String::new(), "b".to_owned()]
    );
    assert_eq!(
        screen::wrap_text("a字", 2),
        vec!["a".to_owned(), "字".to_owned()]
    );
    let mut buffer = tui::Buffer::empty(4, 2);
    screen::paint_feed(
        &mut buffer,
        tui::Rect {
            x: 0,
            y: 0,
            width: 0,
            height: 2,
        },
        &[],
    );
    screen::paint_feed(
        &mut buffer,
        tui::Rect {
            x: 0,
            y: 0,
            width: 4,
            height: 0,
        },
        &[],
    );
    let row = vec![
        screen::FeedSpan {
            text: "abcd".to_owned(),
            style: tui::Style::default(),
        },
        screen::FeedSpan {
            text: "z".to_owned(),
            style: tui::Style::default(),
        },
    ];
    screen::paint_feed(
        &mut buffer,
        tui::Rect {
            x: 0,
            y: 0,
            width: 2,
            height: 1,
        },
        &[row],
    );
}

fn rule_parts(unread: bool, time: &str, width: usize) -> Vec<String> {
    screen::rule_spans(unread, time, width)
        .into_iter()
        .map(|span| span.text)
        .collect()
}

fn watch_dir(name: &str) -> PathBuf {
    let cwd = temp(name);
    fs::write(
        cwd.join("bistill.conf"),
        format!("state_dir = {}\npoll_seconds = 15\n", cwd.display()),
    )
    .unwrap();
    cwd
}

struct Poller {
    steps: Vec<Result<Listed, bistill_lib::InboxFault>>,
    now: u64,
    jump: u64,
    pauses: Vec<u64>,
    stop_after_polls: u32,
    polls: u32,
    stop_after_pauses: Option<u32>,
    refresh: bool,
    quit: bool,
    readonly_on: Option<u32>,
    state: PathBuf,
    cached: Vec<usize>,
}

impl Session for Poller {
    fn version(&mut self, _: &str) -> Result<String, Error> {
        Err(Error::Auth("unused".to_owned()))
    }

    fn ping(&mut self, _: &Client) -> Result<Report, Error> {
        Err(Error::Auth("unused".to_owned()))
    }

    fn poll(
        &mut self,
        _: &Client,
        _: u64,
        cached: &[Row],
        publish: &mut dyn FnMut(&Snapshot) -> Result<(), Error>,
    ) -> Result<Listed, bistill_lib::InboxFault> {
        self.cached.push(cached.len());
        self.polls += 1;
        if self.readonly_on == Some(self.polls) {
            self.readonly_on = None;
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                let (path, mode) = if self.polls == 1 {
                    (self.state.clone(), 0o555)
                } else {
                    (self.state.join("snapshot.json"), 0o444)
                };
                fs::set_permissions(path, fs::Permissions::from_mode(mode)).unwrap();
            }
        }
        let result = self.steps.remove(0);
        if let Ok(listed) = &result {
            if let Err(error) = publish(&listed.snapshot) {
                return Err(bistill_lib::InboxFault {
                    error,
                    retry_after_ms: None,
                });
            }
        }
        result
    }

    fn now_ms(&mut self) -> u64 {
        self.now
    }

    fn pause(&mut self, duration: Duration, _wake: &std::sync::mpsc::Receiver<Nudge>) -> Paused {
        self.pauses.push(duration.as_millis() as u64);
        if self.quit {
            return Paused::Quit;
        }
        let wrote = self.refresh;
        if self.refresh {
            fs::write(self.state.join("refresh"), b"").unwrap();
            self.refresh = false;
        }
        self.now += self.jump;
        if wrote {
            Paused::Refresh
        } else {
            Paused::Elapsed
        }
    }

    fn again(&mut self) -> bool {
        if self
            .stop_after_pauses
            .is_some_and(|count| self.pauses.len() as u32 >= count)
        {
            return false;
        }
        self.polls < self.stop_after_polls
    }
}

fn poller(cwd: &Path, steps: Vec<Result<Listed, bistill_lib::InboxFault>>) -> Poller {
    Poller {
        steps,
        now: 1_000_000,
        jump: 20_000,
        pauses: Vec::new(),
        stop_after_polls: 1,
        polls: 0,
        stop_after_pauses: None,
        refresh: false,
        quit: false,
        readonly_on: None,
        state: cwd.to_owned(),
        cached: Vec::new(),
    }
}

fn listed_ok(rows: Vec<Row>) -> Result<Listed, bistill_lib::InboxFault> {
    Ok(Listed {
        snapshot: sample_snapshot(rows, Vec::new()),
        requests: vec![host::Request {
            program: "curl".to_owned(),
            url: "https://git.example.invalid/rest".to_owned(),
            headers: vec!["Authorization: Bearer secret-token".to_owned()],
            user_agent: "bistill/0.1.0 (internal)".to_owned(),
            timeout: Duration::from_secs(15),
            ca_file: None,
            fail_with_body: true,
        }],
    })
}

fn fault(status: u16, retry_after_ms: Option<u64>) -> Result<Listed, bistill_lib::InboxFault> {
    Err(bistill_lib::InboxFault {
        error: Error::Http(status),
        retry_after_ms,
    })
}

fn run_watch(cwd: &Path, poller: &mut Poller, verbose: bool) -> (i32, String) {
    let config = bistill_lib::load(cwd, &bistill_lib::Flags::default(), &env_token()).unwrap();
    let previous = match bistill_lib::read_snapshot(&config.state_dir) {
        Ok(previous) => previous,
        Err(err) => return (bistill_lib::exit_code(&err), explain(&err)),
    };
    let held = match lock::acquire(&config.state_dir) {
        Ok(lock::Acquire::Holder(held)) => held,
        Ok(lock::Acquire::Busy { pid }) => {
            return (1, format!("Another bistill is polling (pid {pid}).\n"));
        }
        Err(err) => return (1, explain(&Error::from(err))),
    };
    let board = std::sync::Mutex::new(watch::Board::new(previous));
    let (nudge_tx, nudge_rx) = std::sync::mpsc::channel();
    let (paint_tx, _paint_rx) = std::sync::mpsc::channel();
    let outcome = watch::poll(poller, &config, verbose, &board, &nudge_rx, &paint_tx);
    drop(nudge_tx);
    drop(held);
    (outcome.code, outcome.stderr)
}

#[test]
fn watch_polls_backs_off_and_releases_the_lock() {
    let cwd = watch_dir("watch-poll");
    let again = || sample_row("PRJ", "repo", 12, "Fix the pipe", false, false, false);
    let mut session = poller(
        &cwd,
        vec![listed_ok(vec![again()]), listed_ok(vec![again()])],
    );
    session.stop_after_polls = 2;
    let (code, stderr) = run_watch(&cwd, &mut session, true);
    assert_eq!(code, 0, "{stderr}");
    assert!(stderr.contains("Bearer ***"));
    assert!(!stderr.contains("secret-token"));
    assert_eq!(session.cached, vec![0, 1]);
    assert_eq!(session.pauses, vec![15_000]);
    assert!(!cwd.join("poll.lock").exists());
    let snapshot = bistill_lib::read_snapshot(&cwd).unwrap().unwrap();
    assert_eq!(snapshot.needs_review.len(), 1);

    let cwd = watch_dir("watch-auth");
    let row = sample_row("PRJ", "repo", 12, "Fix the pipe", false, false, false);
    let mut session = poller(
        &cwd,
        vec![listed_ok(vec![row]), fault(401, None), fault(401, None)],
    );
    session.stop_after_polls = 3;
    let (code, stderr) = run_watch(&cwd, &mut session, true);
    assert_eq!(code, 0, "{stderr}");
    assert!(stderr.contains("Token rejected."));
    let snapshot = bistill_lib::read_snapshot(&cwd).unwrap().unwrap();
    assert_eq!(snapshot.needs_review.len(), 1);
    assert_eq!(snapshot.status, bistill_lib::SnapshotStatus::Auth);
    assert_eq!(snapshot.status_since_ms, 1_020_000);

    let cwd = watch_dir("watch-double");
    let mut session = poller(
        &cwd,
        vec![fault(500, None), fault(500, None), fault(500, None)],
    );
    session.stop_after_polls = 3;
    let (code, _) = run_watch(&cwd, &mut session, true);
    assert_eq!(code, 0);
    assert_eq!(session.pauses, vec![15_000, 30_000, 10_000]);
    assert!(!cwd.join("snapshot.json").exists());

    let cwd = watch_dir("watch-refresh");
    let mut session = poller(&cwd, vec![fault(429, Some(120_000)), listed_ok(Vec::new())]);
    session.stop_after_polls = 2;
    session.jump = 200_000;
    session.refresh = true;
    let (code, stderr) = run_watch(&cwd, &mut session, true);
    assert_eq!(code, 0, "{stderr}");
    assert_eq!(session.pauses, vec![120_000]);
    assert!(!cwd.join("refresh").exists());
    let snapshot = bistill_lib::read_snapshot(&cwd).unwrap().unwrap();
    assert_eq!(snapshot.status, bistill_lib::SnapshotStatus::Ok);

    let cwd = watch_dir("watch-floor");
    let mut session = poller(&cwd, vec![fault(429, Some(15_000))]);
    session.stop_after_polls = 2;
    session.stop_after_pauses = Some(1);
    session.refresh = true;
    let (code, _) = run_watch(&cwd, &mut session, true);
    assert_eq!(code, 0);
    assert!(session.polls == 1);

    let cwd = watch_dir("watch-cap");
    let mut session = poller(
        &cwd,
        vec![
            fault(429, Some(700_000)),
            fault(401, None),
            listed_ok(Vec::new()),
        ],
    );
    session.stop_after_polls = 3;
    session.jump = 650_000;
    let (code, _) = run_watch(&cwd, &mut session, true);
    assert_eq!(code, 0);
    assert_eq!(session.pauses.len(), 2);

    let cwd = watch_dir("watch-quit");
    let mut session = poller(&cwd, vec![listed_ok(Vec::new())]);
    session.stop_after_polls = 2;
    session.quit = true;
    let (code, stderr) = run_watch(&cwd, &mut session, true);
    assert_eq!(code, 0, "{stderr}");
    assert_eq!(session.polls, 1);

    let cwd = watch_dir("watch-missing");
    let mut session = poller(
        &cwd,
        vec![Err(bistill_lib::InboxFault {
            error: Error::Curl(CurlFault::Missing {
                program: "curl".to_owned(),
            }),
            retry_after_ms: None,
        })],
    );
    let (code, stderr) = run_watch(&cwd, &mut session, true);
    assert_eq!(code, 2, "{stderr}");
    assert!(stderr.contains("curl must be on PATH."));
    assert!(!cwd.join("poll.lock").exists());

    for cwd in [
        "watch-poll",
        "watch-auth",
        "watch-double",
        "watch-refresh",
        "watch-floor",
        "watch-cap",
        "watch-missing",
        "watch-quit",
    ] {
        let _ = fs::remove_dir_all(
            std::env::temp_dir().join(format!("bistill-bin-{cwd}-{}", std::process::id())),
        );
    }
}

#[test]
fn watch_lock_names_a_live_pid_and_replaces_a_dead_one() {
    let cwd = watch_dir("watch-busy");
    fs::write(cwd.join("poll.lock"), format!("{}\n", std::process::id())).unwrap();
    let mut session = poller(&cwd, vec![listed_ok(Vec::new())]);
    let (code, stderr) = run_watch(&cwd, &mut session, true);
    assert_eq!(code, 1, "{stderr}");
    assert!(stderr.contains(&format!("pid {}).", std::process::id())));
    assert!(cwd.join("poll.lock").exists());

    let cwd = watch_dir("watch-dead");
    let mut child = std::process::Command::new("true").spawn().unwrap();
    let pid = child.id();
    child.wait().unwrap();
    assert!(!super::pid_os::pid_alive(pid));
    assert!(super::pid_os::pid_alive(std::process::id()));
    assert!(!super::pid_os::pid_alive(0));
    fs::write(cwd.join("poll.lock"), format!("{pid}\n")).unwrap();
    let mut session = poller(&cwd, vec![listed_ok(Vec::new())]);
    let (code, stderr) = run_watch(&cwd, &mut session, true);
    assert_eq!(code, 0, "{stderr}");
    assert!(!cwd.join("poll.lock").exists());

    let cwd = watch_dir("watch-garbage");
    fs::write(cwd.join("poll.lock"), "nope\n").unwrap();
    let held = match super::lock::acquire(&cwd).unwrap() {
        super::lock::Acquire::Holder(held) => held,
        super::lock::Acquire::Busy { pid } => panic!("busy {pid}"),
    };
    let text = fs::read_to_string(cwd.join("poll.lock")).unwrap();
    assert_eq!(text.trim(), std::process::id().to_string());
    drop(held);
    assert!(!cwd.join("poll.lock").exists());

    let missing = match super::lock::acquire(Path::new("/no/such/bistill-state")) {
        Err(err) => err,
        Ok(_) => panic!("missing dir"),
    };
    assert_eq!(missing.kind(), std::io::ErrorKind::NotFound);

    let cwd = watch_dir("watch-dir-lock");
    fs::create_dir(cwd.join("poll.lock")).unwrap();
    assert!(super::lock::acquire(&cwd).is_err());

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let cwd = watch_dir("watch-unreadable");
        fs::write(cwd.join("poll.lock"), "1\n").unwrap();
        fs::set_permissions(cwd.join("poll.lock"), fs::Permissions::from_mode(0o000)).unwrap();
        let held = match super::lock::acquire(&cwd).unwrap() {
            super::lock::Acquire::Holder(held) => held,
            super::lock::Acquire::Busy { .. } => panic!("busy"),
        };
        drop(held);
    }

    let cwd = watch_dir("watch-corrupt");
    fs::write(cwd.join("snapshot.json"), "{").unwrap();
    let mut session = poller(&cwd, vec![listed_ok(Vec::new())]);
    let (code, stderr) = run_watch(&cwd, &mut session, true);
    assert_eq!(code, 5, "{stderr}");
    assert!(!cwd.join("poll.lock").exists());

    let _ = fs::remove_dir_all(&cwd);
}

#[cfg(unix)]
#[test]
fn watch_returns_when_the_snapshot_cannot_be_written() {
    use std::os::unix::fs::PermissionsExt;
    let cwd = watch_dir("watch-write-ok");
    let mut session = poller(&cwd, vec![listed_ok(Vec::new())]);
    session.readonly_on = Some(1);
    let (code, stderr) = run_watch(&cwd, &mut session, true);
    fs::set_permissions(&cwd, fs::Permissions::from_mode(0o700)).unwrap();
    assert_eq!(code, 1, "{stderr}");

    let cwd = watch_dir("watch-write-err");
    let mut session = poller(&cwd, vec![listed_ok(Vec::new()), fault(401, None)]);
    session.stop_after_polls = 2;
    session.readonly_on = Some(2);
    let (code, stderr) = run_watch(&cwd, &mut session, true);
    fs::set_permissions(&cwd, fs::Permissions::from_mode(0o700)).unwrap();
    assert_eq!(code, 1, "{stderr}");
    fs::remove_dir_all(&cwd).unwrap();
}

#[test]
fn watch_status_follows_the_failure() {
    assert_eq!(
        watch::poll_status(&Error::Http(401)),
        bistill_lib::SnapshotStatus::Auth
    );
    assert_eq!(
        watch::poll_status(&Error::Http(429)),
        bistill_lib::SnapshotStatus::RateLimited
    );
    assert_eq!(
        watch::poll_status(&Error::Http(500)),
        bistill_lib::SnapshotStatus::Error
    );
    assert_eq!(
        watch::poll_status(&Error::Tls("verify".to_owned())),
        bistill_lib::SnapshotStatus::Tls
    );
    assert_eq!(
        watch::poll_status(&Error::Curl(CurlFault::Timeout {
            program: "curl".to_owned(),
        })),
        bistill_lib::SnapshotStatus::Unreachable
    );
    assert_eq!(
        watch::poll_status(&Error::Curl(CurlFault::Failed {
            program: "curl".to_owned(),
            message: "reset".to_owned(),
        })),
        bistill_lib::SnapshotStatus::Unreachable
    );
    assert_eq!(
        watch::poll_status(&Error::Io(std::io::Error::other("down"))),
        bistill_lib::SnapshotStatus::Unreachable
    );
    assert_eq!(
        watch::poll_status(&Error::Auth("no".to_owned())),
        bistill_lib::SnapshotStatus::Error
    );

    let cwd = watch_dir("watch-tls");
    let mut session = poller(
        &cwd,
        vec![
            listed_ok(Vec::new()),
            Err(bistill_lib::InboxFault {
                error: Error::Tls("verify".to_owned()),
                retry_after_ms: Some(1),
            }),
        ],
    );
    session.stop_after_polls = 2;
    let (code, stderr) = run_watch(&cwd, &mut session, true);
    assert_eq!(code, 0, "{stderr}");
    let snapshot = bistill_lib::read_snapshot(&cwd).unwrap().unwrap();
    assert_eq!(snapshot.status, bistill_lib::SnapshotStatus::Tls);
    fs::remove_dir_all(&cwd).unwrap();
}

#[test]
fn live_poll_reports_a_missing_program() {
    let cwd = watch_dir("live-poll");
    let config = bistill_lib::load(&cwd, &bistill_lib::Flags::default(), &env_token()).unwrap();
    let client = Client::new("bistill-missing-poll", &config);
    let err = match Live.poll(&client, 1, &[], &mut |_| Ok(())) {
        Err(err) => err,
        Ok(_) => panic!("missing"),
    };
    assert!(matches!(
        err.error,
        Error::Curl(CurlFault::Missing { program }) if program == "bistill-missing-poll"
    ));
    assert!(Live.again());
    assert!(Live.now_ms() > 0);
    let (tx, rx) = std::sync::mpsc::channel();
    assert!(matches!(
        Live.pause(Duration::from_millis(0), &rx),
        Paused::Elapsed
    ));
    drop(tx);
    assert!(matches!(
        Live.pause(Duration::from_millis(0), &rx),
        Paused::Quit
    ));
    let (tx, rx) = std::sync::mpsc::channel();
    tx.send(Nudge::Refresh).unwrap();
    assert!(matches!(
        Live.pause(Duration::from_secs(2), &rx),
        Paused::Refresh
    ));
    let (tx, rx) = std::sync::mpsc::channel();
    tx.send(Nudge::Quit).unwrap();
    assert!(matches!(
        Live.pause(Duration::from_secs(2), &rx),
        Paused::Quit
    ));
    fs::remove_dir_all(&cwd).unwrap();
}

#[test]
fn draw_waits_for_a_publish_an_age_and_quit() {
    let cwd = watch_dir("draw-wait");
    let config = bistill_lib::load(&cwd, &bistill_lib::Flags::default(), &env_token()).unwrap();
    let board = std::sync::Mutex::new(watch::Board::new(None));
    let (tray_tx, _tray_rx) = std::sync::mpsc::channel();
    let (nudge_tx, _nudge_rx) = std::sync::mpsc::channel();
    let mut screen = screen::Screen::new();
    let mut open = |_url: &str| Ok(());
    let (tx, rx) = std::sync::mpsc::channel();
    let out = app::Out {
        tray: &tray_tx,
        nudge: &nudge_tx,
    };
    assert!(!app::take_wakes(
        &mut screen,
        &board,
        &config,
        &mut open,
        &out,
        &rx,
        Some(Duration::ZERO),
    ));
    drop(tx);
    assert!(app::take_wakes(
        &mut screen,
        &board,
        &config,
        &mut open,
        &out,
        &rx,
        Some(Duration::ZERO),
    ));
    assert!(matches!(app::next_step(&rx, None), app::Step::Gone));
    let (tx, rx) = std::sync::mpsc::channel();
    tx.send(watch::Wake::Republish).unwrap();
    assert!(matches!(
        app::next_step(&rx, None),
        app::Step::Wake(watch::Wake::Republish)
    ));
    let (tx, rx) = std::sync::mpsc::channel();
    tx.send(watch::Wake::Stop).unwrap();
    assert!(app::take_wakes(
        &mut screen,
        &board,
        &config,
        &mut open,
        &out,
        &rx,
        Some(Duration::from_secs(1)),
    ));
    let (tx, rx) = std::sync::mpsc::channel();
    tx.send(watch::Wake::Republish).unwrap();
    tx.send(watch::Wake::Input(tui::Event::Key(tui::KeyCode::Char('?'))))
        .unwrap();
    assert!(!app::take_wakes(
        &mut screen,
        &board,
        &config,
        &mut open,
        &out,
        &rx,
        Some(Duration::from_secs(1)),
    ));
    let (tx, rx) = std::sync::mpsc::channel();
    tx.send(watch::Wake::Input(tui::Event::Key(tui::KeyCode::Char('q'))))
        .unwrap();
    assert!(app::take_wakes(
        &mut screen,
        &board,
        &config,
        &mut open,
        &out,
        &rx,
        Some(Duration::from_secs(1)),
    ));
    assert!(board.lock().unwrap().stop);
    fs::remove_dir_all(&cwd).unwrap();
}

#[test]
fn next_age_tracks_the_visible_unit() {
    assert!(screen::next_age(None, 1_000).is_none());
    let empty = sample_snapshot(Vec::new(), Vec::new());
    assert!(screen::next_age(Some(&empty), 1_000).is_none());
    let now = 10_000_000_000u64;
    let mut future = sample_row("PRJ", "repo", 1, "Later", false, false, false);
    future.updated_ms = now + 5_000;
    let ahead = sample_snapshot(vec![future], Vec::new());
    assert!(screen::next_age(Some(&ahead), now).is_none());

    let cases = [
        (0u64, 1_000u64),
        (500, 500),
        (90_000, 30_000),
        (7_200_000, 3_600_000),
        (5 * 86_400_000, 86_400_000),
    ];
    for (elapsed, step) in cases {
        let mut row = sample_row("PRJ", "repo", 1, "Aged", false, false, false);
        row.updated_ms = now - elapsed;
        let snapshot = sample_snapshot(vec![row], Vec::new());
        assert_eq!(
            screen::next_age(Some(&snapshot), now),
            Some(Duration::from_millis(step)),
            "{elapsed}"
        );
    }
    let ago_cases = [
        (10_000u64, 50_000u64),
        (3_600_000, 3_600_000),
        (86_400_000, 86_400_000),
    ];
    for (elapsed, step) in ago_cases {
        let mut row = sample_row("PRJ", "repo", 1, "Noted", false, false, false);
        row.updated_ms = now + 1;
        row.events.push(bistill_lib::Event {
            id: 1,
            created_ms: now - elapsed,
            actor_slug: "pat".to_owned(),
            actor_name: "Pat".to_owned(),
            kind: bistill_lib::EventKind::Commented,
            text: "hi".to_owned(),
            thread: Vec::new(),
            added_user: false,
        });
        let snapshot = sample_snapshot(vec![row], Vec::new());
        assert_eq!(
            screen::next_age(Some(&snapshot), now),
            Some(Duration::from_millis(step)),
            "ago {elapsed}"
        );
    }
    let mut soon = sample_row("PRJ", "repo", 1, "Soon", false, false, false);
    soon.updated_ms = now - 90_000;
    let mut later = sample_row("PRJ", "repo", 2, "Later", false, false, false);
    later.updated_ms = now - 500;
    let both = sample_snapshot(vec![soon], vec![later]);
    assert_eq!(
        screen::next_age(Some(&both), now),
        Some(Duration::from_millis(500))
    );
}

#[cfg(unix)]
#[test]
fn refresh_file_wakes_and_cancel_ends_the_wait() {
    use std::os::unix::ffi::OsStrExt;
    let cwd = watch_dir("refresh-watch");
    assert!(refresh_os::pipe_pair(-1).is_err());
    assert!(refresh_os::notify_fd(-1).is_err());
    assert!(refresh_os::c_path(Path::new("a\0b")).is_err());
    assert!(refresh_os::c_path(Path::new("state")).is_ok());
    assert!(refresh_os::bytes_read(0).is_err());
    assert!(refresh_os::bytes_read(-1).is_err());
    assert_eq!(refresh_os::bytes_read(4).unwrap(), 4);
    refresh_os::close_fd(-1);
    assert!(refresh_os::poll_pair(-1, 0).is_err());
    assert!(refresh_os::poll_pair(0, -1).is_err());
    assert!(refresh_os::open(Path::new("/no/such/bistill-refresh")).is_err());
    assert!(refresh_os::open_with(&cwd, -1).is_err());
    let (idle_r, idle_w) = refresh_os::pipe_pair(0).unwrap();
    assert!(refresh_os::poll_for(idle_r, idle_w, 0).is_err());
    refresh_os::close_fd(idle_r);
    refresh_os::close_fd(idle_w);
    let mut zero = vec![0u8; 16];
    zero.extend(inotify_name(b"refresh"));
    assert!(refresh_os::names_refresh(&zero));
    assert!(refresh_os::open(Path::new("a\0b")).is_err());
    let odd = std::ffi::OsStr::from_bytes(&[0xff]);
    assert!(refresh_os::c_path(Path::new(odd)).is_ok());
    assert!(!refresh_os::names_refresh(&[]));
    assert!(!refresh_os::names_refresh(&[0; 8]));
    let mut truncated = vec![0u8; 16];
    truncated[12..16].copy_from_slice(&100u32.to_ne_bytes());
    assert!(!refresh_os::names_refresh(&truncated));
    assert!(!refresh_os::names_refresh(&inotify_name(b"other")));
    assert!(refresh_os::names_refresh(&inotify_name(b"refresh")));
    let mut bare = vec![0u8; 16];
    bare[12..16].copy_from_slice(&7u32.to_ne_bytes());
    bare.extend_from_slice(b"refresh");
    assert!(refresh_os::names_refresh(&bare));
    let mut both = inotify_name(b"other");
    both.extend(inotify_name(b"refresh"));
    assert!(refresh_os::names_refresh(&both));

    let watch = refresh_os::open(&cwd).unwrap();
    thread::scope(|scope| {
        let waiting = scope.spawn(|| watch.wait());
        fs::write(cwd.join("noise"), b"").unwrap();
        thread::sleep(Duration::from_millis(50));
        fs::write(cwd.join("refresh"), b"").unwrap();
        assert!(waiting.join().unwrap().unwrap());
    });
    let watch = refresh_os::open(&cwd).unwrap();
    thread::scope(|scope| {
        let waiting = scope.spawn(|| watch.wait());
        watch.cancel();
        assert!(!waiting.join().unwrap().unwrap());
    });
    fs::remove_dir_all(&cwd).unwrap();
}

#[cfg(unix)]
fn inotify_name(name: &[u8]) -> Vec<u8> {
    let mut body = name.to_vec();
    if !body.contains(&0) {
        body.push(0);
    }
    let mut buf = vec![0u8; 16];
    buf[12..16].copy_from_slice(&(body.len() as u32).to_ne_bytes());
    buf.extend(body);
    buf
}

#[test]
fn watch_reports_config_and_lock_errors() {
    let cwd = watch_dir("watch-lock-err");
    fs::create_dir(cwd.join("poll.lock")).unwrap();
    let mut session = poller(&cwd, Vec::new());
    let (code, stderr) = run_watch(&cwd, &mut session, true);
    assert_eq!(code, 1, "{stderr}");
    fs::remove_dir_all(&cwd).unwrap();

    let cwd = watch_dir("watch-quiet");
    let mut session = poller(&cwd, vec![listed_ok(Vec::new())]);
    let (code, stderr) = run_watch(&cwd, &mut session, false);
    assert_eq!(code, 0, "{stderr}");
    assert!(stderr.is_empty());
    fs::remove_dir_all(&cwd).unwrap();
}

struct Inbox {
    steps: Vec<Result<Listed, bistill_lib::InboxFault>>,
    now: u64,
    jump: u64,
    polls: u32,
    stop_after: u32,
    forever: bool,
    barrier: Option<std::sync::Arc<std::sync::Barrier>>,
    waited: bool,
}

impl Session for Inbox {
    fn version(&mut self, _: &str) -> Result<String, Error> {
        Err(Error::Auth("unused".to_owned()))
    }

    fn ping(&mut self, _: &Client) -> Result<Report, Error> {
        Err(Error::Auth("unused".to_owned()))
    }

    fn poll(
        &mut self,
        _: &Client,
        _: u64,
        _cached: &[Row],
        publish: &mut dyn FnMut(&Snapshot) -> Result<(), Error>,
    ) -> Result<Listed, bistill_lib::InboxFault> {
        self.polls += 1;
        if let Some(barrier) = &self.barrier {
            if !self.waited {
                self.waited = true;
                barrier.wait();
            }
        }
        let result = self.steps.remove(0);
        if let Ok(listed) = &result {
            if let Err(error) = publish(&listed.snapshot) {
                return Err(bistill_lib::InboxFault {
                    error,
                    retry_after_ms: None,
                });
            }
        }
        result
    }

    fn now_ms(&mut self) -> u64 {
        self.now
    }

    fn pause(&mut self, _: Duration, _: &std::sync::mpsc::Receiver<Nudge>) -> Paused {
        self.now += self.jump;
        std::thread::yield_now();
        Paused::Elapsed
    }

    fn again(&mut self) -> bool {
        self.forever || self.polls < self.stop_after
    }
}

fn feed(steps: Vec<Result<Listed, bistill_lib::InboxFault>>) -> Inbox {
    let stop_after = steps.len() as u32;
    Inbox {
        steps,
        now: 1_000_000,
        jump: 20_000,
        polls: 0,
        stop_after,
        forever: false,
        barrier: None,
        waited: false,
    }
}

enum DriveStep {
    Until(String),
    Event(tui::Event),
    /// Return once without an event so the draw thread can wait on an age.
    Idle,
}

struct DrivePark(std::sync::Arc<tui::Gate>);

impl tui::Park for DrivePark {
    fn park(&self, seen: u64) -> u64 {
        match self.0.park_for(seen, Duration::from_secs(8)) {
            Some(generation) => generation,
            None => panic!("stalled"),
        }
    }

    fn unpark(&self) {
        self.0.unpark();
    }
}

struct Drive {
    grid: tui::TestBackend,
    barrier: Option<std::sync::Arc<std::sync::Barrier>>,
    fetch_seen: bool,
    queue: std::collections::VecDeque<DriveStep>,
    spins: u32,
    gate: std::sync::Arc<tui::Gate>,
}

impl Drive {
    fn new(queue: Vec<DriveStep>) -> Self {
        Drive {
            grid: tui::TestBackend::new(100, 24),
            barrier: None,
            fetch_seen: false,
            queue: queue.into(),
            spins: 0,
            gate: tui::Gate::new(),
        }
    }
}

impl tui::Backend for Drive {
    fn size(&self) -> tui::Rect {
        tui::Rect {
            x: 0,
            y: 0,
            width: 100,
            height: 24,
        }
    }

    fn draw(&mut self, buffer: &tui::Buffer) {
        self.grid.draw(buffer);
        tui::Park::unpark(&*self.gate);
    }

    fn parking(&self) -> std::sync::Arc<dyn tui::Park> {
        std::sync::Arc::new(DrivePark(self.gate.clone()))
    }

    fn poll(&mut self, timeout: Duration) -> Option<tui::Event> {
        let _ = timeout;
        let text = drive_text(&self.grid);
        if let Some(barrier) = &self.barrier {
            if !self.fetch_seen && text.contains("Fetching from Bitbucket...") {
                self.fetch_seen = true;
                barrier.wait();
            }
        }
        loop {
            match self.queue.front() {
                Some(DriveStep::Until(needle)) if text.contains(needle) => {
                    self.queue.pop_front();
                }
                Some(DriveStep::Until(_)) => {
                    self.spins += 1;
                    assert!(self.spins < 10_000, "stalled\n{text}");
                    std::thread::yield_now();
                    return None;
                }
                Some(DriveStep::Idle) => {
                    self.queue.pop_front();
                    return None;
                }
                Some(DriveStep::Event(_)) => {
                    if let Some(DriveStep::Event(event)) = self.queue.pop_front() {
                        return Some(event);
                    }
                }
                None => {
                    self.spins += 1;
                    assert!(self.spins < 10_000, "stalled\n{text}");
                    std::thread::yield_now();
                    return None;
                }
            }
        }
    }
}

fn drive_text(grid: &tui::TestBackend) -> String {
    let mut out = String::new();
    for y in 0..24 {
        for x in 0..100 {
            if let Some(cell) = grid.cell(x, y) {
                out.push(glyph_char(&cell.glyph));
            }
        }
        out.push('\n');
    }
    out
}

fn step_key(ch: char) -> DriveStep {
    DriveStep::Event(tui::Event::Key(tui::KeyCode::Char(ch)))
}

#[test]
fn tui_repaints_when_an_age_changes() {
    let cwd = watch_dir("tui-age");
    let mut row = sample_row("PRJ", "repo", 12, "Fix the pipe", false, false, false);
    row.updated_ms = crate::unix_ms(std::time::SystemTime::now()).saturating_sub(400);
    let mut session = feed(vec![listed_ok(vec![row])]);
    let mut backend = Drive::new(vec![
        DriveStep::Until("Fix the pipe".to_owned()),
        DriveStep::Idle,
        step_key('q'),
    ]);
    let prepared = prepare(&cwd, &bistill_lib::Flags::default(), &env_token(), false)
        .unwrap_or_else(|exit| panic!("{}", exit.stderr));
    let exit = drive(&mut backend, prepared, &mut session, &mut |_| Ok(()));
    assert_eq!(exit.code, 0, "{}", exit.stderr);
    fs::remove_dir_all(&cwd).unwrap();
}

#[test]
fn tui_holder_opens_refreshes_and_releases_the_lock() {
    let cwd = watch_dir("tui-holder");
    let barrier = std::sync::Arc::new(std::sync::Barrier::new(2));
    let mut session = feed(vec![listed_ok(vec![sample_row(
        "PRJ",
        "repo",
        12,
        "Fix the pipe",
        false,
        false,
        false,
    )])]);
    session.barrier = Some(barrier.clone());
    let mut backend = Drive::new(vec![
        DriveStep::Until("Fix the pipe".to_owned()),
        DriveStep::Event(tui::Event::Resize {
            width: 100,
            height: 24,
        }),
        DriveStep::Event(tui::Event::Key(tui::KeyCode::Enter)),
        DriveStep::Event(tui::Event::Key(tui::KeyCode::Enter)),
        step_key('r'),
        step_key('q'),
    ]);
    backend.barrier = Some(barrier);
    let mut opened = Vec::new();
    let prepared = prepare(&cwd, &bistill_lib::Flags::default(), &env_token(), false)
        .unwrap_or_else(|exit| panic!("{}", exit.stderr));
    let exit = drive(&mut backend, prepared, &mut session, &mut |url| {
        opened.push(url.to_owned());
        if opened.len() == 1 {
            Ok(())
        } else {
            Err(host::Error::Missing {
                program: "xdg-open".to_owned(),
            })
        }
    });
    assert_eq!(exit.code, 0, "{}", exit.stderr);
    assert_eq!(
        opened,
        vec![
            "https://git.example.invalid/pull/12".to_owned(),
            "https://git.example.invalid/pull/12".to_owned(),
        ]
    );
    assert!(exit.stderr.contains("xdg-open must be on PATH."));
    assert!(!exit.stderr.contains("secret-token"));
    assert!(cwd.join("refresh").is_file());
    assert!(!cwd.join("poll.lock").exists());
    assert!(drive_text(&backend.grid).contains("Fix the pipe"));
    fs::remove_dir_all(&cwd).unwrap();
}

#[test]
fn tui_marks_read_and_ignores_from_the_keys() {
    let cwd = watch_dir("tui-mark");
    fs::write(
        cwd.join("watermarks.json"),
        r#"{"items":[{"id":"PRJ/repo/12","activity_id":1,"ignored":true,"primed":true,"seen_failed":false,"seen_conflict":false}]}"#,
    )
    .unwrap();
    let barrier = std::sync::Arc::new(std::sync::Barrier::new(2));
    let mut session = feed(vec![listed_ok(vec![sample_row(
        "PRJ",
        "repo",
        12,
        "Fix the pipe",
        false,
        false,
        false,
    )])]);
    session.barrier = Some(barrier.clone());
    let mut backend = Drive::new(vec![
        DriveStep::Until("Fix the pipe".to_owned()),
        step_key('m'),
        step_key('i'),
        step_key('i'),
        step_key('q'),
    ]);
    backend.barrier = Some(barrier);
    let prepared = prepare(&cwd, &bistill_lib::Flags::default(), &env_token(), false)
        .unwrap_or_else(|exit| panic!("{}", exit.stderr));
    let exit = drive(&mut backend, prepared, &mut session, &mut |_| Ok(()));
    assert_eq!(exit.code, 0, "{}", exit.stderr);
    let marks = fs::read_to_string(cwd.join("watermarks.json")).unwrap();
    assert!(marks.contains("PRJ/repo/12"));
    assert!(marks.contains("\"ignored\":true"));
    fs::remove_dir_all(&cwd).unwrap();
}

#[test]
fn tui_logs_a_watermark_that_cannot_be_read() {
    let cwd = watch_dir("tui-mark-dir");
    fs::create_dir(cwd.join("watermarks.json")).unwrap();
    let barrier = std::sync::Arc::new(std::sync::Barrier::new(2));
    let mut session = feed(vec![listed_ok(vec![sample_row(
        "PRJ",
        "repo",
        12,
        "Fix the pipe",
        false,
        false,
        false,
    )])]);
    session.barrier = Some(barrier.clone());
    let mut backend = Drive::new(vec![
        DriveStep::Until("Fix the pipe".to_owned()),
        step_key('m'),
        step_key('q'),
    ]);
    backend.barrier = Some(barrier);
    let prepared = prepare(&cwd, &bistill_lib::Flags::default(), &env_token(), false)
        .unwrap_or_else(|exit| panic!("{}", exit.stderr));
    let exit = drive(&mut backend, prepared, &mut session, &mut |_| Ok(()));
    assert_eq!(exit.code, 0, "{}", exit.stderr);
    assert!(!exit.stderr.is_empty());
    fs::remove_dir_all(&cwd).unwrap();
}

#[cfg(unix)]
#[test]
fn tui_logs_a_watermark_that_cannot_be_written() {
    use std::os::unix::fs::PermissionsExt;
    let cwd = watch_dir("tui-mark-ro");
    let log = cwd.join("logs");
    fs::create_dir(&log).unwrap();
    fs::write(
        cwd.join("bistill.conf"),
        format!(
            "state_dir = {}\npoll_seconds = 15\nlog_file = {}\n",
            cwd.display(),
            log.display()
        ),
    )
    .unwrap();
    fs::write(cwd.join("watermarks.json"), b"{\"items\":[]}").unwrap();
    fs::set_permissions(
        cwd.join("watermarks.json"),
        fs::Permissions::from_mode(0o444),
    )
    .unwrap();
    let barrier = std::sync::Arc::new(std::sync::Barrier::new(2));
    let mut session = feed(vec![listed_ok(vec![sample_row(
        "PRJ",
        "repo",
        12,
        "Fix the pipe",
        false,
        false,
        false,
    )])]);
    session.barrier = Some(barrier.clone());
    let mut backend = Drive::new(vec![
        DriveStep::Until("Fix the pipe".to_owned()),
        step_key('m'),
        step_key('q'),
    ]);
    backend.barrier = Some(barrier);
    let prepared = prepare(&cwd, &bistill_lib::Flags::default(), &env_token(), false)
        .unwrap_or_else(|exit| panic!("{}", exit.stderr));
    let exit = drive(&mut backend, prepared, &mut session, &mut |_| Ok(()));
    fs::set_permissions(
        cwd.join("watermarks.json"),
        fs::Permissions::from_mode(0o644),
    )
    .unwrap();
    assert_eq!(exit.code, 1, "{}", exit.stderr);
    fs::remove_dir_all(&cwd).unwrap();
}

#[test]
fn ignore_skips_a_missing_row_and_marks_the_author() {
    let snapshot = sample_snapshot(
        vec![sample_row(
            "PRJ",
            "repo",
            12,
            "Fix the pipe",
            false,
            false,
            false,
        )],
        Vec::new(),
    );
    let mut store = bistill_lib::Store::default();
    crate::app::apply_ignore(&mut store, &snapshot, "missing");
    crate::app::apply_ignore(&mut store, &snapshot, "PRJ/repo/12");
    assert!(bistill_lib::is_ignored(&store, "PRJ/repo/12"));
}

#[test]
fn tui_shows_token_rejected_and_a_failed_poll() {
    let cwd = watch_dir("tui-auth");
    let log = cwd.join("bistill.log");
    fs::write(
        cwd.join("bistill.conf"),
        format!(
            "state_dir = {}\npoll_seconds = 15\nlog_file = {}\n",
            cwd.display(),
            log.display()
        ),
    )
    .unwrap();
    let row = sample_row("PRJ", "repo", 12, "Fix the pipe", false, false, false);
    bistill_lib::write_snapshot(&cwd, &sample_snapshot(vec![row], Vec::new())).unwrap();
    let barrier = std::sync::Arc::new(std::sync::Barrier::new(2));
    let mut session = feed(vec![fault(401, None)]);
    session.barrier = Some(barrier.clone());
    let mut backend = Drive::new(vec![
        DriveStep::Until("Token rejected.".to_owned()),
        step_key('q'),
    ]);
    backend.barrier = Some(barrier);
    let prepared = prepare(&cwd, &bistill_lib::Flags::default(), &env_token(), false)
        .unwrap_or_else(|exit| panic!("{}", exit.stderr));
    let exit = drive(&mut backend, prepared, &mut session, &mut |_| Ok(()));
    assert_eq!(exit.code, 0, "{}", exit.stderr);
    assert!(drive_text(&backend.grid).contains("Fix the pipe"));
    assert!(drive_text(&backend.grid).contains("Token rejected."));
    assert!(
        fs::read_to_string(&log)
            .unwrap()
            .contains("Token rejected.")
    );
    assert!(!cwd.join("poll.lock").exists());

    let cwd = watch_dir("tui-500");
    let barrier = std::sync::Arc::new(std::sync::Barrier::new(2));
    let mut session = feed(vec![fault(500, None)]);
    session.barrier = Some(barrier.clone());
    let mut backend = Drive::new(vec![DriveStep::Until("HTTP 500".to_owned()), step_key('q')]);
    backend.barrier = Some(barrier);
    let prepared = prepare(&cwd, &bistill_lib::Flags::default(), &env_token(), false)
        .unwrap_or_else(|exit| panic!("{}", exit.stderr));
    let exit = drive(&mut backend, prepared, &mut session, &mut |_| Ok(()));
    assert_eq!(exit.code, 0, "{}", exit.stderr);
    assert!(drive_text(&backend.grid).contains("HTTP 500"));
    fs::remove_dir_all(&cwd).unwrap();
    let _ = fs::remove_dir_all(
        std::env::temp_dir().join(format!("bistill-bin-tui-auth-{}", std::process::id())),
    );
}

#[test]
fn tui_second_process_names_the_lock_pid() {
    let cwd = watch_dir("tui-viewer");
    fs::write(cwd.join("poll.lock"), format!("{}\n", std::process::id())).unwrap();
    let mut backend = Drive::new(Vec::new());
    let mut session = feed(Vec::new());
    let prepared = prepare(&cwd, &bistill_lib::Flags::default(), &env_token(), false)
        .unwrap_or_else(|exit| panic!("{}", exit.stderr));
    let exit = drive(&mut backend, prepared, &mut session, &mut |_| Ok(()));
    assert_eq!(exit.code, 1, "{}", exit.stderr);
    assert!(exit.stderr.contains(&std::process::id().to_string()));
    assert_eq!(
        fs::read_to_string(cwd.join("poll.lock")).unwrap().trim(),
        std::process::id().to_string()
    );
    fs::remove_dir_all(&cwd).unwrap();
}

#[test]
fn tui_quits_during_the_wait_and_reports_a_missing_curl() {
    let cwd = watch_dir("tui-quit");
    let mut session = feed(vec![listed_ok(vec![sample_row(
        "PRJ",
        "repo",
        12,
        "Fix the pipe",
        false,
        false,
        false,
    )])]);
    session.forever = true;
    session.jump = 0;
    let mut backend = Drive::new(vec![
        DriveStep::Until("Fix the pipe".to_owned()),
        step_key('q'),
    ]);
    let prepared = prepare(&cwd, &bistill_lib::Flags::default(), &env_token(), false)
        .unwrap_or_else(|exit| panic!("{}", exit.stderr));
    let exit = drive(&mut backend, prepared, &mut session, &mut |_| Ok(()));
    assert_eq!(exit.code, 0, "{}", exit.stderr);
    assert!(!cwd.join("poll.lock").exists());

    let cwd = watch_dir("tui-missing");
    let mut session = feed(vec![Err(bistill_lib::InboxFault {
        error: Error::Curl(CurlFault::Missing {
            program: "curl".to_owned(),
        }),
        retry_after_ms: None,
    })]);
    let mut backend = Drive::new(Vec::new());
    let prepared = prepare(&cwd, &bistill_lib::Flags::default(), &env_token(), false)
        .unwrap_or_else(|exit| panic!("{}", exit.stderr));
    let exit = drive(&mut backend, prepared, &mut session, &mut |_| Ok(()));
    assert_eq!(exit.code, 2, "{}", exit.stderr);
    assert!(exit.stderr.contains("curl must be on PATH."));
    assert!(!cwd.join("poll.lock").exists());

    let cwd = watch_dir("tui-log-dir");
    fs::write(
        cwd.join("bistill.conf"),
        format!(
            "state_dir = {}\npoll_seconds = 15\nlog_file = {}\n",
            cwd.display(),
            cwd.display()
        ),
    )
    .unwrap();
    fs::write(cwd.join("poll.lock"), format!("{}\n", std::process::id())).unwrap();
    let mut backend = Drive::new(vec![step_key('q')]);
    let mut session = feed(Vec::new());
    let prepared = prepare(&cwd, &bistill_lib::Flags::default(), &env_token(), false)
        .unwrap_or_else(|exit| panic!("{}", exit.stderr));
    let exit = drive(&mut backend, prepared, &mut session, &mut |_| Ok(()));
    assert_eq!(exit.code, 1, "{}", exit.stderr);
    fs::remove_dir_all(&cwd).unwrap();
    let _ = fs::remove_dir_all(
        std::env::temp_dir().join(format!("bistill-bin-tui-quit-{}", std::process::id())),
    );
    let _ = fs::remove_dir_all(
        std::env::temp_dir().join(format!("bistill-bin-tui-missing-{}", std::process::id())),
    );
}

#[test]
fn tui_prepare_and_phases() {
    let cwd = watch_dir("tui-config");
    let err = match prepare(&cwd, &bistill_lib::Flags::default(), &Env::new(), false) {
        Err(err) => err,
        Ok(_) => panic!("config"),
    };
    assert_eq!(err.code, 1);
    assert!(!err.stderr.is_empty());

    fs::write(cwd.join("snapshot.json"), b"{").unwrap();
    let err = match prepare(&cwd, &bistill_lib::Flags::default(), &env_token(), false) {
        Err(err) => err,
        Ok(_) => panic!("json"),
    };
    assert_eq!(err.code, 5);

    fs::remove_file(cwd.join("snapshot.json")).unwrap();
    fs::create_dir(cwd.join("poll.lock")).unwrap();
    let prepared = prepare(&cwd, &bistill_lib::Flags::default(), &env_token(), false)
        .unwrap_or_else(|exit| panic!("{}", exit.stderr));
    let mut backend = tui::TestBackend::new(40, 8);
    let mut session = feed(Vec::new());
    let exit = drive(&mut backend, prepared, &mut session, &mut |_| Ok(()));
    assert_eq!(exit.code, 1, "{}", exit.stderr);
    fs::remove_dir_all(&cwd).unwrap();

    assert!(matches!(
        super::app::screen_phase(bistill_lib::SnapshotStatus::Fetching, 0, "", 0),
        screen::Phase::Fetching
    ));
    assert!(matches!(
        super::app::screen_phase(bistill_lib::SnapshotStatus::Ok, 0, "", 15_000),
        screen::Phase::Ready
    ));
    assert!(matches!(
        super::app::screen_phase(bistill_lib::SnapshotStatus::Auth, 0, "", 0),
        screen::Phase::Auth
    ));
    assert!(matches!(
        super::app::screen_phase(bistill_lib::SnapshotStatus::Tls, 0, "", 0),
        screen::Phase::Tls
    ));
    assert!(matches!(
        super::app::screen_phase(bistill_lib::SnapshotStatus::Unreachable, 5, "", 0),
        screen::Phase::Unreachable { since_ms: 5 }
    ));
    assert!(matches!(
        super::app::screen_phase(bistill_lib::SnapshotStatus::RateLimited, 0, "", 0),
        screen::Phase::RateLimited
    ));
    assert!(matches!(
        super::app::screen_phase(bistill_lib::SnapshotStatus::Error, 0, "HTTP 500", 0),
        screen::Phase::Failed { message } if message == "HTTP 500"
    ));
    let mut screen = screen::Screen::new();
    let drawn = draw_screen(
        &mut screen,
        80,
        8,
        None,
        &screen::Role::Holder(screen::Phase::Failed {
            message: "HTTP 500".to_owned(),
        }),
        0,
    );
    assert!(table_has(&drawn, "HTTP 500"));

    match browser("https://git.example.invalid/pull/12") {
        host::Open::XdgOpen {
            program,
            url,
            timeout,
        } => {
            assert_eq!(program, "xdg-open");
            assert_eq!(url, "https://git.example.invalid/pull/12");
            assert_eq!(timeout, Duration::from_secs(15));
        }
        host::Open::WindowsStart { .. } => panic!("xdg-open"),
    }
    let offset = super::zone_os::local_offset_secs();
    assert!(offset.unsigned_abs() <= 24 * 60 * 60);
}

#[test]
fn help_box_covers_the_inbox_and_keys_mark_or_ignore() {
    let mut row = sample_row("PRJ", "repo", 12, "Fix the pipe", false, false, false);
    row.events = vec![
        bistill_lib::Event {
            id: 5,
            created_ms: 1,
            actor_slug: "ada".to_owned(),
            actor_name: "Ada".to_owned(),
            kind: bistill_lib::EventKind::Commented,
            text: "please look".to_owned(),
            thread: vec!["ada".to_owned()],
            added_user: false,
        },
        bistill_lib::Event {
            id: 4,
            created_ms: 1,
            actor_slug: "jcitizen".to_owned(),
            actor_name: "Jane".to_owned(),
            kind: bistill_lib::EventKind::Pushed,
            text: String::new(),
            thread: Vec::new(),
            added_user: false,
        },
        bistill_lib::Event {
            id: 3,
            created_ms: 1,
            actor_slug: "sam".to_owned(),
            actor_name: "Sam".to_owned(),
            kind: bistill_lib::EventKind::Approved,
            text: String::new(),
            thread: Vec::new(),
            added_user: false,
        },
        bistill_lib::Event {
            id: 2,
            created_ms: 1,
            actor_slug: "sam".to_owned(),
            actor_name: "Sam".to_owned(),
            kind: bistill_lib::EventKind::Reopened,
            text: String::new(),
            thread: Vec::new(),
            added_user: false,
        },
        bistill_lib::Event {
            id: 1,
            created_ms: 1,
            actor_slug: "sam".to_owned(),
            actor_name: "Sam".to_owned(),
            kind: bistill_lib::EventKind::Added,
            text: String::new(),
            thread: Vec::new(),
            added_user: true,
        },
        bistill_lib::Event {
            id: 0,
            created_ms: 1,
            actor_slug: "sam".to_owned(),
            actor_name: "Sam".to_owned(),
            kind: bistill_lib::EventKind::Other,
            text: String::new(),
            thread: Vec::new(),
            added_user: false,
        },
    ];
    let snapshot = sample_snapshot(vec![row], Vec::new());
    let mut screen = screen::Screen::new();
    let mut floors = std::collections::BTreeMap::new();
    floors.insert("PRJ/repo/12".to_owned(), 4);
    screen.set_marks(floors, std::collections::BTreeSet::new());
    let grid = draw_screen(
        &mut screen,
        160,
        24,
        Some(&snapshot),
        &screen::Role::Holder(screen::Phase::Ready),
        0,
    );
    let text = grid_text(&grid);
    assert!(text.contains("new "));
    assert_eq!(find_style(&grid, "new").fg, Some(tui::Color::Green));
    assert!(text.contains("please look"));
    assert!(text.contains("pushed"));
    assert_eq!(
        act(
            &mut screen,
            key(tui::KeyCode::Char('m')),
            Some(&snapshot),
            0
        ),
        screen::Action::None
    );
    assert!(matches!(
        screen.take_pending(),
        Some(screen::Pending::Read(id)) if id == "PRJ/repo/12"
    ));
    assert_eq!(
        act(
            &mut screen,
            key(tui::KeyCode::Char('i')),
            Some(&snapshot),
            0
        ),
        screen::Action::None
    );
    assert!(matches!(
        screen.take_pending(),
        Some(screen::Pending::Ignore(id)) if id == "PRJ/repo/12"
    ));
    let mut ignored = std::collections::BTreeSet::new();
    ignored.insert("PRJ/repo/12".to_owned());
    screen.set_marks(std::collections::BTreeMap::new(), ignored);
    let badged = draw_screen(
        &mut screen,
        160,
        24,
        Some(&snapshot),
        &screen::Role::Holder(screen::Phase::Ready),
        0,
    );
    assert!(grid_text(&badged).contains("ignored"));
    assert_eq!(
        act(
            &mut screen,
            key(tui::KeyCode::Char('?')),
            Some(&snapshot),
            0
        ),
        screen::Action::None
    );
    let help = draw_screen(
        &mut screen,
        160,
        24,
        Some(&snapshot),
        &screen::Role::Holder(screen::Phase::Ready),
        0,
    );
    let help_text = grid_text(&help);
    assert!(help_text.contains("Mark this pull request"));
    assert!(help_text.contains("Fix the pipe"));
    assert!(row_text(&help, 23).contains("close"));
    assert_eq!(
        act(&mut screen, key(tui::KeyCode::Tab), Some(&snapshot), 0),
        screen::Action::None
    );
    assert_eq!(
        act(
            &mut screen,
            key(tui::KeyCode::Char('m')),
            Some(&snapshot),
            0
        ),
        screen::Action::None
    );
    assert_eq!(
        act(
            &mut screen,
            key(tui::KeyCode::Char('i')),
            Some(&snapshot),
            0
        ),
        screen::Action::None
    );
    assert!(screen.take_pending().is_none());
}
