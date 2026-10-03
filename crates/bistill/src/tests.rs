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
    toasts: Vec<host::Toast>,
    fail_notify: bool,
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

    fn list(&mut self, client: &Client, now_ms: u64) -> Result<Listed, Error> {
        let _ = now_ms;
        self.origin = client.requests()[0].url.clone();
        self.listed.take().expect("list")
    }

    fn notify(&mut self, toast: &host::Toast) -> Result<(), host::Error> {
        self.toasts.push(toast.clone());
        if self.fail_notify {
            Err(host::Error::Missing {
                program: super::notify_bin::PROGRAM.to_owned(),
            })
        } else {
            Ok(())
        }
    }

    fn poll(&mut self, client: &Client, now_ms: u64) -> Result<Listed, bistill_lib::InboxFault> {
        self.list(client, now_ms)
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
        toasts: Vec::new(),
        fail_notify: false,
    }
}

#[test]
fn help_lists_ping_options() {
    let cwd = temp("help");
    let (code, stdout, stderr, _) = run(&["--help"], &cwd, &env_token(), ready(Ok(report())));
    assert_eq!(code, 0);
    assert!(stdout.contains("bistill ping"));
    assert!(stdout.contains("bistill ls"));
    assert!(stdout.contains("--json"));
    assert!(stdout.contains("--count"));
    assert!(stdout.contains("--verbose"));
    assert!(stdout.contains("bistill watch"));
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
    assert!(stderr.contains("Run ping, ls, or watch."));
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
        toasts: Vec::new(),
        fail_notify: false,
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

fn listed_script(listed: Result<Listed, Error>) -> Script {
    Script {
        version: None,
        report: None,
        listed: Some(listed),
        origin: String::new(),
        toasts: Vec::new(),
        fail_notify: false,
    }
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
fn ls_prints_sections_count_and_json() {
    let cwd = temp("ls");
    let env = env_token();
    let snapshot = || {
        sample_snapshot(
            vec![
                sample_row("PRJ", "repo", 13, "Draft the pipe", true, false, false),
                sample_row("PRJ", "repo", 12, "Fix the pipe", false, false, false),
            ],
            vec![
                sample_row("PRJ", "pipe", 21, "Waiting on Sam", false, true, true),
                sample_row("~jcitizen", "mine", 3, "Personal repo", false, false, false),
            ],
        )
    };
    let (code, stdout, stderr, _) = run(
        &["ls"],
        &cwd,
        &env,
        listed_script(Ok(Listed {
            snapshot: snapshot(),
            requests: Vec::new(),
        })),
    );
    assert_eq!(code, 0);
    assert!(stderr.is_empty());
    assert!(stdout.contains("Needs review\n"));
    assert!(stdout.contains("Waiting on others\n"));
    assert!(stdout.contains("PRJ/repo#13  Draft the pipe  draft\n"));
    assert!(stdout.contains("PRJ/repo#12  Fix the pipe\n"));
    assert!(stdout.contains("PRJ/pipe#21  Waiting on Sam  stale  needs work\n"));
    assert!(stdout.contains("~jcitizen/mine#3  Personal repo\n"));
    let (count_code, count, _, _) = run(
        &["ls", "--count", "--json"],
        &cwd,
        &env,
        listed_script(Ok(Listed {
            snapshot: snapshot(),
            requests: Vec::new(),
        })),
    );
    assert_eq!(count_code, 0);
    assert_eq!(count, "2\n");
    let (json_code, json, _, _) = run(
        &["ls", "--json"],
        &cwd,
        &env,
        listed_script(Ok(Listed {
            snapshot: snapshot(),
            requests: Vec::new(),
        })),
    );
    assert_eq!(json_code, 0);
    assert!(json.contains("\"id\":\"PRJ/repo/13\""));
    assert!(json.ends_with('\n'));
    let disk = fs::read_to_string(cwd.join("snapshot.json")).unwrap();
    assert!(disk.contains("PRJ/repo/13"));
    assert!(disk.contains("fingerprint"));
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
    let (code, stdout, stderr, _) = run(
        &["ls"],
        &cwd,
        &env_token(),
        listed_script(Ok(Listed {
            snapshot: sample_snapshot(Vec::new(), Vec::new()),
            requests: Vec::new(),
        })),
    );
    assert_eq!(code, 1, "{stderr}");
    assert!(stdout.is_empty());
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
    let (code, stdout, stderr, _) = run(
        &["ls"],
        &cwd,
        &env_token(),
        listed_script(Ok(Listed {
            snapshot: sample_snapshot(Vec::new(), Vec::new()),
            requests: Vec::new(),
        })),
    );
    fs::set_permissions(&dir, fs::Permissions::from_mode(0o700)).unwrap();
    assert_eq!(code, 1, "{stderr}");
    assert!(stdout.is_empty());
    fs::remove_dir_all(&cwd).unwrap();
}

#[test]
fn ls_empty_is_success_and_verbose_redacts() {
    let cwd = temp("ls-empty");
    let env = env_token();
    let config = bistill_lib::load(&cwd, &bistill_lib::Flags::default(), &env).unwrap();
    let client = Client::new(curl_bin::PROGRAM, &config);
    let request =
        client.request("/rest/api/1.0/inbox/pull-requests?role=REVIEWER&start=0&limit=25");
    let (code, stdout, stderr, _) = run(
        &["ls", "--verbose"],
        &cwd,
        &env,
        listed_script(Ok(Listed {
            snapshot: sample_snapshot(Vec::new(), Vec::new()),
            requests: vec![request],
        })),
    );
    assert_eq!(code, 0);
    assert_eq!(stdout, "Nothing needs your attention.\n");
    assert!(stderr.contains("Bearer ***"));
    assert!(!stderr.contains("secret-token"));
    let (missing, _, err, _) = run(
        &["ls"],
        &cwd,
        &Env::new(),
        listed_script(Ok(Listed {
            snapshot: sample_snapshot(Vec::new(), Vec::new()),
            requests: Vec::new(),
        })),
    );
    assert_eq!(missing, 1);
    assert!(err.contains("base_url"));
    let (rejected, out, token, _) = run(&["ls"], &cwd, &env, listed_script(Err(Error::Http(401))));
    assert_eq!(rejected, 11);
    assert!(out.is_empty());
    assert!(token.contains("Token rejected."));
    let (url_code, _, _, origin) = run(
        &["ls", "--url", "https://ls.example.invalid"],
        &cwd,
        &env,
        listed_script(Ok(Listed {
            snapshot: sample_snapshot(Vec::new(), Vec::new()),
            requests: Vec::new(),
        })),
    );
    assert_eq!(url_code, 0);
    assert!(origin.starts_with("https://ls.example.invalid"));
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
    let err = match Live.list(&client, 0) {
        Err(err) => err,
        Ok(_) => panic!("expected HTTP 500"),
    };
    assert!(matches!(err, Error::Http(500)), "{err}");
    thread.join().expect("server");
    fs::remove_dir_all(&cwd).unwrap();
}

#[test]
fn ls_notifies_each_change_and_logs_a_missing_notifier_once() {
    let cwd = temp("notify");
    let env = env_token();
    let rows = || {
        sample_snapshot(
            vec![sample_row(
                "PRJ",
                "repo",
                12,
                "Fix the pipe",
                false,
                false,
                false,
            )],
            vec![sample_row(
                "PRJ",
                "pipe",
                21,
                "Waiting on Sam",
                false,
                true,
                true,
            )],
        )
    };
    let mut script = listed_script(Ok(Listed {
        snapshot: rows(),
        requests: Vec::new(),
    }));
    script.fail_notify = true;
    let mut stdout = Vec::new();
    let mut stderr = Vec::new();
    let code = execute(
        &args(&["ls"]),
        &cwd,
        &env,
        &mut stdout,
        &mut stderr,
        &mut script,
    );
    let err = String::from_utf8(stderr).unwrap();
    let out = String::from_utf8(stdout).unwrap();
    assert_eq!(code, 0, "{err}");
    assert!(out.contains("Needs review"));
    assert_eq!(script.toasts.len(), 2);
    let program = super::notify_bin::PROGRAM;
    assert_eq!(
        err.matches(&format!("{program} must be on PATH.")).count(),
        1
    );
    assert_toast(
        &script.toasts[0],
        "PRJ/repo#12 needs review\nhttps://git.example.invalid/pull/12",
    );
    assert_toast(
        &script.toasts[1],
        "PRJ/pipe#21 waiting\nhttps://git.example.invalid/pull/21",
    );

    let mut quiet = listed_script(Ok(Listed {
        snapshot: rows(),
        requests: Vec::new(),
    }));
    let mut stdout = Vec::new();
    let mut stderr = Vec::new();
    let code = execute(
        &args(&["ls"]),
        &cwd,
        &env,
        &mut stdout,
        &mut stderr,
        &mut quiet,
    );
    assert_eq!(code, 0);
    assert!(quiet.toasts.is_empty());
    assert!(String::from_utf8(stderr).unwrap().is_empty());

    let mut gone = listed_script(Ok(Listed {
        snapshot: sample_snapshot(
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
        ),
        requests: Vec::new(),
    }));
    let mut stdout = Vec::new();
    let mut stderr = Vec::new();
    let code = execute(
        &args(&["ls"]),
        &cwd,
        &env,
        &mut stdout,
        &mut stderr,
        &mut gone,
    );
    assert_eq!(code, 0);
    assert_eq!(gone.toasts.len(), 1);
    assert_toast(
        &gone.toasts[0],
        "PRJ/pipe#21 merged or declined\nhttps://git.example.invalid/pull/21",
    );
    fs::remove_dir_all(&cwd).unwrap();
}

fn assert_toast(toast: &host::Toast, body: &str) {
    let args = host::toast_arguments(toast);
    let text = args
        .iter()
        .map(|arg| arg.to_string_lossy().into_owned())
        .collect::<Vec<_>>()
        .join("\n");
    assert!(text.contains(body), "{text}");
    #[cfg(unix)]
    {
        match toast {
            host::Toast::NotifySend {
                program,
                title,
                expire,
                ..
            } => {
                assert_eq!(program, "notify-send");
                assert_eq!(title, "Bistill");
                assert_eq!(*expire, Some(Duration::from_millis(10_000)));
            }
            host::Toast::PowerShell { .. } => panic!("linux toast is notify-send"),
        }
        assert!(text.contains("--expire-time"));
        assert!(text.contains("10000"));
    }
    #[cfg(windows)]
    {
        match toast {
            host::Toast::PowerShell {
                program,
                title,
                url,
                ..
            } => {
                assert_eq!(program, "powershell.exe");
                assert_eq!(title, "Bistill");
                assert_eq!(url.as_deref(), Some(body.lines().nth(1).unwrap()));
            }
            host::Toast::NotifySend { .. } => panic!("windows toast is powershell"),
        }
    }
}

#[test]
fn ls_refuses_a_corrupt_snapshot() {
    let cwd = temp("corrupt-snapshot");
    fs::write(cwd.join("snapshot.json"), "{").unwrap();
    let (code, stdout, stderr, _) = run(
        &["ls"],
        &cwd,
        &env_token(),
        listed_script(Ok(Listed {
            snapshot: sample_snapshot(Vec::new(), Vec::new()),
            requests: Vec::new(),
        })),
    );
    assert_eq!(code, 5, "{stderr}");
    assert!(stdout.is_empty());
    fs::remove_dir_all(&cwd).unwrap();
}

#[test]
fn live_notify_reports_a_missing_program() {
    let toast = host::Toast::NotifySend {
        program: "bistill-missing-notify".to_owned(),
        title: "Bistill".to_owned(),
        body: "body".to_owned(),
        expire: None,
        timeout: Duration::from_secs(1),
    };
    let err = Live.notify(&toast).unwrap_err();
    assert!(matches!(
        err,
        host::Error::Missing { program } if program == "bistill-missing-notify"
    ));
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
    let snapshot = inbox();
    let holder = screen::Role::Holder(screen::Phase::Fetching);
    let mut screen = screen::Screen::new();
    let wide = draw_screen(&mut screen, 160, 24, Some(&snapshot), &holder, 0);
    let top = row_text(&wide, 0);
    assert!(top.contains("Needs review"));
    assert!(top.contains("Waiting"));
    assert_eq!(row_of(&wide, "Detail"), Some(1));
    let shown = grid_text(&wide);
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
    assert!(shown.contains("R0 UNAPPROVED"));
    assert!(shown.contains("R1 NEEDS_WORK"));
    assert!(shown.contains("R2 APPROVED"));
    assert!(row_text(&wide, 23).contains("Fetching from Bitbucket..."));
    assert!(wide.get(1, 2).unwrap().style.reverse);

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
    let detail = row_of(&stacked, "Detail").unwrap();
    assert!(detail > 1);
    assert!(!row_text(&stacked, 1).contains("Detail"));

    let ready = screen::Role::Holder(screen::Phase::Ready);
    let ready_grid = draw_screen(
        &mut screen::Screen::new(),
        160,
        24,
        Some(&snapshot),
        &ready,
        0,
    );
    assert!(row_text(&ready_grid, 23).trim().is_empty());

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

    let viewer = screen::Role::Viewer { pid: 42 };
    let viewer_grid = draw_screen(
        &mut screen::Screen::new(),
        160,
        24,
        Some(&snapshot),
        &viewer,
        0,
    );
    assert!(row_text(&viewer_grid, 23).contains("Holder 42."));
    let loading = draw_screen(&mut screen::Screen::new(), 160, 24, None, &viewer, 0);
    let loading_text = grid_text(&loading);
    assert!(row_text(&loading, 23).contains("Fetching from Bitbucket..."));
    assert!(loading_text.contains("0/0"));
    assert!(!loading_text.contains("Nothing needs your attention."));

    let empty = sample_snapshot(Vec::new(), Vec::new());
    let empty_grid = draw_screen(&mut screen::Screen::new(), 160, 24, Some(&empty), &ready, 0);
    assert!(grid_text(&empty_grid).contains("Nothing needs your attention."));

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
    assert!(!side_text(&draft, 80).contains("build "));

    let _ = act(
        &mut screen,
        key(tui::KeyCode::Char('j')),
        Some(&snapshot),
        0,
    );
    let ship = draw_screen(&mut screen, 160, 24, Some(&snapshot), &ready, 0);
    assert!(side_text(&ship, 80).contains("build in progress"));

    let _ = act(
        &mut screen,
        key(tui::KeyCode::Char('j')),
        Some(&snapshot),
        0,
    );
    let quiet = draw_screen(&mut screen, 160, 24, Some(&snapshot), &ready, 0);
    assert!(side_text(&quiet, 80).contains("Quiet change"));
    assert!(side_text(&quiet, 80).contains("unanswered as author 0"));
    assert!(!side_text(&quiet, 80).contains("build "));
    assert!(!side_text(&quiet, 80).contains("conflicted"));
    assert!(!side_text(&quiet, 80).contains("mergeable"));

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
    assert!(side_text(&marked, 80).contains("build successful"));
    assert!(side_text(&marked, 80).contains("conflicted"));
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

    let mut screen = screen::Screen::new();
    let _ = draw_screen(&mut screen, 160, 24, Some(&snapshot), &ready, 0);
    for _ in 0..3 {
        let _ = act(
            &mut screen,
            key(tui::KeyCode::Char('j')),
            Some(&snapshot),
            0,
        );
    }
    let _ = draw_screen(&mut screen, 160, 24, Some(&snapshot), &ready, 0);
    assert_eq!(
        act(&mut screen, press_at(81, 4), Some(&snapshot), 10),
        screen::Action::None
    );
    let _ = act(
        &mut screen,
        tui::Event::Wheel {
            direction: tui::Wheel::Down,
            column: 81,
            row: 4,
        },
        Some(&snapshot),
        0,
    );
    let quiet = draw_screen(&mut screen, 160, 24, Some(&snapshot), &ready, 0);
    assert!(side_text(&quiet, 80).contains("Quiet change"));
    assert_eq!(
        act(&mut screen, press_at(1, 2), Some(&snapshot), 20),
        screen::Action::None
    );
    let _ = draw_screen(&mut screen, 160, 24, Some(&snapshot), &ready, 0);
    assert_eq!(
        act(&mut screen, press_at(81, 4), Some(&snapshot), 30),
        screen::Action::None
    );
    for _ in 0..20 {
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
    let tail = draw_screen(&mut screen, 160, 24, Some(&snapshot), &ready, 0);
    assert!(!side_text(&tail, 80).contains("R0 UNAPPROVED"));
    assert!(side_text(&tail, 80).contains("feature -> main"));
    assert!(side_text(&tail, 80).contains("unanswered as author 2"));
    assert!(side_text(&tail, 80).contains("unanswered as reviewer 3"));
    assert!(side_text(&tail, 80).contains("open tasks 4"));
    assert!(side_text(&tail, 80).contains("build failed"));
    assert!(side_text(&tail, 80).contains("conflicted"));
    assert!(side_text(&tail, 80).contains("mergeable"));
    assert!(side_text(&tail, 80).contains("https://git.example.invalid/pull/12"));
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
    let same = draw_screen(&mut screen, 160, 24, Some(&snapshot), &ready, 0);
    assert_eq!(side_text(&same, 80), before);
    for _ in 0..20 {
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
    let head = draw_screen(&mut screen, 160, 24, Some(&snapshot), &ready, 0);
    assert!(side_text(&head, 80).contains("R0 UNAPPROVED"));
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
    let still = draw_screen(&mut screen, 160, 24, Some(&snapshot), &ready, 0);
    assert!(side_text(&still, 80).contains("R0 UNAPPROVED"));
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
    readonly_on: Option<u32>,
    state: PathBuf,
    toasts: Vec<host::Toast>,
    fail_notify: bool,
}

impl Session for Poller {
    fn version(&mut self, _: &str) -> Result<String, Error> {
        Err(Error::Auth("unused".to_owned()))
    }

    fn ping(&mut self, _: &Client) -> Result<Report, Error> {
        Err(Error::Auth("unused".to_owned()))
    }

    fn list(&mut self, _: &Client, _: u64) -> Result<Listed, Error> {
        Err(Error::Auth("unused".to_owned()))
    }

    fn notify(&mut self, toast: &host::Toast) -> Result<(), host::Error> {
        self.toasts.push(toast.clone());
        if self.fail_notify {
            Err(host::Error::Missing {
                program: notify_bin::PROGRAM.to_owned(),
            })
        } else {
            Ok(())
        }
    }

    fn poll(&mut self, _: &Client, _: u64) -> Result<Listed, bistill_lib::InboxFault> {
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
        self.steps.remove(0)
    }

    fn now_ms(&mut self) -> u64 {
        self.now
    }

    fn pause(&mut self, duration: Duration) {
        self.pauses.push(duration.as_millis() as u64);
        if self.refresh {
            fs::write(self.state.join("refresh"), b"").unwrap();
            self.refresh = false;
        }
        self.now += self.jump;
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
        readonly_on: None,
        state: cwd.to_owned(),
        toasts: Vec::new(),
        fail_notify: false,
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

fn run_watch(cwd: &Path, poller: &mut Poller) -> (i32, String) {
    let mut stdout = Vec::new();
    let mut stderr = Vec::new();
    let code = execute(
        &args(&["watch", "--verbose"]),
        cwd,
        &env_token(),
        &mut stdout,
        &mut stderr,
        poller,
    );
    (code, String::from_utf8(stderr).unwrap())
}

#[test]
fn watch_polls_backs_off_and_releases_the_lock() {
    let cwd = watch_dir("watch-poll");
    let again = || sample_row("PRJ", "repo", 12, "Fix the pipe", false, false, false);
    let mut session = poller(
        &cwd,
        vec![listed_ok(vec![again()]), listed_ok(vec![again()])],
    );
    session.fail_notify = true;
    session.stop_after_polls = 2;
    let (code, stderr) = run_watch(&cwd, &mut session);
    assert_eq!(code, 0, "{stderr}");
    assert!(stderr.contains("Bearer ***"));
    assert!(!stderr.contains("secret-token"));
    assert_eq!(session.toasts.len(), 1);
    assert!(stderr.contains("must be on PATH."));
    assert_eq!(session.pauses, vec![1_000]);
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
    let (code, stderr) = run_watch(&cwd, &mut session);
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
    let (code, _) = run_watch(&cwd, &mut session);
    assert_eq!(code, 0);
    assert_eq!(session.pauses, vec![1_000, 1_000, 1_000]);
    assert!(!cwd.join("snapshot.json").exists());

    let cwd = watch_dir("watch-refresh");
    let mut session = poller(&cwd, vec![fault(429, Some(120_000)), listed_ok(Vec::new())]);
    session.stop_after_polls = 2;
    session.jump = 200_000;
    session.refresh = true;
    let (code, stderr) = run_watch(&cwd, &mut session);
    assert_eq!(code, 0, "{stderr}");
    assert_eq!(session.pauses, vec![1_000]);
    assert!(!cwd.join("refresh").exists());
    let snapshot = bistill_lib::read_snapshot(&cwd).unwrap().unwrap();
    assert_eq!(snapshot.status, bistill_lib::SnapshotStatus::Ok);

    let cwd = watch_dir("watch-floor");
    let mut session = poller(&cwd, vec![fault(429, Some(15_000))]);
    session.stop_after_polls = 2;
    session.stop_after_pauses = Some(1);
    session.refresh = true;
    let (code, _) = run_watch(&cwd, &mut session);
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
    let (code, _) = run_watch(&cwd, &mut session);
    assert_eq!(code, 0);
    assert_eq!(session.pauses.len(), 2);

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
    let (code, stderr) = run_watch(&cwd, &mut session);
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
    let (code, stderr) = run_watch(&cwd, &mut session);
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
    let (code, stderr) = run_watch(&cwd, &mut session);
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
    let (code, stderr) = run_watch(&cwd, &mut session);
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
    let (code, stderr) = run_watch(&cwd, &mut session);
    fs::set_permissions(&cwd, fs::Permissions::from_mode(0o700)).unwrap();
    assert_eq!(code, 1, "{stderr}");

    let cwd = watch_dir("watch-write-err");
    let mut session = poller(&cwd, vec![listed_ok(Vec::new()), fault(401, None)]);
    session.stop_after_polls = 2;
    session.readonly_on = Some(2);
    let (code, stderr) = run_watch(&cwd, &mut session);
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
    let (code, stderr) = run_watch(&cwd, &mut session);
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
    let err = match Live.poll(&client, 1) {
        Err(err) => err,
        Ok(_) => panic!("missing"),
    };
    assert!(matches!(
        err.error,
        Error::Curl(CurlFault::Missing { program }) if program == "bistill-missing-poll"
    ));
    assert!(Live.again());
    assert!(Live.now_ms() > 0);
    Live.pause(Duration::from_millis(0));
    fs::remove_dir_all(&cwd).unwrap();
}

#[test]
fn watch_reports_config_and_lock_errors() {
    let cwd = watch_dir("watch-config");
    let mut session = poller(&cwd, Vec::new());
    let mut stdout = Vec::new();
    let mut stderr = Vec::new();
    let code = execute(
        &args(&["watch"]),
        &cwd,
        &Env::new(),
        &mut stdout,
        &mut stderr,
        &mut session,
    );
    assert_eq!(code, 1);
    fs::remove_dir_all(&cwd).unwrap();

    let cwd = watch_dir("watch-lock-err");
    fs::create_dir(cwd.join("poll.lock")).unwrap();
    let mut session = poller(&cwd, Vec::new());
    let (code, stderr) = run_watch(&cwd, &mut session);
    assert_eq!(code, 1, "{stderr}");
    fs::remove_dir_all(&cwd).unwrap();

    let cwd = watch_dir("watch-quiet");
    let mut session = poller(&cwd, vec![listed_ok(Vec::new())]);
    let mut stdout = Vec::new();
    let mut stderr = Vec::new();
    let code = execute(
        &args(&["watch"]),
        &cwd,
        &env_token(),
        &mut stdout,
        &mut stderr,
        &mut session,
    );
    assert_eq!(code, 0, "{}", String::from_utf8(stderr).unwrap());
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

    fn list(&mut self, _: &Client, _: u64) -> Result<Listed, Error> {
        Err(Error::Auth("unused".to_owned()))
    }

    fn notify(&mut self, _: &host::Toast) -> Result<(), host::Error> {
        Ok(())
    }

    fn poll(&mut self, _: &Client, _: u64) -> Result<Listed, bistill_lib::InboxFault> {
        self.polls += 1;
        if let Some(barrier) = &self.barrier {
            if !self.waited {
                self.waited = true;
                barrier.wait();
            }
        }
        self.steps.remove(0)
    }

    fn now_ms(&mut self) -> u64 {
        self.now
    }

    fn pause(&mut self, _: Duration) {
        self.now += self.jump;
        std::thread::yield_now();
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
    Run(Box<dyn FnMut()>),
    Event(tui::Event),
}

struct Drive {
    grid: tui::TestBackend,
    barrier: Option<std::sync::Arc<std::sync::Barrier>>,
    fetch_seen: bool,
    queue: std::collections::VecDeque<DriveStep>,
    spins: u32,
}

impl Drive {
    fn new(queue: Vec<DriveStep>) -> Self {
        Drive {
            grid: tui::TestBackend::new(100, 24),
            barrier: None,
            fetch_seen: false,
            queue: queue.into(),
            spins: 0,
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
                Some(DriveStep::Run(_)) => {
                    if let Some(DriveStep::Run(mut run)) = self.queue.pop_front() {
                        run();
                    }
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
    let prepared = prepare(&cwd, &env_token()).unwrap_or_else(|exit| panic!("{}", exit.stderr));
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
    let prepared = prepare(&cwd, &env_token()).unwrap_or_else(|exit| panic!("{}", exit.stderr));
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
    let prepared = prepare(&cwd, &env_token()).unwrap_or_else(|exit| panic!("{}", exit.stderr));
    let exit = drive(&mut backend, prepared, &mut session, &mut |_| Ok(()));
    assert_eq!(exit.code, 0, "{}", exit.stderr);
    assert!(drive_text(&backend.grid).contains("HTTP 500"));
    fs::remove_dir_all(&cwd).unwrap();
    let _ = fs::remove_dir_all(
        std::env::temp_dir().join(format!("bistill-bin-tui-auth-{}", std::process::id())),
    );
}

#[test]
fn tui_viewer_rereads_and_leaves_the_holder() {
    let cwd = watch_dir("tui-viewer");
    fs::write(cwd.join("poll.lock"), format!("{}\n", std::process::id())).unwrap();
    let first = cwd.clone();
    let second = cwd.clone();
    let garbage = cwd.clone();
    let mut backend = Drive::new(vec![
        DriveStep::Until("Fetching from Bitbucket...".to_owned()),
        DriveStep::Run(Box::new(move || {
            let row = sample_row("PRJ", "repo", 12, "First pipe", false, false, false);
            bistill_lib::write_snapshot(&first, &sample_snapshot(vec![row], Vec::new())).unwrap();
        })),
        DriveStep::Until("First pipe".to_owned()),
        DriveStep::Run(Box::new(move || {
            let row = sample_row("PRJ", "repo", 13, "Second pipe", false, false, false);
            bistill_lib::write_snapshot(&second, &sample_snapshot(vec![row], Vec::new())).unwrap();
        })),
        DriveStep::Until("Second pipe".to_owned()),
        DriveStep::Run(Box::new(move || {
            fs::write(garbage.join("snapshot.json"), b"{").unwrap();
        })),
        step_key('r'),
        step_key('q'),
    ]);
    let mut session = feed(Vec::new());
    let prepared = prepare(&cwd, &env_token()).unwrap_or_else(|exit| panic!("{}", exit.stderr));
    let exit = drive(&mut backend, prepared, &mut session, &mut |_| Ok(()));
    assert_eq!(exit.code, 0, "{}", exit.stderr);
    let text = drive_text(&backend.grid);
    assert!(text.contains("Second pipe"));
    assert!(text.contains(&format!("Holder {}.", std::process::id())));
    assert!(cwd.join("refresh").is_file());
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
    let prepared = prepare(&cwd, &env_token()).unwrap_or_else(|exit| panic!("{}", exit.stderr));
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
    let prepared = prepare(&cwd, &env_token()).unwrap_or_else(|exit| panic!("{}", exit.stderr));
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
    let prepared = prepare(&cwd, &env_token()).unwrap_or_else(|exit| panic!("{}", exit.stderr));
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
    let err = match prepare(&cwd, &Env::new()) {
        Err(err) => err,
        Ok(_) => panic!("config"),
    };
    assert_eq!(err.code, 1);
    assert!(!err.stderr.is_empty());

    fs::write(cwd.join("snapshot.json"), b"{").unwrap();
    let err = match prepare(&cwd, &env_token()) {
        Err(err) => err,
        Ok(_) => panic!("json"),
    };
    assert_eq!(err.code, 5);

    fs::remove_file(cwd.join("snapshot.json")).unwrap();
    fs::create_dir(cwd.join("poll.lock")).unwrap();
    let prepared = prepare(&cwd, &env_token()).unwrap_or_else(|exit| panic!("{}", exit.stderr));
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
