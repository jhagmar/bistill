use super::*;
use crate::config::{nonempty, path_from, sample_state};
use std::ffi::OsString;
use std::fs;
use std::io::{Read, Write};
use std::net::TcpListener;
use std::path::{Path, PathBuf};
use std::thread;
use std::time::{Duration, Instant};

fn env_token() -> Env {
    let mut env = Env::new();
    env.base_url = Some("https://git.example.invalid".to_owned());
    env.username = Some("jcitizen".to_owned());
    env.token = Some("secret-token".to_owned());
    env.dirs = Dirs::sample();
    env
}

fn temp(name: &str) -> PathBuf {
    let path = std::env::temp_dir().join(format!("bistill-cfg-{name}-{}", std::process::id()));
    if path.exists() {
        relax(&path);
        fs::remove_dir_all(&path).unwrap();
    }
    fs::create_dir_all(&path).unwrap();
    path
}

fn relax(path: &Path) {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = fs::set_permissions(path, fs::Permissions::from_mode(0o700));
        if let Ok(entries) = fs::read_dir(path) {
            for entry in entries.flatten() {
                relax(&entry.path());
            }
        }
    }
    #[cfg(not(unix))]
    {
        let _ = path;
    }
}

fn write(path: &Path, text: &str) {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).unwrap();
    }
    fs::write(path, text).unwrap();
}

fn write_private(path: &Path, text: &str) {
    write(path, text);
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(path, fs::Permissions::from_mode(0o600)).unwrap();
    }
}

#[cfg(unix)]
fn write_mode(path: &Path, text: &str, mode: u32) {
    write(path, text);
    use std::os::unix::fs::PermissionsExt;
    fs::set_permissions(path, fs::Permissions::from_mode(mode)).unwrap();
}

#[test]
fn errors_display_and_convert() {
    let missing = Error::from(host::Error::Missing {
        program: "curl".to_owned(),
    });
    assert!(missing.to_string().contains("curl is not on PATH"));
    let timeout = Error::from(host::Error::Timeout {
        program: "curl".to_owned(),
    });
    assert!(timeout.to_string().contains("timed out"));
    let failed = Error::from(host::Error::Failed {
        program: "curl".to_owned(),
        message: "exit 7".to_owned(),
    });
    assert!(failed.to_string().contains("curl failed: exit 7"));
    let tls = Error::from(host::Error::Tls {
        message: "verify".to_owned(),
    });
    assert!(tls.to_string().contains("TLS failed: verify"));
    let json = Error::from(json::parse(b"{").unwrap_err());
    assert!(json.to_string().contains("line"));
    assert!(std::error::Error::source(&json).is_some());
    let io = Error::from(std::io::Error::other("disk"));
    assert!(io.to_string().contains("disk"));
    assert!(std::error::Error::source(&io).is_some());
    let http = Error::Http(401);
    assert_eq!(http.to_string(), "HTTP 401");
    let auth = Error::Auth("slug mismatch".to_owned());
    assert_eq!(auth.to_string(), "slug mismatch");
    let faults = [
        ConfigFault::Missing { key: "base_url" },
        ConfigFault::Unknown {
            key: "port".to_owned(),
        },
        ConfigFault::Duplicate {
            key: "username".to_owned(),
        },
        ConfigFault::Value {
            key: "base_url".to_owned(),
            message: "trailing slash",
        },
        ConfigFault::TokenMode {
            path: PathBuf::from("token"),
        },
        ConfigFault::Read {
            path: PathBuf::from("token"),
            message: "denied".to_owned(),
        },
        ConfigFault::Utf8 {
            path: PathBuf::from("token"),
        },
    ];
    for fault in faults {
        let err = Error::Config(fault);
        assert!(!err.to_string().is_empty());
        assert!(std::error::Error::source(&err).is_none());
        let _ = format!("{err:?}");
    }
    for err in [&missing, &timeout, &failed, &tls, &json, &io, &http, &auth] {
        let _ = format!("{err:?}");
        let _ = std::error::Error::source(err);
    }
}

#[test]
fn redact_argv_hides_bearer_values() {
    let args = [
        OsString::from("-H"),
        OsString::from("Authorization: Bearer secret-token"),
        OsString::from("Bearer one Bearer two"),
        OsString::from("Bearer "),
        OsString::from("Accept: application/json"),
    ];
    let text = redact_argv(&args);
    assert_eq!(
        text,
        vec![
            "-H",
            "Authorization: Bearer ***",
            "Bearer *** Bearer ***",
            "Bearer ",
            "Accept: application/json",
        ]
    );
    assert!(
        !text
            .iter()
            .any(|arg| arg.contains("secret-token") || arg.contains("one") || arg.contains("two"))
    );
}

#[test]
fn env_token_loads_without_a_file() {
    let env = env_token();
    let loaded = load(Path::new("/no/such/cwd"), &Flags::default(), &env).unwrap();
    assert_eq!(loaded.base_url, "https://git.example.invalid");
    assert_eq!(loaded.username, "jcitizen");
    assert_eq!(loaded.token(), "secret-token");
    assert_eq!(loaded.poll_seconds.get(), 60);
    assert_eq!(loaded.stale_days, 7);
    assert_eq!(loaded.state_dir, sample_state());
    assert!(loaded.ca_file.is_none());
    assert!(loaded.log_file.is_none());
    let debug = format!("{loaded:?}");
    assert!(!debug.contains("secret-token"));
    assert!(debug.contains("***"));
    let env_debug = format!("{env:?}");
    assert!(!env_debug.contains("secret-token"));
    assert!(format!("{:?}", Env::default()).contains("token"));
    assert_eq!(
        nonempty(Ok("https://git.example.invalid".to_owned())).as_deref(),
        Some("https://git.example.invalid")
    );
    assert!(nonempty(Ok(String::new())).is_none());
    assert!(nonempty(Err(std::env::VarError::NotPresent)).is_none());
    assert_eq!(
        path_from(Some("/tmp".to_owned())),
        Some(PathBuf::from("/tmp"))
    );
    assert!(path_from(None).is_none());
    let _ = Env::from_process();
}

#[test]
fn flags_win_over_env_and_file() {
    let dir = temp("flags");
    write(
        &dir.join("bistill.conf"),
        "base_url = https://file.example.invalid/\nusername = file-user\ntoken_file = /file/token\n",
    );
    let mut env = Env::new();
    env.base_url = Some("https://env.example.invalid".to_owned());
    env.username = Some("env-user".to_owned());
    env.token = Some("env-token".to_owned());
    env.dirs = Dirs::sample();
    let flags = Flags {
        config: None,
        base_url: Some("https://flag.example.invalid".to_owned()),
        username: Some("flag-user".to_owned()),
        token_file: Some(PathBuf::from("/flag/token")),
    };
    let loaded = load(&dir, &flags, &env).unwrap();
    assert_eq!(loaded.base_url, "https://flag.example.invalid");
    assert_eq!(loaded.username, "flag-user");
    assert_eq!(loaded.token(), "env-token");

    let mut env = Env::new();
    env.base_url = Some("https://env.example.invalid".to_owned());
    env.username = Some("env-user".to_owned());
    env.token_file = Some(dir.join("token"));
    env.dirs = Dirs::sample();
    write_private(&dir.join("token"), "file-secret\n");
    let loaded = load(&dir, &Flags::default(), &env).unwrap();
    assert_eq!(loaded.base_url, "https://env.example.invalid");
    assert_eq!(loaded.username, "env-user");
    assert_eq!(loaded.token(), "file-secret");

    env.base_url = None;
    env.username = None;
    env.token_file = None;
    let loaded = load(&dir, &Flags::default(), &env).unwrap_err();
    assert!(loaded.to_string().contains("trailing slash"));
}

#[test]
fn search_order_and_platform_defaults() {
    let dir = temp("search");
    write(
        &dir.join("bistill.conf"),
        "\u{feff}# desk\nbase_url = https://cwd.example.invalid # team\nusername = cwd-user\ntoken_file = ignored\npoll_seconds = 10\nstale_days = 3\nca_file = /corp/ca.pem\nlog_file = /tmp/bistill.log\nstate_dir = /var/lib/bistill\n",
    );
    let mut env = env_token();
    env.base_url = None;
    env.username = None;
    let loaded = load(&dir, &Flags::default(), &env).unwrap();
    assert_eq!(loaded.base_url, "https://cwd.example.invalid");
    assert_eq!(loaded.username, "cwd-user");
    assert_eq!(loaded.poll_seconds.get(), 15);
    assert_eq!(loaded.stale_days, 3);
    assert_eq!(loaded.ca_file, Some(PathBuf::from("/corp/ca.pem")));
    assert_eq!(loaded.log_file, Some(PathBuf::from("/tmp/bistill.log")));
    assert_eq!(loaded.state_dir, PathBuf::from("/var/lib/bistill"));

    let explicit = dir.join("chosen.conf");
    write(
        &explicit,
        "base_url = https://chosen.example.invalid\nusername = chosen\n",
    );
    let flags = Flags {
        config: Some(explicit),
        base_url: None,
        username: None,
        token_file: None,
    };
    let loaded = load(&dir, &flags, &env).unwrap();
    assert_eq!(loaded.base_url, "https://chosen.example.invalid");
    assert_eq!(loaded.username, "chosen");
}

#[test]
fn config_faults_name_the_problem() {
    let dir = temp("faults");
    let mut env = env_token();
    let missing = load(&dir, &Flags::default(), &Env::new()).unwrap_err();
    assert!(missing.to_string().contains("missing base_url"));
    let mut only_url = Env::new();
    only_url.base_url = Some("https://git.example.invalid".to_owned());
    only_url.dirs = Dirs::sample();
    let missing = load(&dir, &Flags::default(), &only_url).unwrap_err();
    assert!(missing.to_string().contains("missing username"));

    env.dirs = Dirs::new();
    let err = load(&dir, &Flags::default(), &env).unwrap_err();
    assert!(err.to_string().contains("missing state_dir"));

    let flags = Flags {
        config: Some(dir.join("missing.conf")),
        ..Flags::default()
    };
    let err = load(&dir, &flags, &env_token()).unwrap_err();
    assert!(err.to_string().contains("not a file"));

    write(
        &dir.join("bistill.conf"),
        "base_url https://git.example.invalid\n",
    );
    let err = load(&dir, &Flags::default(), &env_token()).unwrap_err();
    assert!(err.to_string().contains("missing '='"));

    write(&dir.join("bistill.conf"), "= https://git.example.invalid\n");
    let err = load(&dir, &Flags::default(), &env_token()).unwrap_err();
    assert!(err.to_string().contains("empty"));

    write(&dir.join("bistill.conf"), "base_url =\n");
    let err = load(&dir, &Flags::default(), &env_token()).unwrap_err();
    assert!(err.to_string().contains("base_url empty"));

    write(&dir.join("bistill.conf"), "port = 9\n");
    let err = load(&dir, &Flags::default(), &env_token()).unwrap_err();
    assert!(err.to_string().contains("unknown key port"));

    write(
        &dir.join("bistill.conf"),
        "base_url = https://git.example.invalid\nbase_url = https://other.example.invalid\n",
    );
    let err = load(&dir, &Flags::default(), &env_token()).unwrap_err();
    assert!(err.to_string().contains("duplicate key base_url"));

    write(
        &dir.join("bistill.conf"),
        "poll_seconds = no\nbase_url = https://git.example.invalid\nusername = jcitizen\n",
    );
    let err = load(&dir, &Flags::default(), &env_token()).unwrap_err();
    assert!(err.to_string().contains("poll_seconds not an integer"));

    write(
        &dir.join("bistill.conf"),
        "stale_days = no\nbase_url = https://git.example.invalid\nusername = jcitizen\n",
    );
    let err = load(&dir, &Flags::default(), &env_token()).unwrap_err();
    assert!(err.to_string().contains("stale_days not an integer"));

    fs::remove_file(dir.join("bistill.conf")).unwrap();
    let mut blank = env_token();
    blank.base_url = Some(String::new());
    let err = load(&dir, &Flags::default(), &blank).unwrap_err();
    assert!(err.to_string().contains("empty"));

    let mut blank = env_token();
    blank.token = Some(String::new());
    let err = load(Path::new("/absent"), &Flags::default(), &blank).unwrap_err();
    assert!(err.to_string().contains("BISTILL_TOKEN empty"));

    let flags = Flags {
        token_file: Some(PathBuf::new()),
        ..Flags::default()
    };
    let mut env = env_token();
    env.token = None;
    let err = load(Path::new("/absent"), &flags, &env).unwrap_err();
    assert!(err.to_string().contains("token_file empty"));

    env.token = None;
    env.token_file = None;
    let err = load(Path::new("/absent"), &Flags::default(), &env).unwrap_err();
    assert!(err.to_string().contains("missing token_file"));

    let bytes = dir.join("bad.conf");
    fs::write(&bytes, [0xff, 0xfe]).unwrap();
    let flags = Flags {
        config: Some(bytes),
        ..Flags::default()
    };
    let err = load(&dir, &flags, &env_token()).unwrap_err();
    assert!(err.to_string().contains("not UTF-8"));
}

#[cfg(unix)]
#[test]
fn unix_token_file_mode() {
    use std::os::unix::fs::PermissionsExt;
    let dir = temp("mode");
    let token = dir.join("token");
    write_mode(&token, "  secret-token \n", 0o644);
    let mut env = Env::new();
    env.base_url = Some("https://git.example.invalid".to_owned());
    env.username = Some("jcitizen".to_owned());
    env.dirs = Dirs::sample();
    env.token_file = Some(token.clone());
    let err = load(&dir, &Flags::default(), &env).unwrap_err();
    assert!(err.to_string().contains("group or world accessible"));

    write_mode(&token, "  secret-token \n", 0o600);
    let loaded = load(&dir, &Flags::default(), &env).unwrap();
    assert_eq!(loaded.token(), "secret-token");
    env.token_file = None;
    let flags = Flags {
        token_file: Some(token.clone()),
        ..Flags::default()
    };
    let loaded = load(&dir, &flags, &env).unwrap();
    assert_eq!(loaded.token(), "secret-token");
    write(
        &dir.join("bistill.conf"),
        &format!(
            "base_url = https://git.example.invalid\nusername = from-file\ntoken_file = {}\n",
            token.display()
        ),
    );
    env.base_url = None;
    env.username = None;
    let loaded = load(&dir, &Flags::default(), &env).unwrap();
    assert_eq!(loaded.username, "from-file");
    assert_eq!(loaded.token(), "secret-token");

    write_mode(&token, " \n", 0o600);
    let err = load(&dir, &Flags::default(), &env).unwrap_err();
    assert!(err.to_string().contains("token_file empty"));

    write_mode(&token, "secret-token", 0o000);
    let err = load(&dir, &Flags::default(), &env).unwrap_err();
    assert!(err.to_string().contains("cannot read"));
    fs::set_permissions(&token, fs::Permissions::from_mode(0o600)).unwrap();

    fs::write(dir.join("token-bytes"), [0xff]).unwrap();
    fs::set_permissions(dir.join("token-bytes"), fs::Permissions::from_mode(0o600)).unwrap();
    env.token_file = Some(dir.join("token-bytes"));
    let err = load(&dir, &Flags::default(), &env).unwrap_err();
    assert!(err.to_string().contains("not UTF-8"));

    env.token_file = Some(dir.join("absent-token"));
    let err = load(&dir, &Flags::default(), &env).unwrap_err();
    assert!(err.to_string().contains("cannot read"));
}

const APP: &str = include_str!("../../../fixtures/application-properties.json");
const USER_JSON: &str = include_str!("../../../fixtures/user.json");
const COUNT_SPLIT: &str = include_str!("../../../fixtures/inbox-count.json");
const COUNT_TOTAL: &str = include_str!("../../../fixtures/inbox-count-total.json");

fn ping_client(base: &str, username: &str) -> Client {
    let mut env = env_token();
    env.base_url = Some(base.to_owned());
    env.username = Some(username.to_owned());
    let config = load(Path::new("/no-bistill-cwd"), &Flags::default(), &env).unwrap();
    Client::new("curl", &config)
}

struct Queue {
    steps: Vec<Result<host::Response, host::Error>>,
    urls: Vec<String>,
}

impl Fetch for Queue {
    fn get(&mut self, request: &host::Request) -> Result<host::Response, host::Error> {
        self.urls.push(request.url.clone());
        self.steps.remove(0)
    }
}

fn step(status: u16, body: &str) -> Result<host::Response, host::Error> {
    Ok(host::Response {
        status,
        body: body.as_bytes().to_vec(),
        retry_after: None,
    })
}

fn run_ping(
    username: &str,
    steps: Vec<Result<host::Response, host::Error>>,
) -> (Result<Report, Error>, Vec<String>) {
    let client = ping_client("https://git.example.invalid", username);
    let mut queue = Queue {
        steps,
        urls: Vec::new(),
    };
    let result = ping_with(&client, &mut queue);
    (result, queue.urls)
}

#[test]
fn exit_codes_follow_the_spec() {
    let cases = [
        (
            Error::Curl(CurlFault::Missing {
                program: "curl".to_owned(),
            }),
            2,
        ),
        (Error::Tls("verify".to_owned()), 3),
        (
            Error::Curl(CurlFault::Timeout {
                program: "curl".to_owned(),
            }),
            4,
        ),
        (Error::Json(json::parse(b"{").unwrap_err()), 5),
        (Error::Http(401), 11),
        (Error::Http(403), 12),
        (Error::Http(404), 13),
        (Error::Http(500), 10),
        (
            Error::Curl(CurlFault::Failed {
                program: "curl".to_owned(),
                message: "exit 7".to_owned(),
            }),
            1,
        ),
        (Error::Config(ConfigFault::Missing { key: "base_url" }), 1),
        (Error::Auth("slug mismatch".to_owned()), 1),
        (Error::Io(std::io::Error::other("disk")), 1),
    ];
    for (err, code) in cases {
        assert_eq!(exit_code(&err), code, "{err}");
    }
}

#[test]
fn request_carries_bearer_agent_and_ca() {
    let dir = temp("ping-ca");
    fs::write(dir.join("bistill.conf"), "ca_file = /corp/root.pem\n").unwrap();
    let config = load(&dir, &Flags::default(), &env_token()).unwrap();
    let client = Client::new("curl", &config);
    let request = client.request("/rest/api/1.0/application-properties");
    assert_eq!(request.timeout, TIMEOUT);
    assert_eq!(request.user_agent, "bistill/0.1.0 (internal)");
    assert!(request.fail_with_body);
    assert_eq!(
        request.ca_file.as_deref(),
        Some(Path::new("/corp/root.pem"))
    );
    let text = redact_argv(&host::arguments(&request)).join(" ");
    assert!(text.contains("Bearer ***"));
    assert!(!text.contains("secret-token"));
    assert!(text.contains("--cacert"));
    assert!(text.contains("--fail-with-body"));
    assert!(text.contains("--max-time"));
    let debug = format!("{client:?}");
    assert!(debug.contains("***"));
    assert!(!debug.contains("secret-token"));
    let bare = ping_client("https://git.example.invalid", "a/b");
    assert!(bare.request("/").ca_file.is_none());
    let planned = bare.requests();
    assert_eq!(planned.len(), 4);
    assert!(planned[2].url.ends_with("/users/a%2Fb"));
    relax(&dir);
    fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn fixtures_parse_without_network() {
    let product = parse_product(APP.as_bytes()).unwrap();
    assert_eq!(product.display_name, "Bitbucket");
    assert_eq!(product.version, "8.19.0");
    let user = parse_user("Jcitizen", USER_JSON.as_bytes()).unwrap();
    assert_eq!(user.slug, "jcitizen");
    assert_eq!(user.display_name, "Jane Citizen");
    let split = parse_inbox(COUNT_SPLIT.as_bytes()).unwrap();
    assert_eq!(split.to_string(), "reviewer 2, author 1");
    let total = parse_inbox(COUNT_TOTAL.as_bytes()).unwrap();
    assert_eq!(total.to_string(), "3");
    let both = parse_inbox(br#"{"reviewer":2,"author":1,"count":9}"#).unwrap();
    assert!(matches!(
        both,
        InboxCount::Split {
            reviewer: 2,
            author: 1
        }
    ));
    let partial = parse_inbox(br#"{"reviewer":1,"count":4}"#).unwrap();
    assert!(matches!(partial, InboxCount::Total(4)));
    let zero = parse_inbox(br#"{"count":0}"#).unwrap();
    assert!(matches!(zero, InboxCount::Total(0)));
    let open = parse_inbox(br#"{"OPEN":4}"#).unwrap();
    assert!(matches!(open, InboxCount::Total(4)));
    let count_wins = parse_inbox(br#"{"count":2,"OPEN":9}"#).unwrap();
    assert!(matches!(count_wins, InboxCount::Total(2)));
    let _ = format!("{product:?} {user:?} {split:?} {total:?} {both:?} {partial:?} {zero:?}");
}

#[test]
fn parsers_reject_bad_shapes() {
    assert!(parse_product(b"{").is_err());
    assert!(parse_user("jcitizen", b"{").is_err());
    assert!(parse_inbox(b"{").is_err());
    let missing = parse_product(b"{}").unwrap_err();
    assert!(missing.to_string().contains("missing displayName"));
    let kind = parse_product(br#"{"displayName":1}"#).unwrap_err();
    assert!(kind.to_string().contains("displayName is not a string"));
    let version = parse_product(br#"{"displayName":"Bitbucket"}"#).unwrap_err();
    assert!(version.to_string().contains("missing version"));
    let slug = parse_user("jcitizen", b"{}").unwrap_err();
    assert!(slug.to_string().contains("missing slug"));
    let mismatch = parse_user("jcitizen", br#"{"slug":"other","displayName":"O"}"#).unwrap_err();
    assert!(matches!(mismatch, Error::Auth(_)));
    assert!(
        mismatch
            .to_string()
            .contains("slug other does not match username jcitizen")
    );
    let no_name = parse_user("jcitizen", br#"{"slug":"jcitizen"}"#).unwrap_err();
    assert!(no_name.to_string().contains("missing displayName"));
    let count = parse_inbox(br#"{"reviewer":1}"#).unwrap_err();
    assert!(count.to_string().contains("missing inbox count"));
    let text = parse_inbox(br#"{"count":"3"}"#).unwrap_err();
    assert!(text.to_string().contains("missing inbox count"));
}

#[test]
fn ping_reads_product_user_and_split_count() {
    let (result, urls) = run_ping(
        "Jcitizen",
        vec![
            step(302, ""),
            step(200, APP),
            step(200, USER_JSON),
            step(200, COUNT_SPLIT),
        ],
    );
    let report = result.unwrap();
    assert_eq!(report.product.version, "8.19.0");
    assert_eq!(report.user.slug, "jcitizen");
    assert_eq!(report.inbox.to_string(), "reviewer 2, author 1");
    assert_eq!(report.bodies.application_properties, APP.as_bytes());
    assert_eq!(report.bodies.user, USER_JSON.as_bytes());
    assert_eq!(report.bodies.inbox_count, COUNT_SPLIT.as_bytes());
    assert_eq!(
        urls,
        vec![
            "https://git.example.invalid/".to_owned(),
            "https://git.example.invalid/rest/api/1.0/application-properties".to_owned(),
            "https://git.example.invalid/rest/api/1.0/users/Jcitizen".to_owned(),
            "https://git.example.invalid/rest/api/1.0/inbox/pull-requests/count".to_owned(),
        ]
    );
    let _ = format!("{report:?}");
}

#[test]
fn ping_reads_a_single_count() {
    let (result, _) = run_ping(
        "jcitizen",
        vec![
            step(200, "ok"),
            step(200, APP),
            step(200, USER_JSON),
            step(200, COUNT_TOTAL),
        ],
    );
    let report = result.unwrap();
    assert!(matches!(report.inbox, InboxCount::Total(3)));
    let _ = format!("{:?}", report.inbox);
}

#[test]
fn ping_stops_when_the_origin_times_out() {
    let (result, urls) = run_ping(
        "jcitizen",
        vec![Err(host::Error::Timeout {
            program: "curl".to_owned(),
        })],
    );
    assert!(matches!(
        result,
        Err(Error::Curl(CurlFault::Timeout { .. }))
    ));
    assert_eq!(urls.len(), 1);
}

#[test]
fn ping_stops_when_application_properties_fails() {
    let (timed_out, _) = run_ping(
        "jcitizen",
        vec![
            step(200, "ok"),
            Err(host::Error::Tls {
                message: "verify".to_owned(),
            }),
        ],
    );
    assert!(matches!(timed_out, Err(Error::Tls(_))));
    let (denied, urls) = run_ping("jcitizen", vec![step(302, ""), step(401, "no")]);
    assert!(matches!(denied, Err(Error::Http(401))));
    assert_eq!(urls.len(), 2);
    let (bad, _) = run_ping("jcitizen", vec![step(200, "ok"), step(200, "{")]);
    assert!(matches!(bad, Err(Error::Json(_))));
}

#[test]
fn ping_stops_when_the_user_call_fails() {
    let (missing, urls) = run_ping("a/b", vec![step(200, "ok"), step(200, APP), step(404, "")]);
    assert!(matches!(missing, Err(Error::Http(404))));
    assert_eq!(
        urls[2],
        "https://git.example.invalid/rest/api/1.0/users/a%2Fb"
    );
    let (mismatch, seen) = run_ping(
        "jcitizen",
        vec![
            step(200, "ok"),
            step(200, APP),
            step(200, r#"{"slug":"other","displayName":"O"}"#),
        ],
    );
    assert!(matches!(mismatch, Err(Error::Auth(_))));
    assert_eq!(seen.len(), 3);
}

#[test]
fn ping_stops_when_the_count_fails() {
    let (denied, urls) = run_ping(
        "jcitizen",
        vec![
            step(200, "ok"),
            step(200, APP),
            step(200, USER_JSON),
            step(403, "no"),
        ],
    );
    assert!(matches!(denied, Err(Error::Http(403))));
    assert_eq!(urls.len(), 4);
    let (bad, _) = run_ping(
        "jcitizen",
        vec![
            step(200, "ok"),
            step(200, APP),
            step(200, USER_JSON),
            step(200, "{"),
        ],
    );
    assert!(matches!(bad, Err(Error::Json(_))));
}

#[test]
fn ping_through_curl_reads_a_local_server() {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
    let port = listener.local_addr().expect("addr").port();
    listener.set_nonblocking(true).expect("nonblocking");
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
                        APP.as_bytes()
                    } else if req.contains("/users/") {
                        USER_JSON.as_bytes()
                    } else if req.contains("pull-requests/count") {
                        COUNT_SPLIT.as_bytes()
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
    let client = ping_client(&format!("http://127.0.0.1:{port}"), "jcitizen");
    let report = ping(&client).unwrap_or_else(|err| panic!("{err}"));
    assert_eq!(report.product.display_name, "Bitbucket");
    assert_eq!(report.user.display_name, "Jane Citizen");
    assert_eq!(report.inbox.to_string(), "reviewer 2, author 1");
    thread.join().expect("server");
}

const REVIEWER_PAGE: &str = include_str!("../../../fixtures/inbox-reviewer.json");
const AUTHOR_PAGE: &str = include_str!("../../../fixtures/inbox-author.json");
const NOW_MS: u64 = 1_700_000_000_000;

#[test]
fn inbox_fixtures_split_into_the_two_sections() {
    let reviewer = parse_page(REVIEWER_PAGE.as_bytes()).unwrap();
    let author = parse_page(AUTHOR_PAGE.as_bytes()).unwrap();
    let _ = format!("{reviewer:?} {author:?}");
    let _ = format!("{:?}", reviewer.values[3].state);
    let _ = format!("{:?}", reviewer.values[4].state);
    let _ = format!("{:?}", reviewer.values[5].reviewers[0].status);
    assert_eq!(reviewer.size, 6);
    assert!(matches!(
        reviewer.end,
        PageEnd::More {
            next_page_start: 25
        }
    ));
    assert!(matches!(author.end, PageEnd::Last));
    let mut prs = reviewer.values;
    prs.extend(author.values);
    let sections = classify(&prs, "Jcitizen", "https://git.example.invalid", NOW_MS, 7);
    let needs: Vec<_> = sections
        .needs_review
        .iter()
        .map(|row| row.title.as_str())
        .collect();
    assert_eq!(
        needs,
        [
            "Already approved",
            "Draft the pipe",
            "Quiet draft",
            "Fix the pipe",
            "Both roles"
        ]
    );
    assert!(
        sections
            .needs_review
            .iter()
            .all(|row| !row.stale && !row.needs_work)
    );
    assert!(!sections.needs_review[0].draft);
    assert_eq!(
        sections.needs_review[0].reviewers[0].status,
        ReviewStatus::Approved
    );
    assert!(sections.needs_review[1].draft);
    assert_eq!(
        sections.needs_review[1].html_url,
        "https://git.example.invalid/projects/PRJ/repos/repo/pull-requests/13"
    );
    assert!(sections.needs_review[2].draft);
    assert_eq!(
        sections.needs_review[2].reviewers[0].status,
        ReviewStatus::NeedsWork
    );
    assert!(!sections.needs_review[3].draft);
    assert_eq!(
        sections.needs_review[3].html_url,
        "https://git.example.invalid/projects/PRJ/repos/repo/pull-requests/12"
    );
    let waiting: Vec<_> = sections.waiting.iter().map(|row| row.id.as_str()).collect();
    assert_eq!(waiting, ["PRJ/pipe/21", "~jcitizen/mine/3"]);
    assert!(sections.waiting[0].stale && sections.waiting[0].needs_work);
    assert!(
        !sections.waiting[1].stale && !sections.waiting[1].needs_work && !sections.waiting[1].draft
    );
    assert_eq!(
        sections.waiting[1].html_url,
        "https://git.example.invalid/projects/~jcitizen/repos/mine/pull-requests/3"
    );
    assert!(
        sections
            .needs_review
            .iter()
            .all(|row| row.id != "PRJ/repo/15")
    );
    assert!(
        sections
            .waiting
            .iter()
            .all(|row| row.title != "Already merged")
    );
    assert!(
        sections
            .needs_review
            .iter()
            .chain(sections.waiting.iter())
            .all(|row| row.title != "Already declined")
    );
    let _ = format!("{sections:?}");
}

#[test]
fn classify_drops_duplicates_and_bounds_stale() {
    let first = authored(9, "First", "repo", "PRJ", NOW_MS, r#""draft":false"#);
    let second = authored(9, "Second", "repo", "PRJ", NOW_MS, r#""draft":false"#);
    let page =
        parse_page(page_body(&format!("{first},{second}"), "true", None).as_bytes()).unwrap();
    let sections = classify(
        &page.values,
        "jcitizen",
        "https://git.example.invalid",
        NOW_MS,
        7,
    );
    assert_eq!(sections.waiting.len(), 1);
    assert_eq!(sections.waiting[0].title, "First");
    let day = 86_400_000;
    let exact = authored(
        1,
        "Exact",
        "repo",
        "PRJ",
        NOW_MS - 7 * day,
        r#""properties":{"draft":"false"}"#,
    );
    let just = authored(
        2,
        "Just",
        "a/b",
        "PRJ",
        NOW_MS - 7 * day - 1,
        r#""properties":{"draft":"true"}"#,
    );
    let page = parse_page(page_body(&format!("{exact},{just}"), "true", None).as_bytes()).unwrap();
    let sections = classify(
        &page.values,
        "jcitizen",
        "https://git.example.invalid",
        NOW_MS,
        7,
    );
    assert!(sections.waiting[0].stale && sections.waiting[0].draft);
    assert!(!sections.waiting[1].stale && !sections.waiting[1].draft);
    assert_eq!(
        sections.waiting[0].html_url,
        "https://git.example.invalid/projects/PRJ/repos/a%2Fb/pull-requests/2"
    );
    let other = authored(3, "Other", "repo", "PRJ", NOW_MS, r#""draft":false"#)
        .replace(r#""slug":"jcitizen""#, r#""slug":"other""#);
    let page = parse_page(page_body(&other, "true", None).as_bytes()).unwrap();
    let sections = classify(
        &page.values,
        "jcitizen",
        "https://git.example.invalid",
        NOW_MS,
        7,
    );
    assert!(sections.needs_review.is_empty());
    assert!(sections.waiting.is_empty());
}

#[test]
fn inbox_pages_reject_bad_shapes() {
    assert!(parse_page(b"{").is_err());
    let cases = vec![
        (r#"{"isLastPage":true,"values":[]}"#.to_owned(), "missing size"),
        (r#"{"size":1,"values":[]}"#.to_owned(), "missing isLastPage"),
        (
            r#"{"size":1,"isLastPage":false,"values":[]}"#.to_owned(),
            "missing nextPageStart",
        ),
        (
            r#"{"size":1,"isLastPage":"yes","values":[]}"#.to_owned(),
            "isLastPage is not a boolean",
        ),
        (
            r#"{"size":"1","isLastPage":true,"values":[]}"#.to_owned(),
            "size is not an integer",
        ),
        (r#"{"size":0,"isLastPage":true}"#.to_owned(), "missing values"),
        (
            r#"{"size":0,"isLastPage":true,"values":{}}"#.to_owned(),
            "values is not an array",
        ),
        (page_body(r#"{"id":1}"#, "true", None), "missing fromRef"),
        (
            page_body(
                &authored(1, "T", "repo", "PRJ", 1, r#""draft":false"#)
                    .replace(r#""title":"T","#, ""),
                "true",
                None,
            ),
            "missing title",
        ),
        (
            page_body(&authored(1, "T", "repo", "PRJ", 1, r#""draft":1"#), "true", None),
            "draft is not a boolean",
        ),
        (
            page_body(
                &authored(1, "T", "repo", "PRJ", 1, r#""properties":{"draft":"yes"}"#),
                "true",
                None,
            ),
            "draft is not a boolean",
        ),
        (
            page_body(
                &authored(1, "T", "repo", "PRJ", 1, r#""properties":{"draft":1}"#),
                "true",
                None,
            ),
            "draft is not a boolean",
        ),
        (
            page_body(
                &authored(1, "T", "repo", "PRJ", 1, r#""reviewers":{}"#),
                "true",
                None,
            ),
            "reviewers is not an array",
        ),
        (
            page_body(
                &authored(1, "T", "repo", "PRJ", 1, r#""state":"CLOSED""#),
                "true",
                None,
            ),
            "unknown state",
        ),
        (
            page_body(
                &authored(1, "T", "repo", "PRJ", 1, r#""title":1"#).replace(r#""title":"T""#, r#""title":1"#),
                "true",
                None,
            ),
            "title is not a string",
        ),
        (
            r#"{"size":1,"isLastPage":true,"values":[{"id":1,"title":"T","state":"OPEN","createdDate":1,"updatedDate":1,"fromRef":{"repository":{"slug":"repo","project":{"key":"PRJ"}}},"toRef":{"displayId":"main"},"author":{"user":{"displayName":"Pat","slug":"pat"}}}]}"#.to_owned(),
            "missing displayId",
        ),
    ];
    for (body, needle) in cases {
        let err = parse_page(body.as_bytes()).unwrap_err();
        assert!(err.to_string().contains(needle), "{err} / {needle}");
    }
    let weird = authored(
        1,
        "T",
        "repo",
        "PRJ",
        1,
        r#""reviewers":[{"user":{"displayName":"Sam","slug":"sam"},"status":"MAYBE"}]"#,
    );
    let err = parse_page(page_body(&weird, "true", None).as_bytes()).unwrap_err();
    assert!(err.to_string().contains("unknown status"), "{err}");
    let bare = authored(4, "Bare", "repo", "PRJ", 1, r#""properties":{}"#);
    let page = parse_page(page_body(&bare, "true", None).as_bytes()).unwrap();
    assert!(!page.values[0].draft);
    let empty = parse_page(br#"{"size":0,"isLastPage":true,"values":[]}"#).unwrap();
    let sections = classify(
        &empty.values,
        "jcitizen",
        "https://git.example.invalid",
        NOW_MS,
        7,
    );
    assert!(sections.needs_review.is_empty() && sections.waiting.is_empty());
    let _ = format!("{:?}", empty.end);
}

fn page_body(values: &str, last: &str, next: Option<u64>) -> String {
    let next = match next {
        Some(start) => format!(r#","nextPageStart":{start}"#),
        None => String::new(),
    };
    format!(r#"{{"size":1,"isLastPage":{last}{next},"values":[{values}]}}"#)
}

const EMPTY_PAGE: &str = r#"{"size":0,"isLastPage":true,"values":[]}"#;

fn quiet_activity(rows: usize) -> Vec<Result<host::Response, host::Error>> {
    let mut steps = Vec::new();
    for _ in 0..rows {
        steps.push(step(200, EMPTY_PAGE));
    }
    steps
}

fn quiet_author(rows: usize) -> Vec<Result<host::Response, host::Error>> {
    let mut steps = Vec::new();
    for _ in 0..rows {
        steps.push(step(200, EMPTY_PAGE));
        steps.push(step(200, r#"{"count":0}"#));
        steps.push(step(200, r#"{"conflicted":false,"canMerge":false}"#));
    }
    steps
}

fn run_list(
    steps: Vec<Result<host::Response, host::Error>>,
) -> (Result<Listed, Error>, Vec<String>) {
    let client = ping_client("https://git.example.invalid", "jcitizen");
    let mut queue = Queue {
        steps,
        urls: Vec::new(),
    };
    let result = list_inbox(&client, &mut queue, NOW_MS, &mut |_| Ok(()));
    (result, queue.urls)
}

#[test]
fn list_inbox_pages_both_roles_and_round_trips() {
    let mut steps = vec![
        step(200, APP),
        step(200, USER_JSON),
        step(200, REVIEWER_PAGE),
        step(200, EMPTY_PAGE),
        step(200, AUTHOR_PAGE),
    ];
    steps.extend(quiet_activity(5));
    steps.extend(quiet_author(2));
    let (listed, urls) = run_list(steps);
    let mut listed = listed.unwrap();
    assert!(urls.iter().any(|url| {
        url.contains("/projects/~jcitizen/repos/mine/pull-requests/3/activities?start=0&limit=25")
    }));
    assert!(urls[2].contains("role=REVIEWER&start=0&limit=25"));
    assert!(urls[3].contains("role=REVIEWER&start=25&limit=25"));
    assert!(urls[4].contains("role=AUTHOR&start=0&limit=25"));
    let needs: Vec<_> = listed
        .snapshot
        .needs_review
        .iter()
        .map(|row| row.id.as_str())
        .collect();
    assert_eq!(
        needs,
        [
            "PRJ/repo/17",
            "PRJ/repo/13",
            "PRJ/repo/14",
            "PRJ/repo/12",
            "PRJ/repo/22"
        ]
    );
    let waiting: Vec<_> = listed
        .snapshot
        .waiting
        .iter()
        .map(|row| row.id.as_str())
        .collect();
    assert_eq!(waiting, ["PRJ/pipe/21", "~jcitizen/mine/3"]);
    assert_eq!(listed.snapshot.truncated, 0);
    assert_eq!(listed.snapshot.poll_seconds, 60);
    assert_eq!(listed.snapshot.user_name, "Jane Citizen");
    assert_eq!(listed.snapshot.bitbucket_version, "8.19.0");
    assert_eq!(attention_count(&listed.snapshot), 5);
    listed.snapshot.needs_review[0].open_tasks = 9;
    assert_eq!(attention_count(&listed.snapshot), 5);
    listed.snapshot.waiting[0].open_tasks = 1;
    assert_eq!(attention_count(&listed.snapshot), 6);
    listed.snapshot.waiting[0].open_tasks = 0;
    listed.snapshot.waiting[0].unanswered_as_author = 2;
    assert_eq!(attention_count(&listed.snapshot), 6);
    let text = to_json(&listed.snapshot);
    let parsed = json::parse(text.as_bytes()).unwrap();
    assert_eq!(json::to_vec(&parsed), text.as_bytes());
    assert!(text.contains("\"enrichment\":\"ready\""));
    assert!(text.contains("\"build\":\"none\""));
    assert!(text.contains("\"fingerprint\":\""));
    let _ = format!("{:?}", listed.snapshot);
}

#[test]
fn list_inbox_retries_lowercase_and_records_the_cap() {
    let (empty, urls) = run_list(vec![
        step(200, APP),
        step(200, USER_JSON),
        step(400, "no"),
        step(200, EMPTY_PAGE),
        step(200, EMPTY_PAGE),
    ]);
    let empty = empty.unwrap();
    assert!(urls[2].contains("role=REVIEWER"));
    assert!(urls[3].contains("role=reviewer"));
    assert!(empty.snapshot.needs_review.is_empty());
    assert!(empty.snapshot.waiting.is_empty());
    assert_eq!(attention_count(&empty.snapshot), 0);
    let (again, _) = run_list(vec![
        step(200, APP),
        step(200, USER_JSON),
        step(400, "no"),
        step(400, "no"),
    ]);
    assert!(matches!(again, Err(Error::Http(400))));
    let stuck = r#"{"size":0,"isLastPage":false,"nextPageStart":0,"values":[]}"#;
    let (stalled, _) = run_list(vec![step(200, APP), step(200, USER_JSON), step(200, stuck)]);
    let Err(stalled) = stalled else {
        panic!("expected nextPageStart");
    };
    assert!(stalled.to_string().contains("nextPageStart"));
    let mut values = Vec::new();
    for id in 1..=51 {
        values.push(format!(
            r#"{{"id":{id},"title":"T{id}","state":"OPEN","createdDate":1,"updatedDate":{id},"fromRef":{{"displayId":"f","repository":{{"slug":"repo","project":{{"key":"PRJ"}}}}}},"toRef":{{"displayId":"main"}},"author":{{"user":{{"displayName":"Pat","slug":"pat"}}}},"reviewers":[{{"user":{{"displayName":"Jane Citizen","slug":"jcitizen"}},"status":"UNAPPROVED"}}]}}"#
        ));
    }
    let page = format!(
        r#"{{"size":51,"isLastPage":true,"values":[{}]}}"#,
        values.join(",")
    );
    let mut steps = vec![
        step(200, APP),
        step(200, USER_JSON),
        step(200, &page),
        step(200, EMPTY_PAGE),
    ];
    steps.extend(quiet_activity(51));
    let (capped, urls) = run_list(steps);
    let capped = capped.unwrap();
    assert_eq!(capped.snapshot.needs_review.len(), 51);
    assert_eq!(capped.snapshot.truncated, 0);
    assert!(capped.snapshot.waiting.is_empty());
    assert_eq!(
        urls.iter()
            .filter(|url| url.contains("/activities"))
            .count(),
        51
    );
    assert!(urls.iter().any(|url| url.contains("/pull-requests/51/")));
}

#[test]
fn list_inbox_stops_on_the_failing_get() {
    let (timeout, _) = run_list(vec![Err(host::Error::Timeout {
        program: "curl".to_owned(),
    })]);
    assert!(matches!(
        timeout,
        Err(Error::Curl(CurlFault::Timeout { .. }))
    ));
    let (denied, _) = run_list(vec![step(401, "no")]);
    assert!(matches!(denied, Err(Error::Http(401))));
    let (bad_product, _) = run_list(vec![step(200, "{")]);
    assert!(matches!(bad_product, Err(Error::Json(_))));
    let (bad_user, _) = run_list(vec![step(200, APP), step(200, "{")]);
    assert!(matches!(bad_user, Err(Error::Json(_))));
    let (user_http, _) = run_list(vec![step(200, APP), step(404, "no")]);
    assert!(matches!(user_http, Err(Error::Http(404))));
    let (bad_page, _) = run_list(vec![step(200, APP), step(200, USER_JSON), step(200, "{")]);
    assert!(matches!(bad_page, Err(Error::Json(_))));
    let (author_http, _) = run_list(vec![
        step(200, APP),
        step(200, USER_JSON),
        step(200, EMPTY_PAGE),
        step(500, "no"),
    ]);
    assert!(matches!(author_http, Err(Error::Http(500))));
    let page = page_body(&listed_pr(1, "pat", true, None), "true", None);
    let (enrich_http, _) = run_list(vec![
        step(200, APP),
        step(200, USER_JSON),
        step(200, &page),
        step(200, EMPTY_PAGE),
        step(500, "no"),
    ]);
    assert!(matches!(enrich_http, Err(Error::Http(500))));
}

fn authored(id: u64, title: &str, repo: &str, project: &str, updated: u64, extra: &str) -> String {
    format!(
        r#"{{"id":{id},"title":"{title}","state":"OPEN","createdDate":1,"updatedDate":{updated},"fromRef":{{"displayId":"feature","repository":{{"slug":"{repo}","project":{{"key":"{project}"}}}}}},"toRef":{{"displayId":"main"}},"author":{{"user":{{"displayName":"Jane Citizen","slug":"jcitizen"}}}},{extra}}}"#
    )
}

fn person(slug: &str, status: ReviewStatus) -> Reviewer {
    Reviewer {
        name: slug.to_owned(),
        slug: slug.to_owned(),
        status,
    }
}

fn finger_row(id: &str, updated: u64, reviewers: Vec<Reviewer>) -> Row {
    Row {
        id: id.to_owned(),
        project: "PRJ".to_owned(),
        repo: "repo".to_owned(),
        number: 1,
        title: "T".to_owned(),
        author: "Pat".to_owned(),
        from_branch: "feature".to_owned(),
        to_branch: "main".to_owned(),
        reviewers,
        created_ms: 1,
        updated_ms: updated,
        html_url: "https://git.example.invalid/pull/1".to_owned(),
        draft: false,
        stale: false,
        needs_work: false,
        unanswered_as_author: 0,
        unanswered_as_reviewer: 0,
        open_tasks: 0,
        enrichment: Enrichment::Ready,
        build: Build::None,
        conflicted: false,
        can_merge: false,
        fingerprint: String::new(),
        events: Vec::new(),
        events_loaded: false,
    }
}

fn finger_snapshot(slug: &str, needs_review: Vec<Row>, waiting: Vec<Row>) -> Snapshot {
    let mut snapshot = Snapshot {
        fetched_ms: 1,
        user_slug: slug.to_owned(),
        user_name: "Jane Citizen".to_owned(),
        bitbucket_version: "8.19.0".to_owned(),
        bitbucket_name: "Bitbucket".to_owned(),
        status: SnapshotStatus::Ok,
        status_since_ms: 1,
        needs_review,
        waiting,
        truncated: 0,
        poll_seconds: 60,
    };
    stamp(&mut snapshot);
    snapshot
}

#[test]
fn toast_body_lists_english_reasons_and_the_link() {
    let change = Change {
        id: "PRJ/repo/12".to_owned(),
        html_url: "https://git.example.invalid/pull/12".to_owned(),
        reasons: vec![
            Reason::NeedsReview,
            Reason::Waiting,
            Reason::Unanswered,
            Reason::Tasks,
            Reason::Approved,
            Reason::NeedsWork,
            Reason::BuildFailed,
            Reason::Gone,
        ],
        gone_text: String::new(),
    };
    assert_eq!(
        toast_body(&change),
        "PRJ/repo#12 needs review, waiting, unanswered comments, open tasks, approved, needs work, build failed, merged or declined\nhttps://git.example.invalid/pull/12"
    );
    let mut merged = change;
    merged.reasons = vec![Reason::Gone];
    merged.gone_text = "merged".to_owned();
    assert!(toast_body(&merged).contains("merged\n"));
    merged.gone_text = "declined".to_owned();
    assert!(toast_body(&merged).contains("declined\n"));
    let _ = format!("{merged:?}");
    let bare = Change {
        id: "12".to_owned(),
        html_url: "https://git.example.invalid/pull/12".to_owned(),
        reasons: vec![Reason::NeedsReview],
        gone_text: String::new(),
    };
    assert_eq!(
        toast_body(&bare),
        "12 needs review\nhttps://git.example.invalid/pull/12"
    );
    assert_eq!(TITLE, "Bistill");
}

fn change_tokens(changes: &[Change]) -> Vec<(String, Vec<&str>)> {
    changes
        .iter()
        .map(|change| {
            (
                change.id.clone(),
                change.reasons.iter().map(|reason| reason.token()).collect(),
            )
        })
        .collect()
}

#[test]
fn fingerprint_diff_and_snapshot_file() {
    let pending = finger_row(
        "PRJ/repo/5",
        5,
        vec![person("jcitizen", ReviewStatus::Approved)],
    );
    let mut pending = finger_snapshot("jcitizen", Vec::new(), vec![pending]);
    pending.waiting[0].enrichment = Enrichment::Pending;
    stamp(&mut pending);
    assert_eq!(
        pending.waiting[0].fingerprint,
        "350a77616974696e670a415050524f5645440a6a636974697a656e20415050524f5645440a"
    );
    let mut folded = finger_snapshot(
        "Jcitizen",
        Vec::new(),
        vec![finger_row(
            "PRJ/repo/5",
            5,
            vec![person("jcitizen", ReviewStatus::Approved)],
        )],
    );
    folded.waiting[0].enrichment = Enrichment::Pending;
    stamp(&mut folded);
    assert_eq!(
        folded.waiting[0].fingerprint,
        pending.waiting[0].fingerprint
    );
    let text = to_json(&pending);
    assert!(!text.contains("unanswered_as_author"));
    let loaded = parse_snapshot(text.as_bytes()).unwrap();
    assert_eq!(loaded.waiting[0].enrichment, Enrichment::Pending);
    assert_eq!(loaded.waiting[0].open_tasks, 0);

    let needs = finger_row(
        "PRJ/repo/1",
        1,
        vec![person("jcitizen", ReviewStatus::Unapproved)],
    );
    let waiting = finger_row(
        "PRJ/repo/2",
        2,
        vec![person("sam", ReviewStatus::Unapproved)],
    );
    let current = finger_snapshot("jcitizen", vec![needs], vec![waiting]);
    assert_eq!(
        change_tokens(&diff(None, &current)),
        vec![
            ("PRJ/repo/1".to_owned(), vec!["needs_review"]),
            ("PRJ/repo/2".to_owned(), vec!["waiting"]),
        ]
    );
    assert!(diff(Some(&current), &current).is_empty());
    let kept = finger_snapshot(
        "jcitizen",
        vec![finger_row(
            "PRJ/repo/1",
            1,
            vec![person("jcitizen", ReviewStatus::Unapproved)],
        )],
        Vec::new(),
    );
    let gone = diff(Some(&current), &kept);
    assert_eq!(
        change_tokens(&gone),
        vec![("PRJ/repo/2".to_owned(), vec!["gone"])]
    );
    assert_eq!(gone[0].html_url, "https://git.example.invalid/pull/1");
    assert_eq!(
        toast_body(&gone[0]),
        "PRJ/repo#2 merged or declined\nhttps://git.example.invalid/pull/1"
    );
    let same_needs = || {
        finger_row(
            "PRJ/repo/1",
            1,
            vec![person("jcitizen", ReviewStatus::Unapproved)],
        )
    };
    let approved = finger_snapshot(
        "jcitizen",
        vec![same_needs()],
        vec![finger_row(
            "PRJ/repo/2",
            2,
            vec![person("sam", ReviewStatus::Approved)],
        )],
    );
    assert_eq!(
        change_tokens(&diff(Some(&current), &approved)),
        vec![("PRJ/repo/2".to_owned(), vec!["approved"])]
    );
    let added = finger_snapshot(
        "jcitizen",
        vec![same_needs()],
        vec![finger_row(
            "PRJ/repo/2",
            2,
            vec![
                person("sam", ReviewStatus::Unapproved),
                person("alex", ReviewStatus::Approved),
            ],
        )],
    );
    assert_eq!(
        change_tokens(&diff(Some(&current), &added)),
        vec![("PRJ/repo/2".to_owned(), vec!["approved"])]
    );
    let needs_work = finger_snapshot(
        "jcitizen",
        vec![finger_row(
            "PRJ/repo/1",
            1,
            vec![person("jcitizen", ReviewStatus::NeedsWork)],
        )],
        vec![finger_row(
            "PRJ/repo/2",
            2,
            vec![person("sam", ReviewStatus::Unapproved)],
        )],
    );
    assert_eq!(
        change_tokens(&diff(Some(&current), &needs_work)),
        vec![("PRJ/repo/1".to_owned(), vec!["needs_work"])]
    );
    let mut reviewer_count = finger_row(
        "PRJ/repo/2",
        2,
        vec![person("sam", ReviewStatus::Unapproved)],
    );
    reviewer_count.unanswered_as_reviewer = 1;
    let reviewer_count = finger_snapshot("jcitizen", vec![same_needs()], vec![reviewer_count]);
    assert_eq!(
        change_tokens(&diff(Some(&current), &reviewer_count)),
        vec![("PRJ/repo/2".to_owned(), vec!["unanswered"])]
    );
    let mut author_count = finger_row(
        "PRJ/repo/2",
        2,
        vec![person("sam", ReviewStatus::Unapproved)],
    );
    author_count.unanswered_as_author = 3;
    author_count.open_tasks = 2;
    author_count.build = Build::Failed;
    let author_count = finger_snapshot("jcitizen", vec![same_needs()], vec![author_count]);
    assert_eq!(
        change_tokens(&diff(Some(&current), &author_count)),
        vec![(
            "PRJ/repo/2".to_owned(),
            vec!["unanswered", "tasks", "build_failed"]
        )]
    );
    let mut failed = finger_row("PRJ/repo/2", 2, vec![person("sam", ReviewStatus::Approved)]);
    failed.build = Build::Failed;
    failed.conflicted = true;
    let failed_prev = finger_snapshot("jcitizen", Vec::new(), vec![failed]);
    let mut failed_next = finger_row("PRJ/repo/2", 2, vec![person("sam", ReviewStatus::Approved)]);
    failed_next.build = Build::Failed;
    failed_next.conflicted = true;
    let failed_next = finger_snapshot("jcitizen", Vec::new(), vec![failed_next]);
    assert!(diff(Some(&failed_prev), &failed_next).is_empty());
    let _ = format!(
        "{:?} {:?} {:?} {:?} {:?}",
        diff(Some(&current), &author_count),
        Section::NeedsReview,
        Section::Waiting,
        Enrichment::Pending,
        Build::Successful
    );

    let dir = temp("finger-disk");
    write_snapshot(&dir, &current).unwrap();
    let stored = read_snapshot(&dir).unwrap().unwrap();
    assert!(diff(Some(&stored), &current).is_empty());
    assert!(read_snapshot(&temp("finger-missing")).unwrap().is_none());
    let blocked = temp("finger-blocked");
    let file = blocked.join("blocked");
    fs::write(&file, "x").unwrap();
    assert!(write_snapshot(&file, &current).is_err());
    let occupied = temp("finger-occupied");
    fs::create_dir(occupied.join("snapshot.json")).unwrap();
    assert!(write_snapshot(&occupied, &current).is_err());
    assert!(read_snapshot(&occupied).is_err());
    let bad = temp("finger-bad");
    fs::write(bad.join("snapshot.json"), "{").unwrap();
    assert!(matches!(read_snapshot(&bad), Err(Error::Json(_))));
    relax(&dir);
    fs::remove_dir_all(&dir).unwrap();
    relax(&blocked);
    fs::remove_dir_all(&blocked).unwrap();
    relax(&occupied);
    fs::remove_dir_all(&occupied).unwrap();
    relax(&bad);
    fs::remove_dir_all(&bad).unwrap();
}

#[test]
fn snapshot_json_rejects_bad_shapes_and_keeps_status() {
    let mut marked = finger_row(
        "PRJ/repo/1",
        1,
        vec![person("jcitizen", ReviewStatus::Unapproved)],
    );
    marked.can_merge = true;
    let snapshot = finger_snapshot("jcitizen", vec![marked], Vec::new());
    let base = json::parse(to_json(&snapshot).as_bytes()).unwrap();
    for key in [
        "fetched_ms",
        "user",
        "bitbucket",
        "status",
        "status_since_ms",
        "needs_review",
        "waiting",
        "truncated",
        "poll_seconds",
    ] {
        let mut value = base.clone();
        remove_key(&mut value, key);
        let err = parse_snapshot(&json::to_vec(&value)).unwrap_err();
        assert!(err.to_string().contains(key), "{err} / {key}");
    }
    for key in ["slug", "display_name"] {
        let mut value = base.clone();
        remove_key(top(&mut value, "user"), key);
        let err = parse_snapshot(&json::to_vec(&value)).unwrap_err();
        assert!(err.to_string().contains(key), "{err}");
    }
    for key in ["version", "display_name"] {
        let mut value = base.clone();
        remove_key(top(&mut value, "bitbucket"), key);
        let err = parse_snapshot(&json::to_vec(&value)).unwrap_err();
        assert!(err.to_string().contains(key), "{err}");
    }
    for key in [
        "id",
        "project",
        "repo",
        "number",
        "title",
        "author",
        "from_branch",
        "to_branch",
        "reviewers",
        "created_ms",
        "updated_ms",
        "html_url",
        "draft",
        "enrichment",
        "stale",
        "needs_work",
        "unanswered_as_author",
        "unanswered_as_reviewer",
        "open_tasks",
        "build",
        "conflicted",
        "can_merge",
        "fingerprint",
    ] {
        let mut value = base.clone();
        remove_pair(row_fields(&mut value), key);
        let err = parse_snapshot(&json::to_vec(&value)).unwrap_err();
        assert!(err.to_string().contains(key), "{err} / {key}");
    }
    for key in ["name", "slug", "status"] {
        let mut value = base.clone();
        remove_pair(reviewer_fields(&mut value), key);
        let err = parse_snapshot(&json::to_vec(&value)).unwrap_err();
        assert!(err.to_string().contains(key), "{err} / {key}");
    }
    let mut value = base.clone();
    replace(top(&mut value, "user"), json::Value::Array(Vec::new()));
    assert!(
        parse_snapshot(&json::to_vec(&value))
            .unwrap_err()
            .to_string()
            .contains("not an object")
    );
    let mut value = base.clone();
    replace(
        top(&mut value, "needs_review"),
        json::Value::Object(Vec::new()),
    );
    assert!(
        parse_snapshot(&json::to_vec(&value))
            .unwrap_err()
            .to_string()
            .contains("not an array")
    );
    let mut value = base.clone();
    replace(
        top(&mut value, "fetched_ms"),
        json::Value::String("x".to_owned()),
    );
    assert!(
        parse_snapshot(&json::to_vec(&value))
            .unwrap_err()
            .to_string()
            .contains("not an integer")
    );
    let mut value = base.clone();
    replace(
        row_fields_value(&mut value, "title"),
        json::Value::Bool(true),
    );
    assert!(
        parse_snapshot(&json::to_vec(&value))
            .unwrap_err()
            .to_string()
            .contains("not a string")
    );
    let mut value = base.clone();
    replace(
        row_fields_value(&mut value, "draft"),
        json::Value::String("x".to_owned()),
    );
    assert!(
        parse_snapshot(&json::to_vec(&value))
            .unwrap_err()
            .to_string()
            .contains("not a boolean")
    );
    let mut value = base.clone();
    replace(
        row_fields_value(&mut value, "fingerprint"),
        json::Value::String("00".to_owned()),
    );
    assert!(
        parse_snapshot(&json::to_vec(&value))
            .unwrap_err()
            .to_string()
            .contains("fingerprint mismatch")
    );
    assert!(
        parse_snapshot(b"[]")
            .unwrap_err()
            .to_string()
            .contains("not an object")
    );
    assert!(parse_snapshot(b"{").is_err());
    let mut value = base.clone();
    replace(
        top(&mut value, "needs_review"),
        json::Value::Array(vec![json::Value::Bool(true)]),
    );
    assert!(
        parse_snapshot(&json::to_vec(&value))
            .unwrap_err()
            .to_string()
            .contains("row is not an object")
    );
    let mut value = base.clone();
    replace(
        row_fields_value(&mut value, "reviewers"),
        json::Value::Object(Vec::new()),
    );
    assert!(
        parse_snapshot(&json::to_vec(&value))
            .unwrap_err()
            .to_string()
            .contains("not an array")
    );
    let mut value = base.clone();
    replace(
        row_fields_value(&mut value, "reviewers"),
        json::Value::Array(vec![json::Value::Bool(true)]),
    );
    assert!(
        parse_snapshot(&json::to_vec(&value))
            .unwrap_err()
            .to_string()
            .contains("reviewer is not an object")
    );
    let mut value = base.clone();
    replace(
        top(&mut value, "status"),
        json::Value::String("nope".to_owned()),
    );
    assert!(
        parse_snapshot(&json::to_vec(&value))
            .unwrap_err()
            .to_string()
            .contains("unknown status")
    );
    let mut value = base.clone();
    replace(
        row_fields_value(&mut value, "enrichment"),
        json::Value::String("later".to_owned()),
    );
    assert!(
        parse_snapshot(&json::to_vec(&value))
            .unwrap_err()
            .to_string()
            .contains("unknown enrichment")
    );
    let mut value = base.clone();
    replace(
        row_fields_value(&mut value, "build"),
        json::Value::String("nope".to_owned()),
    );
    assert!(
        parse_snapshot(&json::to_vec(&value))
            .unwrap_err()
            .to_string()
            .contains("unknown build")
    );
    let mut value = base.clone();
    replace(
        reviewer_value(&mut value, "status"),
        json::Value::String("MAYBE".to_owned()),
    );
    assert!(
        parse_snapshot(&json::to_vec(&value))
            .unwrap_err()
            .to_string()
            .contains("unknown status")
    );

    for status in [
        SnapshotStatus::Fetching,
        SnapshotStatus::Ok,
        SnapshotStatus::Unreachable,
        SnapshotStatus::Auth,
        SnapshotStatus::Tls,
        SnapshotStatus::RateLimited,
        SnapshotStatus::Error,
    ] {
        let mut copy = finger_snapshot(
            "jcitizen",
            vec![finger_row(
                "PRJ/repo/1",
                1,
                vec![person("jcitizen", ReviewStatus::Unapproved)],
            )],
            Vec::new(),
        );
        copy.status = status;
        let loaded = parse_snapshot(to_json(&copy).as_bytes()).unwrap();
        assert_eq!(loaded.status, status);
        let _ = format!("{status:?}");
    }
    for build in [
        Build::None,
        Build::Successful,
        Build::InProgress,
        Build::Failed,
    ] {
        let mut row = finger_row(
            "PRJ/repo/1",
            1,
            vec![person("jcitizen", ReviewStatus::Unapproved)],
        );
        row.build = build;
        let copy = finger_snapshot("jcitizen", vec![row], Vec::new());
        let loaded = parse_snapshot(to_json(&copy).as_bytes()).unwrap();
        assert_eq!(loaded.needs_review[0].build, build);
        let _ = format!("{build:?}");
        assert!(to_json(&copy).contains(match build {
            Build::None => "none",
            Build::Successful => "successful",
            Build::InProgress => "in_progress",
            Build::Failed => "failed",
        }));
    }
}

fn remove_key(value: &mut json::Value, key: &str) {
    if let json::Value::Object(pairs) = value {
        remove_pair(pairs, key);
    }
}

fn remove_pair(pairs: &mut Vec<(String, json::Value)>, key: &str) {
    pairs.retain(|(name, _)| name != key);
}

fn top<'a>(value: &'a mut json::Value, key: &str) -> &'a mut json::Value {
    let json::Value::Object(pairs) = value else {
        panic!("object");
    };
    let pair = pairs.iter_mut().find(|(name, _)| name == key).unwrap();
    &mut pair.1
}

fn replace(value: &mut json::Value, next: json::Value) {
    *value = next;
}

fn row_fields(value: &mut json::Value) -> &mut Vec<(String, json::Value)> {
    let section = top(value, "needs_review");
    let json::Value::Array(items) = section else {
        panic!("rows");
    };
    let json::Value::Object(fields) = &mut items[0] else {
        panic!("row");
    };
    fields
}

fn row_fields_value<'a>(value: &'a mut json::Value, key: &str) -> &'a mut json::Value {
    let fields = row_fields(value);
    let pair = fields.iter_mut().find(|(name, _)| name == key).unwrap();
    &mut pair.1
}

fn reviewer_fields(value: &mut json::Value) -> &mut Vec<(String, json::Value)> {
    let reviewers = row_fields_value(value, "reviewers");
    let json::Value::Array(items) = reviewers else {
        panic!("reviewers");
    };
    let json::Value::Object(fields) = &mut items[0] else {
        panic!("reviewer");
    };
    fields
}

fn reviewer_value<'a>(value: &'a mut json::Value, key: &str) -> &'a mut json::Value {
    let fields = reviewer_fields(value);
    let pair = fields.iter_mut().find(|(name, _)| name == key).unwrap();
    &mut pair.1
}

#[test]
fn poll_list_keeps_retry_after_on_429() {
    let client = ping_client("https://git.example.invalid", "jcitizen");
    let delayed = Ok(host::Response {
        status: 429,
        body: Vec::new(),
        retry_after: Some(30),
    });
    let mut queue = Queue {
        steps: vec![delayed],
        urls: Vec::new(),
    };
    let err = match poll_list(&client, &mut queue, 1, &[], &mut |_| Ok(())) {
        Err(err) => err,
        Ok(_) => panic!("429"),
    };
    assert!(matches!(err.error, Error::Http(429)));
    assert_eq!(err.retry_after_ms, Some(30_000));

    let open = Ok(host::Response {
        status: 429,
        body: Vec::new(),
        retry_after: None,
    });
    let mut queue = Queue {
        steps: vec![open],
        urls: Vec::new(),
    };
    let err = match poll_list(&client, &mut queue, 1, &[], &mut |_| Ok(())) {
        Err(err) => err,
        Ok(_) => panic!("429"),
    };
    assert_eq!(err.retry_after_ms, None);

    let mut queue = Queue {
        steps: vec![step(500, "")],
        urls: Vec::new(),
    };
    let err = match poll_list(&client, &mut queue, 1, &[], &mut |_| Ok(())) {
        Err(err) => err,
        Ok(_) => panic!("500"),
    };
    assert!(matches!(err.error, Error::Http(500)));
    assert_eq!(err.retry_after_ms, None);

    let huge = Ok(host::Response {
        status: 429,
        body: Vec::new(),
        retry_after: Some(u64::MAX),
    });
    let mut queue = Queue {
        steps: vec![huge],
        urls: Vec::new(),
    };
    let err = match poll_list(&client, &mut queue, 1, &[], &mut |_| Ok(())) {
        Err(err) => err,
        Ok(_) => panic!("429"),
    };
    assert_eq!(err.retry_after_ms, Some(u64::MAX));
}

const ACTIVITIES: &str = include_str!("../../../fixtures/enrich-activities.json");
const BUILD_MIXED: &str = include_str!("../../../fixtures/enrich-build.json");

fn listed_pr(id: u64, author: &str, reviewer: bool, commit: Option<&str>) -> String {
    let commit = match commit {
        Some(commit) => format!(r#","latestCommit":"{commit}""#),
        None => String::new(),
    };
    let reviewers = if reviewer {
        r##","reviewers":[{"user":{"displayName":"Jane Citizen","slug":"jcitizen"},"status":"UNAPPROVED"}]"##
    } else {
        ""
    };
    format!(
        r##"{{"id":{id},"title":"T{id}","state":"OPEN","createdDate":1,"updatedDate":1,"fromRef":{{"displayId":"f"{commit},"repository":{{"slug":"repo","project":{{"key":"PRJ"}}}}}},"toRef":{{"displayId":"main"}},"author":{{"user":{{"displayName":"{author}","slug":"{author}"}}}}{reviewers}}}"##
    )
}

fn scripted(
    steps: Vec<Result<host::Response, host::Error>>,
    commit: Option<&str>,
) -> (Result<crate::enrich::Filled, InboxFault>, Vec<String>) {
    let mut urls = Vec::new();
    let mut steps = steps;
    let result = {
        let mut get = |path: &str| {
            urls.push(path.to_owned());
            match steps.remove(0) {
                Ok(response) => Ok(response),
                Err(err) => Err(InboxFault::from(Error::from(err))),
            }
        };
        enrich::fetch(
            &mut get,
            &enrich::Query {
                project: "~me",
                repo: "repo",
                number: 9,
                author_slug: "pat",
                user_slug: "jcitizen",
                from_commit: commit,
                activities_only: false,
            },
            &mut |_| Ok(()),
        )
    };
    (result, urls)
}

fn fault_text(err: InboxFault) -> String {
    err.error.to_string()
}

#[test]
fn list_enriches_threads_tasks_build_and_merge() {
    let reviewer = page_body(&listed_pr(1, "pat", true, None), "true", None);
    let author = page_body(&listed_pr(2, "jcitizen", false, Some("abc")), "true", None);
    let waiting_thread = r#"{"size":1,"isLastPage":true,"values":[{"action":"COMMENTED","comment":{"createdDate":1,"author":{"slug":"sam"},"comments":[]}}]}"#;
    let (listed, urls) = run_list(vec![
        step(200, APP),
        step(200, USER_JSON),
        step(200, &reviewer),
        step(200, &author),
        step(200, ACTIVITIES),
        step(200, waiting_thread),
        step(200, r#"{"count":2}"#),
        step(200, BUILD_MIXED),
        step(200, r#"{"conflicted":true,"canMerge":false}"#),
    ]);
    let listed = listed.unwrap();
    let _ = listed.snapshot.clone();
    let row = &listed.snapshot.needs_review[0];
    assert_eq!(row.unanswered_as_author, 5);
    assert_eq!(row.unanswered_as_reviewer, 1);
    assert!(row.events_loaded);
    assert!(!row.events.is_empty());
    assert_eq!(row.build, Build::None);
    let waiting = &listed.snapshot.waiting[0];
    assert_eq!(waiting.unanswered_as_author, 1);
    assert_eq!(waiting.unanswered_as_reviewer, 0);
    assert_eq!(waiting.open_tasks, 2);
    assert_eq!(waiting.build, Build::Failed);
    assert!(waiting.conflicted);
    assert!(!waiting.can_merge);
    assert_eq!(attention_count(&listed.snapshot), 2);
    assert!(urls.iter().any(|url| url.contains("/commits/abc")));
    assert!(
        urls.iter()
            .any(|url| url.contains("blocker-comments?state=OPEN&count=true"))
    );
    let bytes = to_json(&listed.snapshot);
    let current = parse_snapshot(bytes.as_bytes()).unwrap();
    let mut previous = parse_snapshot(bytes.as_bytes()).unwrap();
    previous.needs_review[0].unanswered_as_author = 0;
    previous.needs_review[0].unanswered_as_reviewer = 0;
    previous.waiting[0].open_tasks = 0;
    previous.waiting[0].build = Build::None;
    let changes = diff(Some(&previous), &current);
    let reasons = &changes
        .iter()
        .find(|change| change.id == "PRJ/repo/1")
        .unwrap()
        .reasons;
    assert!(reasons.contains(&Reason::Unanswered));
    let waiting_reasons = &changes
        .iter()
        .find(|change| change.id == "PRJ/repo/2")
        .unwrap()
        .reasons;
    assert!(waiting_reasons.contains(&Reason::Tasks));
    assert!(waiting_reasons.contains(&Reason::BuildFailed));
    let bad = parse_page(
        br#"{"size":1,"isLastPage":true,"values":[{"id":1,"title":"T","state":"OPEN","createdDate":1,"updatedDate":1,"fromRef":{"displayId":"f","latestCommit":1,"repository":{"slug":"repo","project":{"key":"PRJ"}}},"toRef":{"displayId":"main"},"author":{"user":{"displayName":"Pat","slug":"pat"}}}]}"#,
    );
    assert!(bad.unwrap_err().to_string().contains("latestCommit"));
}

#[test]
fn enrich_pages_fallbacks_and_rejects_bad_bodies() {
    let page = r#"{"size":1,"isLastPage":false,"nextPageStart":25,"values":[{"action":"COMMENTED","comment":{"createdDate":1,"author":{"slug":"sam"}}}]}"#;
    let (filled, urls) = scripted(
        vec![
            step(200, page),
            step(200, r#"{"size":0,"isLastPage":true,"values":[]}"#),
            step(400, "no"),
            step(
                200,
                r#"{"isLastPage":false,"nextPageStart":25,"values":[{},{}]}"#,
            ),
            step(200, r#"{"isLastPage":true,"values":[{}]}"#),
            step(404, "missing"),
            step(200, r#"{"canMerge":true}"#),
        ],
        Some("abc def"),
    );
    let filled = filled.unwrap();
    let _ = format!("{filled:?}");
    assert_eq!(filled.unanswered_as_author, 1);
    assert_eq!(filled.open_tasks, 3);
    assert_eq!(filled.build, Build::None);
    assert!(!filled.conflicted);
    assert!(filled.can_merge);
    assert!(
        urls[0].contains("/projects/~me/repos/repo/pull-requests/9/activities?start=0&limit=25")
    );
    assert!(urls.iter().any(|url| url.contains("/commits/abc%20def")));
    assert!(urls.iter().any(|url| url.contains("start=25&limit=25")));

    let (progress, _) = scripted(
        vec![
            step(200, EMPTY_PAGE),
            step(200, r#"{"count":0}"#),
            step(200, r#"{"values":[{"state":"RUNNING"}]}"#),
            step(200, "{}"),
        ],
        Some("abc"),
    );
    assert_eq!(progress.unwrap().build, Build::InProgress);
    let (successful, _) = scripted(
        vec![
            step(200, EMPTY_PAGE),
            step(200, r#"{"count":0}"#),
            step(200, r#"{"values":[{"status":"SUCCESSFUL"}]}"#),
            step(200, r#"{"conflicted":true,"canMerge":true}"#),
        ],
        Some("abc"),
    );
    let successful = successful.unwrap();
    assert_eq!(successful.build, Build::Successful);
    assert!(successful.conflicted && successful.can_merge);
    let (none, urls) = scripted(
        vec![
            step(200, EMPTY_PAGE),
            step(200, r#"{"isLastPage":true,"values":[]}"#),
            step(200, "{}"),
        ],
        None,
    );
    assert_eq!(none.unwrap().build, Build::None);
    assert!(urls.iter().all(|url| !url.contains("build-status")));

    let (held, _) = scripted(
        vec![
            step(200, EMPTY_PAGE),
            step(
                200,
                r#"{"isLastPage":false,"nextPageStart":25,"values":[{}]}"#,
            ),
            step(200, r#"{"isLastPage":true,"values":[{},{}]}"#),
            step(200, "{}"),
        ],
        None,
    );
    assert_eq!(held.unwrap().open_tasks, 3);

    let cases = [
        (
            vec![step(
                200,
                r#"{"isLastPage":false,"nextPageStart":0,"values":[]}"#,
            )],
            "nextPageStart",
        ),
        (
            vec![step(200, r#"{"isLastPage":"yes","values":[]}"#)],
            "isLastPage",
        ),
        (
            vec![step(200, r#"{"isLastPage":true,"values":{}}"#)],
            "values",
        ),
        (vec![step(200, r#"{"isLastPage":true}"#)], "missing values"),
        (vec![step(200, "{")], "line"),
        (vec![step(429, "slow")], "HTTP 429"),
        (vec![step(500, "no")], "HTTP 500"),
    ];
    for (steps, needle) in cases {
        let (err, _) = scripted(steps, None);
        assert!(fault_text(err.unwrap_err()).contains(needle), "{needle}");
    }
    let delayed = Ok(host::Response {
        status: 429,
        body: Vec::new(),
        retry_after: Some(4),
    });
    let (err, _) = scripted(vec![delayed], None);
    let err = err.unwrap_err();
    assert_eq!(err.retry_after_ms, Some(4_000));
    let _ = format!("{err:?}");
    let (err, _) = scripted(
        vec![Err(host::Error::Timeout {
            program: "curl".to_owned(),
        })],
        None,
    );
    assert!(fault_text(err.unwrap_err()).contains("timed out"));

    let (err, _) = scripted(
        vec![step(200, EMPTY_PAGE), step(200, r#"{"count":"x"}"#)],
        None,
    );
    assert!(fault_text(err.unwrap_err()).contains("count"));
    let (err, _) = scripted(vec![step(200, EMPTY_PAGE), step(500, "no")], None);
    assert!(fault_text(err.unwrap_err()).contains("HTTP 500"));
    let (err, _) = scripted(
        vec![
            step(200, EMPTY_PAGE),
            step(400, "no"),
            step(200, r#"{"isLastPage":false,"values":[]}"#),
        ],
        None,
    );
    assert!(fault_text(err.unwrap_err()).contains("nextPageStart"));
    let (err, _) = scripted(
        vec![
            step(200, EMPTY_PAGE),
            step(400, "no"),
            step(200, r#"{"isLastPage":false,"nextPageStart":0,"values":[]}"#),
        ],
        None,
    );
    assert!(fault_text(err.unwrap_err()).contains("did not advance"));
    let (err, _) = scripted(
        vec![
            step(200, EMPTY_PAGE),
            step(200, r#"{"count":1}"#),
            step(200, r#"{"values":[{"state":1}]}"#),
        ],
        Some("abc"),
    );
    assert!(fault_text(err.unwrap_err()).contains("state"));
    let (empty_build, _) = scripted(
        vec![
            step(200, EMPTY_PAGE),
            step(200, r#"{"count":0}"#),
            step(200, r#"{"values":[]}"#),
            step(200, "{}"),
        ],
        Some("abc"),
    );
    assert_eq!(empty_build.unwrap().build, Build::None);
    let (err, _) = scripted(
        vec![
            step(200, EMPTY_PAGE),
            step(200, r#"{"count":0}"#),
            step(500, "no"),
        ],
        None,
    );
    assert!(fault_text(err.unwrap_err()).contains("HTTP 500"));
    let (err, _) = scripted(
        vec![
            step(200, EMPTY_PAGE),
            step(200, r#"{"count":0}"#),
            step(200, r#"{"conflicted":"x"}"#),
        ],
        None,
    );
    assert!(fault_text(err.unwrap_err()).contains("conflicted"));
    let (err, _) = scripted(
        vec![
            step(200, EMPTY_PAGE),
            step(200, r#"{"count":0}"#),
            step(200, r#"{"conflicted":true,"canMerge":"x"}"#),
        ],
        None,
    );
    assert!(fault_text(err.unwrap_err()).contains("canMerge"));
    let (err, _) = scripted(
        vec![
            step(200, EMPTY_PAGE),
            step(200, r#"{"count":0}"#),
            step(500, "no"),
        ],
        Some("abc"),
    );
    assert!(fault_text(err.unwrap_err()).contains("HTTP 500"));
}

#[test]
fn enrich_keeps_the_oldest_fifty() {
    let needs: Vec<_> = (0..51)
        .map(|index| finger_row(&format!("p/r/{index}"), index, Vec::new()))
        .collect();
    let waiting = vec![finger_row("w/w/1", 5, Vec::new())];
    let slots = enrich::slots(&needs, &waiting, 50);
    assert_eq!(slots.len(), 50);
    assert!(matches!(slots[0], enrich::Slot::Needs(0)));
    assert!(
        slots
            .iter()
            .all(|slot| !matches!(slot, enrich::Slot::Needs(50)))
    );
    let tied = vec![
        finger_row("a/a/1", 5, Vec::new()),
        finger_row("b/b/2", 5, Vec::new()),
        finger_row("c/c/3", 1, Vec::new()),
    ];
    let order = enrich::slots(&tied, &waiting, 50);
    assert!(matches!(order[0], enrich::Slot::Needs(2)));
    assert!(matches!(order[1], enrich::Slot::Needs(0)));
    assert!(matches!(order[2], enrich::Slot::Needs(1)));
    assert!(matches!(order[3], enrich::Slot::Waiting(0)));
    assert_eq!(
        enrich::rank_cmp((1, 0, 0), (2, 0, 0)),
        std::cmp::Ordering::Less
    );
    assert_eq!(
        enrich::rank_cmp((2, 0, 0), (1, 0, 0)),
        std::cmp::Ordering::Greater
    );
    assert_eq!(
        enrich::rank_cmp((1, 0, 0), (1, 1, 0)),
        std::cmp::Ordering::Less
    );
    assert_eq!(
        enrich::rank_cmp((1, 1, 0), (1, 0, 0)),
        std::cmp::Ordering::Greater
    );
    assert_eq!(
        enrich::rank_cmp((1, 0, 0), (1, 0, 1)),
        std::cmp::Ordering::Less
    );
    assert_eq!(
        enrich::rank_cmp((1, 0, 1), (1, 0, 0)),
        std::cmp::Ordering::Greater
    );
    assert_eq!(
        enrich::rank_cmp((1, 0, 0), (1, 0, 0)),
        std::cmp::Ordering::Equal
    );
}

#[test]
fn poll_publishes_each_reply_and_stops_when_publish_fails() {
    let (quiet, _) = run_list(vec![
        step(200, APP),
        step(200, USER_JSON),
        step(200, EMPTY_PAGE),
        step(200, EMPTY_PAGE),
    ]);
    assert!(quiet.is_ok());
    let mut seen = 0u32;
    let client = ping_client("https://git.example.invalid", "jcitizen");
    let mut queue = Queue {
        steps: vec![
            step(200, APP),
            step(200, USER_JSON),
            step(200, EMPTY_PAGE),
            step(200, EMPTY_PAGE),
        ],
        urls: Vec::new(),
    };
    let counted = poll_list(&client, &mut queue, NOW_MS, &[], &mut |_| {
        seen += 1;
        Ok(())
    });
    assert!(counted.is_ok());
    assert_eq!(seen, 3);
    let mut queue = Queue {
        steps: vec![step(200, APP), step(200, USER_JSON), step(200, EMPTY_PAGE)],
        urls: Vec::new(),
    };
    let err = poll_list(&client, &mut queue, NOW_MS, &[], &mut |_| {
        Err(Error::Io(std::io::Error::other("disk")))
    });
    let Err(err) = err else { panic!("disk") };
    assert!(err.error.to_string().contains("disk"));
    let page = page_body(&listed_pr(1, "pat", true, None), "true", None);
    let mut calls = 0u32;
    let mut queue = Queue {
        steps: vec![
            step(200, APP),
            step(200, USER_JSON),
            step(200, &page),
            step(200, EMPTY_PAGE),
            step(200, EMPTY_PAGE),
        ],
        urls: Vec::new(),
    };
    let err = poll_list(&client, &mut queue, NOW_MS, &[], &mut |_| {
        calls += 1;
        if calls == 3 {
            Err(Error::Io(std::io::Error::other("disk")))
        } else {
            Ok(())
        }
    });
    let Err(err) = err else { panic!("disk") };
    assert!(err.error.to_string().contains("disk"));
    let mut steps = vec![step(200, EMPTY_PAGE)];
    let mut get = |_: &str| match steps.remove(0) {
        Ok(response) => Ok(response),
        Err(err) => Err(InboxFault::from(Error::from(err))),
    };
    let err = enrich::fetch(
        &mut get,
        &enrich::Query {
            project: "PRJ",
            repo: "repo",
            number: 1,
            author_slug: "pat",
            user_slug: "jcitizen",
            from_commit: None,
            activities_only: false,
        },
        &mut |_| Err(InboxFault::from(Error::Io(std::io::Error::other("stop")))),
    );
    assert!(err.unwrap_err().error.to_string().contains("stop"));
}

#[test]
fn clarify_gone_reads_merged_or_declined() {
    let mut changes = vec![
        Change {
            id: "PRJ/repo/12".to_owned(),
            html_url: "https://git.example.invalid/pull/12".to_owned(),
            reasons: vec![Reason::NeedsReview],
            gone_text: String::new(),
        },
        Change {
            id: "nope".to_owned(),
            html_url: "https://git.example.invalid/pull/0".to_owned(),
            reasons: vec![Reason::Gone],
            gone_text: String::new(),
        },
    ];
    let client = ping_client("https://git.example.invalid", "jcitizen");
    let mut queue = Queue {
        steps: Vec::new(),
        urls: Vec::new(),
    };
    clarify_gone(&client, &mut queue, &mut changes);
    assert!(changes[0].gone_text.is_empty());
    assert!(changes[1].gone_text.is_empty());
    let mut gone = changes.pop().unwrap();
    gone.id = "PRJ/repo/12".to_owned();
    let cases = [
        (step(200, r#"{"state":"MERGED"}"#), "merged"),
        (step(200, r#"{"state":"DECLINED"}"#), "declined"),
        (step(200, r#"{"state":"OPEN"}"#), ""),
        (step(200, r#"{"state":"NOPE"}"#), ""),
        (step(200, "{}"), ""),
        (step(200, "{"), ""),
        (step(404, "no"), ""),
        (
            Err(host::Error::Timeout {
                program: "curl".to_owned(),
            }),
            "",
        ),
    ];
    for (response, word) in cases {
        gone.gone_text = "stale".to_owned();
        let mut queue = Queue {
            steps: vec![response],
            urls: Vec::new(),
        };
        let mut one = [gone.clone()];
        clarify_gone(&client, &mut queue, &mut one);
        assert_eq!(one[0].gone_text, word);
        gone = one[0].clone();
    }
    gone.id = "~me/repo/3".to_owned();
    let mut queue = Queue {
        steps: vec![step(200, r#"{"state":"MERGED"}"#)],
        urls: Vec::new(),
    };
    clarify_gone(&client, &mut queue, &mut [gone]);
    assert!(queue.urls[0].contains("/projects/~me/repos/repo/pull-requests/3"));
}

fn sample_event(id: u64, actor: &str, kind: EventKind, text: &str) -> Event {
    Event {
        id,
        created_ms: id,
        actor_slug: actor.to_owned(),
        actor_name: actor.to_owned(),
        kind,
        text: text.to_owned(),
        thread: vec![actor.to_owned()],
        added_user: kind == EventKind::Added,
    }
}

#[test]
fn activities_and_watermarks_follow_the_inbox_rules() {
    let page = json::parse(
        br#"{"values":[
            {"action":"COMMENTED","createdDate":5,"user":{"slug":"ada","displayName":"Ada"},"comment":{"text":"see @Jcitizen","author":{"slug":"ada"},"comments":[{"author":{"slug":"jcitizen"},"text":"ok"}]}},
            {"id":9,"action":"APPROVED","user":{"slug":"sam","displayName":"Sam"}},
            {"id":8,"action":"UPDATED","user":{"slug":"ada","displayName":"Ada"}},
            {"id":7,"action":"REOPENED","user":{"slug":"ada","displayName":"Ada"}},
            {"id":6,"action":"UPDATED","addedReviewers":[{"slug":"jcitizen"},{"user":{"slug":"bea"}},{}],"user":{"slug":"ada","displayName":"Ada"}},
            {"action":"OPENED","createdDate":4},
            1
        ]}"#,
    )
    .unwrap();
    let err = crate::activity::page_events(&page, "jcitizen").unwrap_err();
    assert!(err.to_string().contains("activity is not an object"));
    let missing =
        crate::activity::page_events(&json::parse(b"{}").unwrap(), "jcitizen").unwrap_err();
    assert!(missing.to_string().contains("missing values"));
    let bad = crate::activity::page_events(&json::parse(br#"{"values":1}"#).unwrap(), "jcitizen")
        .unwrap_err();
    assert!(bad.to_string().contains("values is not an array"));
    let mut body = page;
    if let json::Value::Object(pairs) = &mut body {
        pairs[0].1 = json::parse(
            br#"[{"action":"COMMENTED","createdDate":5,"user":{"slug":"ada","displayName":"Ada"},"comment":{"text":"see @Jcitizen","author":{"slug":"ada"},"comments":[{"author":{"slug":"jcitizen"}}]}},{"id":9,"action":"APPROVED","user":{"slug":"sam","displayName":"Sam"}},{"id":8,"action":"UPDATED","user":{"slug":"ada"}},{"id":7,"action":"REOPENED","user":{"slug":"ada","displayName":"Ada"}},{"id":6,"action":"UPDATED","addedReviewers":[{"slug":"jcitizen"},{"user":{"slug":"bea"}},{}],"user":{"slug":"ada","displayName":"Ada"}},{"action":"OPENED","createdDate":4}]"#,
        )
        .unwrap();
    }
    let events = crate::activity::page_events(&body, "jcitizen").unwrap();
    assert_eq!(events.len(), 6);
    assert_eq!(events[0].kind, EventKind::Commented);
    assert!(
        events
            .iter()
            .any(|event| event.kind == EventKind::Added && event.added_user)
    );
    assert!(events.iter().any(|event| event.kind == EventKind::Other));
    assert!(events.iter().any(|event| event.id == 4));
    for event in &events {
        let _ = format!("{event:?} {:?}", event.kind);
    }

    let dir = temp("marks");
    assert!(read_store(&dir).unwrap().marks.is_empty());
    let mut row = finger_row("PRJ/repo/1", 10, Vec::new());
    row.events = vec![
        sample_event(1, "ada", EventKind::Commented, "hello"),
        sample_event(2, "ada", EventKind::Commented, "ping @jcitizen"),
        sample_event(3, "jcitizen", EventKind::Commented, "mine"),
        sample_event(4, "ada", EventKind::Approved, ""),
        sample_event(5, "ada", EventKind::Pushed, "subject"),
        sample_event(6, "ada", EventKind::Reopened, ""),
        sample_event(7, "ada", EventKind::Added, ""),
        sample_event(8, "ada", EventKind::Other, ""),
    ];
    row.events[1].thread = vec!["ada".to_owned(), "jcitizen".to_owned()];
    let mut waiting = finger_row("PRJ/repo/2", 10, Vec::new());
    waiting.events = vec![
        sample_event(3, "sam", EventKind::Commented, "look"),
        sample_event(4, "sam", EventKind::Approved, ""),
        sample_event(5, "sam", EventKind::Pushed, "nope"),
    ];
    waiting.build = Build::Failed;
    waiting.conflicted = true;
    let mut snapshot = finger_snapshot("jcitizen", vec![row], vec![waiting]);
    let encoded = to_json(&snapshot);
    assert!(encoded.contains("\"kind\":\"approved\""));
    assert!(encoded.contains("\"kind\":\"pushed\""));
    assert!(encoded.contains("\"kind\":\"reopened\""));
    assert!(encoded.contains("\"kind\":\"added\""));
    let snap_dir = temp("event-json");
    let reject = |body: String, needle: &str| {
        fs::write(snap_dir.join("snapshot.json"), body).unwrap();
        let err = read_snapshot(&snap_dir).unwrap_err();
        assert!(err.to_string().contains(needle), "{err}");
    };
    fs::write(
        snap_dir.join("snapshot.json"),
        encoded.replace("\"events\":", "\"events_gone\":"),
    )
    .unwrap();
    assert!(read_snapshot(&snap_dir).unwrap().is_some());
    fs::write(
        snap_dir.join("snapshot.json"),
        encoded.replace("\"events_loaded\":false,", ""),
    )
    .unwrap();
    assert!(read_snapshot(&snap_dir).unwrap().is_some());
    reject(
        encoded.replace("\"events\":[", "\"events\":false,\"kept\":["),
        "not an array",
    );
    reject(
        encoded.replace("\"events_loaded\":false", "\"events_loaded\":1"),
        "not a bool",
    );
    reject(
        encoded.replace("\"events\":[", "\"events\":[1,"),
        "not an object",
    );
    reject(
        encoded.replace("\"thread\":[\"ada\"]", "\"thread\":[1]"),
        "not a string",
    );
    reject(
        encoded.replace("\"thread\":[\"ada\"]", "\"thread\":1"),
        "not an array",
    );
    reject(
        encoded.replace("\"thread\":[\"ada\"],", ""),
        "missing thread",
    );
    reject(
        encoded.replace("\"kind\":\"commented\"", "\"kind\":\"nope\""),
        "unknown event kind",
    );
    fs::remove_dir_all(&snap_dir).unwrap();
    let mut store = Store::default();
    assert_eq!(unread_count(&store, &snapshot), 0);
    assert!(caught_up(&store, "PRJ/repo/1").is_none());
    prime(&mut store, &snapshot);
    assert_eq!(caught_up(&store, "PRJ/repo/1"), Some(8));
    assert_eq!(unread_count(&store, &snapshot), 0);
    prime(&mut store, &snapshot);
    snapshot.needs_review[0].events.push(sample_event(
        9,
        "bea",
        EventKind::Commented,
        "no mention",
    ));
    assert_eq!(unread_count(&store, &snapshot), 0);
    snapshot.needs_review[0].events.push(sample_event(
        10,
        "bea",
        EventKind::Commented,
        "hey @Jcitizen",
    ));
    assert_eq!(unread_count(&store, &snapshot), 1);
    snapshot.needs_review[0].events.pop();
    snapshot.needs_review[0]
        .events
        .push(sample_event(11, "bea", EventKind::Pushed, ""));
    assert_eq!(unread_count(&store, &snapshot), 1);
    snapshot.needs_review[0].events.pop();
    snapshot.needs_review[0]
        .events
        .push(sample_event(12, "bea", EventKind::Reopened, ""));
    assert_eq!(unread_count(&store, &snapshot), 1);
    snapshot.needs_review[0].events.pop();
    snapshot.needs_review[0]
        .events
        .push(sample_event(13, "bea", EventKind::Added, ""));
    assert_eq!(unread_count(&store, &snapshot), 1);
    snapshot.needs_review[0].events.pop();
    let mut quiet = sample_event(14, "bea", EventKind::Added, "");
    quiet.added_user = false;
    snapshot.needs_review[0].events.push(quiet);
    assert_eq!(unread_count(&store, &snapshot), 0);
    snapshot.needs_review[0].events.pop();
    let mut threaded = sample_event(15, "bea", EventKind::Commented, "in the thread");
    threaded.thread = vec!["jcitizen".to_owned(), "bea".to_owned()];
    snapshot.needs_review[0].events.push(sample_event(
        16,
        "jcitizen",
        EventKind::Commented,
        "self",
    ));
    assert_eq!(unread_count(&store, &snapshot), 0);
    snapshot.needs_review[0].events.pop();
    snapshot.needs_review[0]
        .events
        .push(sample_event(17, "bea", EventKind::Approved, ""));
    assert_eq!(unread_count(&store, &snapshot), 0);
    snapshot.needs_review[0].events.pop();
    snapshot.needs_review[0]
        .events
        .push(sample_event(18, "bea", EventKind::Other, ""));
    assert_eq!(unread_count(&store, &snapshot), 0);
    snapshot.needs_review[0].events.pop();
    snapshot.needs_review[0].events.push(threaded);
    assert_eq!(unread_count(&store, &snapshot), 1);
    assert!(toggle_ignore(&mut store, &snapshot.needs_review[0], true));
    assert!(is_ignored(&store, "PRJ/repo/1"));
    assert_eq!(unread_count(&store, &snapshot), 0);
    assert!(toggle_ignore(&mut store, &snapshot.needs_review[0], true));
    assert!(!toggle_ignore(&mut store, &snapshot.waiting[0], false));
    snapshot.waiting[0]
        .events
        .push(sample_event(6, "sam", EventKind::Commented, "more"));
    assert_eq!(unread_count(&store, &snapshot), 2);
    mark_read(&mut store, &snapshot.needs_review[0]);
    mark_read(&mut store, &snapshot.waiting[0]);
    assert_eq!(unread_count(&store, &snapshot), 0);
    snapshot.waiting[0].build = Build::Successful;
    prime(&mut store, &snapshot);
    snapshot.waiting[0].build = Build::Failed;
    snapshot.waiting[0].conflicted = false;
    prime(&mut store, &snapshot);
    snapshot.waiting[0].conflicted = true;
    assert_eq!(unread_count(&store, &snapshot), 1);
    mark_read(&mut store, &snapshot.waiting[0]);
    assert_eq!(unread_count(&store, &snapshot), 0);
    write_store(&dir, &store).unwrap();
    let loaded = read_store(&dir).unwrap();
    assert_eq!(loaded.marks.len(), store.marks.len());
    let _ = format!("{store:?} {:?}", store.marks.values().next().unwrap());
    fs::write(dir.join("watermarks.json"), b"{").unwrap();
    assert!(matches!(read_store(&dir).unwrap_err(), Error::Json(_)));
    fs::write(dir.join("watermarks.json"), b"[]").unwrap();
    assert!(read_store(&dir).unwrap_err().to_string().contains("object"));
    fs::write(dir.join("watermarks.json"), b"{}").unwrap();
    assert!(
        read_store(&dir)
            .unwrap_err()
            .to_string()
            .contains("missing items")
    );
    fs::write(dir.join("watermarks.json"), br#"{"items":1}"#).unwrap();
    assert!(
        read_store(&dir)
            .unwrap_err()
            .to_string()
            .contains("not an array")
    );
    fs::write(dir.join("watermarks.json"), br#"{"items":[{"id":"a"}]}"#).unwrap();
    assert!(
        read_store(&dir)
            .unwrap_err()
            .to_string()
            .contains("missing activity_id")
    );
    fs::write(
        dir.join("watermarks.json"),
        br#"{"items":[{"id":"a","activity_id":1}]}"#,
    )
    .unwrap();
    assert!(
        read_store(&dir)
            .unwrap_err()
            .to_string()
            .contains("missing ignored")
    );
    fs::write(dir.join("watermarks.json"), br#"{"items":[{}]}"#).unwrap();
    assert!(
        read_store(&dir)
            .unwrap_err()
            .to_string()
            .contains("missing id")
    );
    let file = dir.join("not-a-dir");
    fs::write(&file, b"x").unwrap();
    assert!(write_store(&file, &store).is_err());
    fs::remove_file(dir.join("watermarks.json")).unwrap();
    fs::create_dir(dir.join("watermarks.json")).unwrap();
    assert!(matches!(read_store(&dir).unwrap_err(), Error::Io(_)));
    fs::remove_dir_all(&dir).unwrap();

    let reviewer = page_body(&listed_pr(1, "pat", true, None), "true", None);
    let mut cached = finger_row("PRJ/repo/1", 1, Vec::new());
    cached.events = vec![sample_event(4, "ada", EventKind::Commented, "kept")];
    cached.events_loaded = true;
    cached.build = Build::Successful;
    let client = ping_client("https://git.example.invalid", "jcitizen");
    let mut queue = Queue {
        steps: vec![
            step(200, APP),
            step(200, USER_JSON),
            step(200, &reviewer),
            step(200, EMPTY_PAGE),
        ],
        urls: Vec::new(),
    };
    let kept = poll_list(&client, &mut queue, NOW_MS, &[cached], &mut |_| Ok(())).unwrap();
    assert!(queue.urls.iter().all(|url| !url.contains("/activities")));
    assert_eq!(kept.snapshot.needs_review[0].events[0].text, "kept");
    assert_eq!(kept.snapshot.needs_review[0].build, Build::Successful);
    let moved = page_body(
        &listed_pr(1, "pat", true, None).replace("\"updatedDate\":1", "\"updatedDate\":9"),
        "true",
        None,
    );
    let mut queue = Queue {
        steps: vec![
            step(200, APP),
            step(200, USER_JSON),
            step(200, &moved),
            step(200, EMPTY_PAGE),
            step(200, EMPTY_PAGE),
        ],
        urls: Vec::new(),
    };
    let mut stale = finger_row("PRJ/repo/1", 1, Vec::new());
    stale.events_loaded = true;
    stale.events = vec![sample_event(1, "ada", EventKind::Commented, "old")];
    let fresh = poll_list(&client, &mut queue, NOW_MS, &[stale], &mut |_| Ok(())).unwrap();
    assert!(fresh.snapshot.needs_review[0].events.is_empty());
    assert!(fresh.snapshot.needs_review[0].events_loaded);
}
