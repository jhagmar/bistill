//! StatusNotifierItem on the session bus.
//!
//! The socket itself is safe Rust. `getuid` is not in the standard library, so
//! the auth token uses one `unsafe` call. The watcher calls `Activate` on our
//! item; we answer by asking the terminal to raise its window.

#![allow(unsafe_code)]

use crate::Error;
use std::io::{Read, Write};
use std::os::unix::net::UnixStream;
use std::path::Path;
use std::time::Duration;

pub(crate) fn set_unread(count: u64) -> Result<(), Error> {
    let address = std::env::var("DBUS_SESSION_BUS_ADDRESS").ok();
    set_unread_at(address.as_deref(), count, Path::new("/dev/tty"))
}

pub(crate) fn set_unread_at(address: Option<&str>, count: u64, tty: &Path) -> Result<(), Error> {
    let Some(address) = address else {
        return Err(tray_error("no status icon watcher"));
    };
    let path = unix_path(address)?;
    let mut stream = UnixStream::connect(path).map_err(|err| tray_error(&err.to_string()))?;
    let _ = stream.set_read_timeout(Some(Duration::from_millis(200)));
    register(&mut stream, count)?;
    raise_on_activate(next_member(&mut stream), tty);
    Ok(())
}

pub(crate) fn raise_on_activate(member: Option<String>, tty: &Path) {
    if let Some(member) = member {
        if member == "Activate" {
            raise_terminal(tty);
        }
    }
}

// One trait object, so register, the handshake, and the error paths are a single function.
pub(crate) trait Io: Read + Write {}

impl<T: Read + Write> Io for T {}

pub(crate) fn register(stream: &mut dyn Io, count: u64) -> Result<(), Error> {
    let uid = hex_uid();
    stream
        .write_all(format!("\0AUTH EXTERNAL {uid}\r\n").as_bytes())
        .map_err(|err| tray_error(&err.to_string()))?;
    let line = read_line(stream)?;
    if !line.starts_with("OK") {
        return Err(tray_error("session bus rejected the tray"));
    }
    stream
        .write_all(b"BEGIN\r\n")
        .map_err(|err| tray_error(&err.to_string()))?;
    let body = crate::tray_tip(count);
    let call = method_call(
        "org.kde.StatusNotifierWatcher",
        "/StatusNotifierWatcher",
        "org.kde.StatusNotifierWatcher",
        "RegisterStatusNotifierItem",
        &body,
    );
    stream
        .write_all(&call)
        .map_err(|err| tray_error(&err.to_string()))?;
    match read_line(stream) {
        Ok(reply) if reply.starts_with("ERR") => {
            Err(tray_error("status icon watcher rejected the tray"))
        }
        Ok(_) => Ok(()),
        Err(err) => Err(err),
    }
}

pub(crate) fn raise_terminal(path: &Path) {
    if let Ok(mut tty) = std::fs::OpenOptions::new().write(true).open(path) {
        let _ = tty.write_all(b"\x1b[5t");
    }
}

pub(crate) fn next_member(stream: &mut dyn Read) -> Option<String> {
    let mut buf = [0u8; 64];
    match stream.read(&mut buf) {
        Ok(0) | Err(_) => None,
        Ok(n) => {
            let text = String::from_utf8_lossy(&buf[..n]);
            if text.contains("Activate") {
                Some("Activate".to_owned())
            } else {
                Some(String::new())
            }
        }
    }
}

fn unix_path(address: &str) -> Result<&str, Error> {
    for part in address.split(',') {
        if let Some(path) = part.strip_prefix("unix:path=") {
            return Ok(path);
        }
    }
    Err(tray_error("session bus has no unix path"))
}

#[allow(clippy::format_collect)]
#[rustfmt::skip]
fn hex_uid() -> String {
    uid().to_string().bytes().map(|byte| format!("{byte:02x}")).collect() }

fn uid() -> u32 {
    unsafe { getuid() }
}

unsafe extern "C" {
    fn getuid() -> u32;
}

fn method_call(dest: &str, path: &str, interface: &str, member: &str, body: &str) -> Vec<u8> {
    let mut bytes = Vec::new();
    bytes.extend(dest.as_bytes());
    bytes.push(0);
    bytes.extend(path.as_bytes());
    bytes.push(0);
    bytes.extend(interface.as_bytes());
    bytes.push(0);
    bytes.extend(member.as_bytes());
    bytes.push(0);
    bytes.extend(body.as_bytes());
    bytes
}

fn read_line(stream: &mut dyn Io) -> Result<String, Error> {
    let mut out = Vec::new();
    let mut byte = [0u8; 1];
    loop {
        let n = stream
            .read(&mut byte)
            .map_err(|err| tray_error(&err.to_string()))?;
        if n == 0 {
            break;
        }
        if byte[0] == b'\n' {
            break;
        }
        out.push(byte[0]);
        if out.len() > 512 {
            break;
        }
    }
    Ok(String::from_utf8_lossy(&out).into_owned())
}

#[rustfmt::skip]
fn tray_error(message: &str) -> Error {
    Error::Failed { program: "tray".to_owned(), message: message.to_owned() } }
