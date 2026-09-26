//! Bitbucket reads for `ping`.
//!
//! [`ping`] checks TLS against the origin, then GETs application-properties,
//! `users/{slug}`, and the inbox count. A completed HTTP response on the origin
//! is a TLS success, including a redirect to a login page. The three REST calls
//! require status 200 and a JSON object.

use crate::{Config, Error};
use std::path::PathBuf;
use std::time::Duration;

/// `User-Agent` sent on every GET. The version is this package's `Cargo.toml` version.
pub const USER_AGENT: &str = concat!("bistill/", env!("CARGO_PKG_VERSION"), " (internal)");

/// `--max-time` for each ping GET.
pub const TIMEOUT: Duration = Duration::from_secs(15);

/// Bitbucket `displayName` and `version` from application-properties.
#[derive(Debug)]
pub struct Product {
    /// `displayName`.
    pub display_name: String,
    /// `version`.
    pub version: String,
}

/// The authenticated user. `slug` is the JSON slug.
#[derive(Debug)]
pub struct User {
    /// Canonical slug from the user JSON.
    pub slug: String,
    /// `displayName`.
    pub display_name: String,
}

/// Inbox count.
///
/// `Split` when both `reviewer` and `author` are integers. `Total` when those
/// two are absent or incomplete and `count` is an integer.
#[derive(Debug)]
pub enum InboxCount {
    /// `reviewer` and `author` fields.
    Split { reviewer: u64, author: u64 },
    /// A single `count` field.
    Total(u64),
}

impl std::fmt::Display for InboxCount {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            InboxCount::Split { reviewer, author } => {
                write!(f, "reviewer {reviewer}, author {author}")
            }
            InboxCount::Total(count) => write!(f, "{count}"),
        }
    }
}

/// Raw bodies of the three REST calls.
#[derive(Debug)]
pub struct Bodies {
    /// `GET /rest/api/1.0/application-properties`.
    pub application_properties: Vec<u8>,
    /// `GET /rest/api/1.0/users/{slug}`.
    pub user: Vec<u8>,
    /// `GET /rest/api/1.0/inbox/pull-requests/count`.
    pub inbox_count: Vec<u8>,
}

/// A finished ping.
#[derive(Debug)]
pub struct Report {
    /// Application properties.
    pub product: Product,
    /// Authenticated user.
    pub user: User,
    /// Inbox count.
    pub inbox: InboxCount,
    /// Response bodies, unchanged.
    pub bodies: Bodies,
}

/// Curl program and the settings each GET needs. `token` is omitted from [`Debug`].
pub struct Client {
    program: String,
    base_url: String,
    username: String,
    token: String,
    ca_file: Option<PathBuf>,
}

impl std::fmt::Debug for Client {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Client")
            .field("program", &self.program)
            .field("base_url", &self.base_url)
            .field("username", &self.username)
            .field("token", &"***")
            .field("ca_file", &self.ca_file)
            .finish()
    }
}

impl Client {
    /// `program` is `curl` or `curl.exe`, or a path to that binary.
    pub fn new(program: &str, config: &Config) -> Self {
        Self {
            program: program.to_owned(),
            base_url: config.base_url.clone(),
            username: config.username.clone(),
            token: config.token().to_owned(),
            ca_file: config.ca_file.clone(),
        }
    }

    /// GET `path` on `base_url`. `path` starts with `/`.
    pub fn request(&self, path: &str) -> host::Request {
        host::Request {
            program: self.program.clone(),
            url: format!("{}{path}", self.base_url),
            headers: vec![
                "Accept: application/json".to_owned(),
                format!("Authorization: Bearer {}", self.token),
            ],
            user_agent: USER_AGENT.to_owned(),
            timeout: TIMEOUT,
            ca_file: self.ca_file.clone(),
            fail_with_body: true,
        }
    }
}

/// One GET. [`CurlFetch`] runs it through `curl`. Tests supply a stand-in.
pub trait Fetch {
    /// Perform `request` and return the status and body.
    fn get(&mut self, request: &host::Request) -> Result<host::Response, host::Error>;
}

/// [`Fetch`] that calls [`host::get`](host::get).
pub struct CurlFetch;

impl Fetch for CurlFetch {
    fn get(&mut self, request: &host::Request) -> Result<host::Response, host::Error> {
        host::get(request)
    }
}

/// Ping through `curl`.
pub fn ping(client: &Client) -> Result<Report, Error> {
    ping_with(client, &mut CurlFetch)
}

/// Ping through `fetch`.
///
/// The origin GET treats any HTTP status as TLS success. Each REST call stops
/// the sequence on a transport error or a status other than 200. The JSON `slug`
/// is kept as the canonical user and must match `username` without regard to ASCII case.
pub fn ping_with(client: &Client, fetch: &mut dyn Fetch) -> Result<Report, Error> {
    call(client, fetch, "/")?;
    let application = call_ok(client, fetch, "/rest/api/1.0/application-properties")?;
    let product = parse_product(&application)?;
    let user_path = format!("/rest/api/1.0/users/{}", encode_segment(&client.username));
    let user_body = call_ok(client, fetch, &user_path)?;
    let user = parse_user(&client.username, &user_body)?;
    let inbox_body = call_ok(client, fetch, "/rest/api/1.0/inbox/pull-requests/count")?;
    let inbox = parse_inbox(&inbox_body)?;
    Ok(Report {
        product,
        user,
        inbox,
        bodies: Bodies {
            application_properties: application,
            user: user_body,
            inbox_count: inbox_body,
        },
    })
}

/// Parse an application-properties body.
pub fn parse_product(body: &[u8]) -> Result<Product, Error> {
    read_product(&json::parse(body)?)
}

/// Parse a user body. `username` must match JSON `slug`, ignoring ASCII case.
pub fn parse_user(username: &str, body: &[u8]) -> Result<User, Error> {
    read_user(username, &json::parse(body)?)
}

/// Parse an inbox-count body.
pub fn parse_inbox(body: &[u8]) -> Result<InboxCount, Error> {
    read_inbox(&json::parse(body)?)
}

fn call(client: &Client, fetch: &mut dyn Fetch, path: &str) -> Result<host::Response, Error> {
    fetch.get(&client.request(path)).map_err(Error::from)
}

fn call_ok(client: &Client, fetch: &mut dyn Fetch, path: &str) -> Result<Vec<u8>, Error> {
    let response = call(client, fetch, path)?;
    if response.status != 200 {
        return Err(Error::Http(response.status));
    }
    Ok(response.body)
}

fn read_product(value: &json::Value) -> Result<Product, Error> {
    Ok(Product {
        display_name: string_field(value, "displayName")?,
        version: string_field(value, "version")?,
    })
}

fn read_user(username: &str, value: &json::Value) -> Result<User, Error> {
    let slug = string_field(value, "slug")?;
    if !slug.eq_ignore_ascii_case(username) {
        return Err(Error::Auth(format!(
            "slug {slug} does not match username {username}"
        )));
    }
    Ok(User {
        slug,
        display_name: string_field(value, "displayName")?,
    })
}

fn read_inbox(value: &json::Value) -> Result<InboxCount, Error> {
    match (u64_field(value, "reviewer"), u64_field(value, "author")) {
        (Some(reviewer), Some(author)) => Ok(InboxCount::Split { reviewer, author }),
        _ => match u64_field(value, "count") {
            Some(count) => Ok(InboxCount::Total(count)),
            None => Err(shape("missing inbox count")),
        },
    }
}

fn string_field(value: &json::Value, name: &str) -> Result<String, Error> {
    match value.get(name) {
        Some(field) => match field.as_str() {
            Some(text) => Ok(text.to_owned()),
            None => Err(shape(&format!("{name} is not a string"))),
        },
        None => Err(shape(&format!("missing {name}"))),
    }
}

fn u64_field(value: &json::Value, name: &str) -> Option<u64> {
    value.get(name).and_then(json::Value::as_u64)
}

fn shape(message: &str) -> Error {
    Error::Json(json::Error {
        message: message.to_owned(),
        offset: 0,
        line: 1,
        column: 1,
    })
}

fn encode_segment(text: &str) -> String {
    let mut out = String::new();
    for byte in text.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(byte as char);
            }
            other => out.push_str(&format!("%{other:02X}")),
        }
    }
    out
}
