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
    let bare = ping_client("https://git.example.invalid", "jcitizen");
    assert!(bare.request("/").ca_file.is_none());
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
