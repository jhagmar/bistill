//! StatusNotifierItem on the session bus.
//!
//! The socket itself is safe Rust. `getuid` is not in the standard library, so
//! the auth token uses one `unsafe` call. The tray thread keeps this connection
//! until quit and answers the watcher’s property reads. `Activate` asks the
//! terminal to raise its window.

#![allow(unsafe_code)]

use crate::Error;
use std::io::{Read, Write};
use std::os::unix::io::AsRawFd;
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::Duration;

const ITEM: &str = "/StatusNotifierItem";
const ITEM_IFACE: &str = "org.kde.StatusNotifierItem";
const WATCHER: &str = "org.kde.StatusNotifierWatcher";
const PROPS: &str = "org.freedesktop.DBus.Properties";

pub(crate) fn run(rx: std::sync::mpsc::Receiver<u64>, log: &mut dyn FnMut(&str)) {
    let address = std::env::var("DBUS_SESSION_BUS_ADDRESS").ok();
    run_at(rx, address.as_deref(), Path::new("/dev/tty"), log);
}

pub(crate) fn run_at(
    rx: std::sync::mpsc::Receiver<u64>,
    address: Option<&str>,
    tty: &Path,
    log: &mut dyn FnMut(&str),
) {
    let mut logged = false;
    let mut live: Option<Live> = None;
    let latest = Mutex::new(None);
    let (mut poke_r, poke_w) = UnixStream::pair().expect("socketpair");
    let _ = poke_r.set_nonblocking(true);
    std::thread::scope(|scope| {
        scope.spawn(|| poke_counts(rx, poke_w, &latest));
        loop {
            let bus = live.as_ref().map(|item| item.raw_fd());
            let (bus_ready, poke_ready) = wait_ready(bus, poke_r.as_raw_fd());
            if poke_ready
                && take_count(
                    &mut poke_r,
                    &latest,
                    &mut live,
                    address,
                    tty,
                    log,
                    &mut logged,
                )
            {
                break;
            }
            if bus_ready {
                poll_bus(&mut live, log, &mut logged);
            }
        }
    });
}

fn poke_counts(
    rx: std::sync::mpsc::Receiver<u64>,
    mut poke_w: UnixStream,
    latest: &Mutex<Option<u64>>,
) {
    while let Ok(count) = rx.recv() {
        *latest.lock().unwrap() = Some(count);
        let _ = poke_w.write_all(&[1]);
    }
    let _ = poke_w.write_all(&[0]);
}

fn take_count(
    poke: &mut UnixStream,
    latest: &Mutex<Option<u64>>,
    live: &mut Option<Live>,
    address: Option<&str>,
    tty: &Path,
    log: &mut dyn FnMut(&str),
    logged: &mut bool,
) -> bool {
    let stop = drain_poke(poke);
    if let Some(count) = latest.lock().unwrap().take() {
        if let Err(err) = open_or_update(live, address, tty, count) {
            *live = None;
            report(logged, log, &err);
        }
    }
    stop
}

fn poll_bus(live: &mut Option<Live>, log: &mut dyn FnMut(&str), logged: &mut bool) {
    let Some(item) = live.as_mut() else {
        return;
    };
    if let Err(err) = item.poll() {
        *live = None;
        report(logged, log, &err);
    }
}

#[cfg(test)]
pub(crate) fn poll_without_a_tray() {
    fn note(message: &str) {
        let _ = message.len();
    }
    note("");
    let mut live = None;
    let mut logged = false;
    poll_bus(&mut live, &mut note, &mut logged);
}

pub(crate) fn drain_poke(poke: &mut dyn Read) -> bool {
    let mut buf = [0u8; 32];
    let mut stop = false;
    loop {
        match poke.read(&mut buf) {
            Ok(0) => return true,
            Ok(n) => {
                if buf[..n].contains(&0) {
                    stop = true;
                }
            }
            Err(err) if err.kind() == std::io::ErrorKind::WouldBlock => return stop,
            Err(_) => return true,
        }
    }
}

#[repr(C)]
struct PollFd {
    fd: i32,
    events: i16,
    revents: i16,
}

const POLLIN: i16 = 1;
const POLLERR: i16 = 8;
const POLLHUP: i16 = 16;

unsafe extern "C" {
    fn poll(fds: *mut PollFd, nfds: u64, timeout: i32) -> i32;
}

fn wait_ready(bus: Option<i32>, poke: i32) -> (bool, bool) {
    ready_flags(bus.is_some(), poll_once(bus, poke))
}

pub(crate) fn ready_flags(has_bus: bool, ready: Option<(bool, bool)>) -> (bool, bool) {
    match ready {
        Some((bus_ready, poke_ready)) if bus_ready || poke_ready => (bus_ready, poke_ready),
        Some(_) => (false, false),
        None if has_bus => (true, false),
        None => (false, true),
    }
}

pub(crate) fn poll_once(bus: Option<i32>, poke: i32) -> Option<(bool, bool)> {
    poll_timed(bus, poke, -1)
}

pub(crate) fn poll_timed(bus: Option<i32>, poke: i32, timeout: i32) -> Option<(bool, bool)> {
    let mut fds = [
        PollFd {
            fd: poke,
            events: POLLIN,
            revents: 0,
        },
        PollFd {
            fd: bus.unwrap_or(poke),
            events: POLLIN,
            revents: 0,
        },
    ];
    let count = if bus.is_some() { 2 } else { 1 };
    let ready = unsafe { poll(fds.as_mut_ptr(), count, timeout) };
    if ready <= 0 {
        return None;
    }
    let mark = POLLIN | POLLERR | POLLHUP;
    let poke_ready = (fds[0].revents & mark) != 0;
    let bus_ready = count == 2 && (fds[1].revents & mark) != 0;
    Some((bus_ready, poke_ready))
}

fn open_or_update(
    live: &mut Option<Live>,
    address: Option<&str>,
    tty: &Path,
    count: u64,
) -> Result<(), Error> {
    if let Some(item) = live.as_mut() {
        item.update(count)
    } else {
        *live = Some(Live::connect(address, tty, count)?);
        Ok(())
    }
}

fn report(logged: &mut bool, log: &mut dyn FnMut(&str), err: &Error) {
    if !*logged {
        *logged = true;
        log(&format!("{err}\n"));
    }
}

struct Live {
    stream: UnixStream,
    name: String,
    serial: u32,
    count: u64,
    tty: PathBuf,
}

impl Live {
    fn connect(address: Option<&str>, tty: &Path, count: u64) -> Result<Self, Error> {
        let Some(address) = address else {
            return Err(tray_error("no status icon watcher"));
        };
        let path = unix_path(address)?;
        let mut stream = UnixStream::connect(path).map_err(|err| io_err(&err))?;
        let _ = stream.set_read_timeout(Some(Duration::from_millis(200)));
        let ready = handshake(&mut stream)?;
        let mut live = Live {
            stream,
            name: ready.name,
            serial: ready.serial,
            count,
            tty: tty.to_owned(),
        };
        for msg in &ready.pending {
            live.dispatch(msg)?;
        }
        Ok(live)
    }

    fn raw_fd(&self) -> i32 {
        self.stream.as_raw_fd()
    }

    fn update(&mut self, count: u64) -> Result<(), Error> {
        self.count = count;
        self.signal("NewIcon", "", &[])?;
        self.signal("NewToolTip", "", &[])?;
        let mut body = Buf::new();
        body.string(ITEM_IFACE);
        write_changed(&mut body, self.count);
        body.array_empty();
        self.signal("PropertiesChanged", "sa{sv}as", &body.data)?;
        Ok(())
    }

    fn signal(&mut self, member: &str, signature: &str, body: &[u8]) -> Result<(), Error> {
        self.serial = self.serial.wrapping_add(1);
        let iface = if member == "PropertiesChanged" {
            PROPS
        } else {
            ITEM_IFACE
        };
        let bytes = if signature.is_empty() {
            message(
                4,
                self.serial,
                &[
                    Field::Path(ITEM),
                    Field::Interface(iface),
                    Field::Member(member),
                    Field::Sender(&self.name),
                ],
                body,
            )
        } else {
            message(
                4,
                self.serial,
                &[
                    Field::Path(ITEM),
                    Field::Interface(iface),
                    Field::Member(member),
                    Field::Sender(&self.name),
                    Field::Signature(signature),
                ],
                body,
            )
        };
        self.stream.write_all(&bytes).map_err(|err| io_err(&err))
    }

    fn poll(&mut self) -> Result<(), Error> {
        loop {
            match pull(&mut self.stream)? {
                None => return Ok(()),
                Some(msg) => self.dispatch(&msg)?,
            }
        }
    }

    fn dispatch(&mut self, msg: &Msg) -> Result<(), Error> {
        if msg.kind != 1 {
            return Ok(());
        }
        if msg.member == "Activate" {
            raise_terminal(&self.tty);
            self.reply(msg, "", &[])
        } else if msg.member == "GetAll" {
            let mut body = Buf::new();
            write_props(&mut body, self.count);
            self.reply(msg, "a{sv}", &body.data)
        } else if msg.member == "Get" {
            self.reply_get(msg)
        } else if msg.member == "Introspect" {
            let mut body = Buf::new();
            body.string(INTROSPECT);
            self.reply(msg, "s", &body.data)
        } else {
            let mut body = Buf::new();
            body.string("unknown method");
            self.error(msg, "org.freedesktop.DBus.Error.UnknownMethod", &body.data)
        }
    }

    fn reply_get(&mut self, msg: &Msg) -> Result<(), Error> {
        let mut reader = Reader {
            data: &msg.raw,
            at: msg.body_at,
        };
        let _iface = reader.string()?;
        let name = reader.string()?;
        let mut body = Buf::new();
        if write_one(&mut body, &name, self.count) {
            self.reply(msg, "v", &body.data)
        } else {
            let mut missing = Buf::new();
            missing.string("unknown property");
            self.error(
                msg,
                "org.freedesktop.DBus.Error.UnknownProperty",
                &missing.data,
            )
        }
    }

    fn reply(&mut self, msg: &Msg, signature: &str, body: &[u8]) -> Result<(), Error> {
        self.serial = self.serial.wrapping_add(1);
        let bytes = if signature.is_empty() {
            message(
                2,
                self.serial,
                &[
                    Field::Reply(msg.serial),
                    Field::Destination(&msg.sender),
                    Field::Sender(&self.name),
                ],
                body,
            )
        } else {
            message(
                2,
                self.serial,
                &[
                    Field::Reply(msg.serial),
                    Field::Destination(&msg.sender),
                    Field::Sender(&self.name),
                    Field::Signature(signature),
                ],
                body,
            )
        };
        self.stream.write_all(&bytes).map_err(|err| io_err(&err))
    }

    fn error(&mut self, msg: &Msg, name: &str, body: &[u8]) -> Result<(), Error> {
        self.serial = self.serial.wrapping_add(1);
        let bytes = message(
            3,
            self.serial,
            &[
                Field::ErrorName(name),
                Field::Reply(msg.serial),
                Field::Destination(&msg.sender),
                Field::Sender(&self.name),
                Field::Signature("s"),
            ],
            body,
        );
        self.stream.write_all(&bytes).map_err(|err| io_err(&err))
    }
}

pub(crate) struct Ready {
    name: String,
    serial: u32,
    pending: Vec<Msg>,
}

pub(crate) fn handshake(io: &mut dyn Io) -> Result<Ready, Error> {
    let uid = hex_uid();
    io.write_all(format!("\0AUTH EXTERNAL {uid}\r\n").as_bytes())
        .map_err(|err| io_err(&err))?;
    let line = read_line(io)?;
    if !line.starts_with("OK") {
        return Err(tray_error("session bus rejected the tray"));
    }
    io.write_all(b"BEGIN\r\n").map_err(|err| io_err(&err))?;
    let hello = message(
        1,
        1,
        &[
            Field::Path("/org/freedesktop/DBus"),
            Field::Destination("org.freedesktop.DBus"),
            Field::Interface("org.freedesktop.DBus"),
            Field::Member("Hello"),
        ],
        &[],
    );
    io.write_all(&hello).map_err(|err| io_err(&err))?;
    let (hello_reply, mut pending) = wait_reply(io, 1)?;
    if hello_reply.kind != 2 {
        return Err(tray_error("session bus rejected the tray"));
    }
    let mut reader = Reader {
        data: &hello_reply.raw,
        at: hello_reply.body_at,
    };
    let name = reader.string()?;
    let register_body = {
        let mut body = Buf::new();
        body.string(&name);
        body.data
    };
    let register = message(
        1,
        2,
        &[
            Field::Path("/StatusNotifierWatcher"),
            Field::Destination(WATCHER),
            Field::Interface(WATCHER),
            Field::Member("RegisterStatusNotifierItem"),
            Field::Sender(&name),
            Field::Signature("s"),
        ],
        &register_body,
    );
    io.write_all(&register).map_err(|err| io_err(&err))?;
    let (registered, more) = wait_reply(io, 2)?;
    pending.extend(more);
    if registered.kind == 2 {
        Ok(Ready {
            name,
            serial: 2,
            pending,
        })
    } else {
        Err(tray_error("status icon watcher rejected the tray"))
    }
}

fn wait_reply(io: &mut dyn Io, serial: u32) -> Result<(Msg, Vec<Msg>), Error> {
    let mut pending = Vec::new();
    loop {
        match pull(io)? {
            None => return Err(tray_error("status icon watcher timed out")),
            Some(msg) if msg.reply_serial == Some(serial) => return Ok((msg, pending)),
            Some(msg) if msg.kind == 1 => pending.push(msg),
            Some(_) => {}
        }
    }
}

pub(crate) fn pull(io: &mut dyn Io) -> Result<Option<Msg>, Error> {
    let mut head = [0u8; 16];
    match io.read(&mut head[..1]) {
        Ok(0) => return Err(tray_error("status icon watcher closed the tray")),
        Ok(_) => {}
        Err(err) if idle(&err) => return Ok(None),
        Err(err) => return Err(io_err(&err)),
    }
    read_full(io, &mut head[1..])?;
    if head[0] != b'l' {
        return Err(tray_error("status icon watcher rejected the tray"));
    }
    let body_len = u32::from_le_bytes([head[4], head[5], head[6], head[7]]) as usize;
    let fields_len = u32::from_le_bytes([head[12], head[13], head[14], head[15]]) as usize;
    if body_len > 1_048_576 {
        return Err(tray_error("status icon watcher rejected the tray"));
    }
    if fields_len > 1_048_576 {
        return Err(tray_error("status icon watcher rejected the tray"));
    }
    let mut rest = vec![0u8; fields_len];
    read_full(io, &mut rest)?;
    let mut raw = head.to_vec();
    raw.extend_from_slice(&rest);
    let body_at = align_len(raw.len(), 8);
    if body_at > raw.len() {
        let mut pad = vec![0u8; body_at - raw.len()];
        read_full(io, &mut pad)?;
        raw.extend_from_slice(&pad);
    }
    let mut body = vec![0u8; body_len];
    read_full(io, &mut body)?;
    raw.extend_from_slice(&body);
    Ok(Some(decode(&raw, body_at)?))
}

fn idle(err: &std::io::Error) -> bool {
    if err.kind() == std::io::ErrorKind::TimedOut {
        true
    } else {
        err.kind() == std::io::ErrorKind::WouldBlock
    }
}

fn read_full(io: &mut dyn Io, buf: &mut [u8]) -> Result<(), Error> {
    let mut at = 0;
    while at < buf.len() {
        match io.read(&mut buf[at..]) {
            Ok(0) => return Err(tray_error("status icon watcher closed the tray")),
            Ok(n) => at += n,
            Err(err) if idle(&err) => return Err(tray_error("status icon watcher timed out")),
            Err(err) => return Err(io_err(&err)),
        }
    }
    Ok(())
}

fn align_len(len: usize, align: usize) -> usize {
    let n = (align - (len % align)) % align;
    len + n
}

pub(crate) struct Msg {
    pub(crate) kind: u8,
    serial: u32,
    reply_serial: Option<u32>,
    pub(crate) member: String,
    sender: String,
    pub(crate) raw: Vec<u8>,
    body_at: usize,
}

pub(crate) fn decode(raw: &[u8], body_at: usize) -> Result<Msg, Error> {
    let serial = u32::from_le_bytes([raw[8], raw[9], raw[10], raw[11]]);
    let fields_len = u32::from_le_bytes([raw[12], raw[13], raw[14], raw[15]]) as usize;
    let mut reader = Reader { data: raw, at: 16 };
    let end = 16 + fields_len;
    let mut reply_serial = None;
    let mut member = String::new();
    let mut sender = String::new();
    while reader.at < end {
        reader.pad(8)?;
        let code = reader.byte()?;
        let kind = reader.signature()?;
        if code == 3 {
            member = reader.dbus_string()?;
        } else if code == 5 {
            reply_serial = Some(reader.u32()?);
        } else if code == 7 {
            sender = reader.dbus_string()?;
        } else if kind == "s" || kind == "o" {
            let _ = reader.dbus_string()?;
        } else if kind == "g" {
            let _ = reader.signature()?;
        } else if kind == "u" {
            let _ = reader.u32()?;
        } else {
            return Err(tray_error("status icon watcher rejected the tray"));
        }
    }
    Ok(Msg {
        kind: raw[1],
        serial,
        reply_serial,
        member,
        sender,
        raw: raw.to_vec(),
        body_at,
    })
}

enum Field<'a> {
    Path(&'a str),
    Interface(&'a str),
    Member(&'a str),
    ErrorName(&'a str),
    Reply(u32),
    Destination(&'a str),
    Sender(&'a str),
    Signature(&'a str),
    #[allow(dead_code)]
    Number(u8, u32),
    #[allow(dead_code)]
    Bad,
}

fn message(kind: u8, serial: u32, fields: &[Field<'_>], body: &[u8]) -> Vec<u8> {
    let mut encoded = Buf::new();
    for field in fields {
        encoded.pad(8);
        match field {
            Field::Path(value) => {
                encoded.byte(1);
                encoded.variant_path(value);
            }
            Field::Interface(value) => {
                encoded.byte(2);
                encoded.variant_string(value);
            }
            Field::Member(value) => {
                encoded.byte(3);
                encoded.variant_string(value);
            }
            Field::ErrorName(value) => {
                encoded.byte(4);
                encoded.variant_string(value);
            }
            Field::Reply(value) => {
                encoded.byte(5);
                encoded.variant_u32(*value);
            }
            Field::Destination(value) => {
                encoded.byte(6);
                encoded.variant_string(value);
            }
            Field::Sender(value) => {
                encoded.byte(7);
                encoded.variant_string(value);
            }
            Field::Signature(value) => {
                encoded.byte(8);
                encoded.variant_signature(value);
            }
            Field::Number(code, value) => {
                encoded.byte(*code);
                encoded.variant_u32(*value);
            }
            Field::Bad => {
                encoded.byte(1);
                encoded.signature("b");
                encoded.u32(1);
            }
        }
    }
    let mut out = Buf::new();
    out.byte(b'l');
    out.byte(kind);
    out.byte(0);
    out.byte(1);
    out.u32(body.len() as u32);
    out.u32(serial);
    out.u32(encoded.data.len() as u32);
    out.data.extend_from_slice(&encoded.data);
    out.pad(8);
    out.data.extend_from_slice(body);
    out.data
}

#[cfg(test)]
pub(crate) fn script_return(reply: u32, text: &str) -> Vec<u8> {
    let mut body = Buf::new();
    body.string(text);
    message(
        2,
        9,
        &[
            Field::Reply(reply),
            Field::Destination(":1.9"),
            Field::Sender(":1.1"),
            Field::Signature("s"),
            Field::Path("/org/freedesktop/DBus"),
        ],
        &body.data,
    )
}

#[cfg(test)]
pub(crate) fn script_error(reply: u32) -> Vec<u8> {
    let mut body = Buf::new();
    body.string("no");
    message(
        3,
        9,
        &[
            Field::ErrorName("org.freedesktop.DBus.Error.ServiceUnknown"),
            Field::Reply(reply),
            Field::Destination(":1.9"),
            Field::Signature("s"),
        ],
        &body.data,
    )
}

#[cfg(test)]
pub(crate) fn script_call(member: &str, signature: &str, body: &[u8]) -> Vec<u8> {
    message(
        1,
        4,
        &[
            Field::Path(ITEM),
            Field::Interface(PROPS),
            Field::Member(member),
            Field::Sender(":1.50"),
            Field::Destination(":1.9"),
            Field::Signature(signature),
        ],
        body,
    )
}

#[cfg(test)]
pub(crate) fn script_string(text: &str) -> Vec<u8> {
    let mut body = Buf::new();
    body.string(text);
    body.data
}

#[cfg(test)]
pub(crate) fn script_two_strings(left: &str, right: &str) -> Vec<u8> {
    let mut body = Buf::new();
    body.string(left);
    body.string(right);
    body.data
}

#[cfg(test)]
pub(crate) fn script_odd() -> Vec<u8> {
    message(4, 1, &[Field::Number(9, 1), Field::Member("Ping")], &[])
}

#[cfg(test)]
pub(crate) fn script_bad() -> Vec<u8> {
    message(4, 1, &[Field::Bad], &[])
}

#[cfg(test)]
pub(crate) fn script_header(endian: u8, body_len: u32, fields_len: u32) -> Vec<u8> {
    let mut head = vec![endian, 1, 0, 1];
    head.extend_from_slice(&body_len.to_le_bytes());
    head.extend_from_slice(&1u32.to_le_bytes());
    head.extend_from_slice(&fields_len.to_le_bytes());
    head
}

fn write_changed(body: &mut Buf, count: u64) {
    body.pad(4);
    let len_at = body.data.len();
    body.u32(0);
    body.pad(8);
    let start = body.data.len();
    entry_string(body, "IconName", icon_name(count));
    entry_pixmap(body, "IconPixmap");
    entry_tooltip(body, count);
    let len = (body.data.len() - start) as u32;
    body.data[len_at..len_at + 4].copy_from_slice(&len.to_le_bytes());
}

fn write_props(body: &mut Buf, count: u64) {
    body.pad(4);
    let len_at = body.data.len();
    body.u32(0);
    body.pad(8);
    let start = body.data.len();
    entry_string(body, "Category", "ApplicationStatus");
    entry_string(body, "Id", "bistill");
    entry_string(body, "Title", "bistill");
    entry_string(body, "Status", "Active");
    entry_string(body, "IconName", icon_name(count));
    entry_pixmap(body, "IconPixmap");
    entry_tooltip(body, count);
    let len = (body.data.len() - start) as u32;
    body.data[len_at..len_at + 4].copy_from_slice(&len.to_le_bytes());
}

fn write_one(body: &mut Buf, name: &str, count: u64) -> bool {
    if name == "Category" {
        body.variant_string("ApplicationStatus");
        true
    } else if name == "Id" || name == "Title" {
        body.variant_string("bistill");
        true
    } else if name == "Status" {
        body.variant_string("Active");
        true
    } else if name == "IconName" {
        body.variant_string(icon_name(count));
        true
    } else if name == "IconPixmap" {
        body.signature("a(iiay)");
        write_pixmap(body);
        true
    } else if name == "ToolTip" {
        body.signature("(sa(iiay)ss)");
        write_tooltip(body, count);
        true
    } else {
        false
    }
}

fn entry_string(body: &mut Buf, key: &str, value: &str) {
    body.pad(8);
    body.string(key);
    body.variant_string(value);
}

fn entry_pixmap(body: &mut Buf, key: &str) {
    body.pad(8);
    body.string(key);
    body.signature("a(iiay)");
    write_pixmap(body);
}

fn entry_tooltip(body: &mut Buf, count: u64) {
    body.pad(8);
    body.string("ToolTip");
    body.signature("(sa(iiay)ss)");
    write_tooltip(body, count);
}

fn write_tooltip(body: &mut Buf, count: u64) {
    body.pad(8);
    body.string(icon_name(count));
    body.u32(0);
    // The pixmap array is empty. Its element is a struct, so the alignment after the length is still written.
    body.pad(8);
    body.string("bistill");
    body.string(&crate::tray_tip(count));
}

fn write_pixmap(body: &mut Buf) {
    let pixels = icon_argb();
    body.pad(4);
    let len_at = body.data.len();
    body.u32(0);
    body.pad(8);
    let start = body.data.len();
    body.pad(8);
    body.i32(16);
    body.i32(16);
    body.pad(4);
    body.u32(pixels.len() as u32);
    body.data.extend_from_slice(&pixels);
    let len = (body.data.len() - start) as u32;
    body.data[len_at..len_at + 4].copy_from_slice(&len.to_le_bytes());
}

fn icon_name(count: u64) -> &'static str {
    if count == 0 {
        "mail-read"
    } else {
        "mail-unread"
    }
}

fn icon_argb() -> Vec<u8> {
    let mut bytes = Vec::with_capacity(16 * 16 * 4);
    for y in 0..16 {
        for x in 0..16 {
            let edge = x < 2 || y < 2 || x > 13 || y > 13;
            if edge {
                bytes.extend_from_slice(&[255, 40, 90, 180]);
            } else {
                bytes.extend_from_slice(&[255, 230, 236, 245]);
            }
        }
    }
    bytes
}

const INTROSPECT: &str = concat!(
    "<node>",
    "<interface name=\"org.kde.StatusNotifierItem\">",
    "<method name=\"Activate\"/>",
    "<property name=\"IconName\" type=\"s\" access=\"read\"/>",
    "<signal name=\"NewIcon\"/>",
    "<signal name=\"NewToolTip\"/>",
    "</interface>",
    "</node>"
);

struct Buf {
    data: Vec<u8>,
}

impl Buf {
    fn new() -> Self {
        Buf { data: Vec::new() }
    }

    fn pad(&mut self, align: usize) {
        let n = (align - (self.data.len() % align)) % align;
        self.data.resize(self.data.len() + n, 0);
    }

    fn byte(&mut self, value: u8) {
        self.data.push(value);
    }

    fn u32(&mut self, value: u32) {
        self.pad(4);
        self.data.extend_from_slice(&value.to_le_bytes());
    }

    fn i32(&mut self, value: i32) {
        self.pad(4);
        self.data.extend_from_slice(&value.to_le_bytes());
    }

    fn string(&mut self, value: &str) {
        self.u32(value.len() as u32);
        self.data.extend_from_slice(value.as_bytes());
        self.data.push(0);
    }

    fn signature(&mut self, value: &str) {
        self.data.push(value.len() as u8);
        self.data.extend_from_slice(value.as_bytes());
        self.data.push(0);
    }

    fn variant_string(&mut self, value: &str) {
        self.signature("s");
        self.string(value);
    }

    fn variant_path(&mut self, value: &str) {
        self.signature("o");
        self.string(value);
    }

    fn variant_signature(&mut self, value: &str) {
        self.signature("g");
        self.signature(value);
    }

    fn variant_u32(&mut self, value: u32) {
        self.signature("u");
        self.u32(value);
    }

    fn array_empty(&mut self) {
        self.u32(0);
    }
}

struct Reader<'a> {
    data: &'a [u8],
    at: usize,
}

impl Reader<'_> {
    fn pad(&mut self, align: usize) -> Result<(), Error> {
        let n = (align - (self.at % align)) % align;
        self.at += n;
        if self.at > self.data.len() {
            Err(tray_error("status icon watcher rejected the tray"))
        } else {
            Ok(())
        }
    }

    fn byte(&mut self) -> Result<u8, Error> {
        if self.at >= self.data.len() {
            Err(tray_error("status icon watcher rejected the tray"))
        } else {
            let value = self.data[self.at];
            self.at += 1;
            Ok(value)
        }
    }

    fn u32(&mut self) -> Result<u32, Error> {
        let aligned = align_len(self.at, 4);
        if aligned + 4 > self.data.len() {
            return Err(tray_error("status icon watcher rejected the tray"));
        }
        self.at = aligned;
        let value = u32::from_le_bytes(self.data[self.at..self.at + 4].try_into().unwrap());
        self.at += 4;
        Ok(value)
    }

    fn signature(&mut self) -> Result<String, Error> {
        let len = usize::from(self.byte()?);
        if self.at + len + 1 > self.data.len() {
            return Err(tray_error("status icon watcher rejected the tray"));
        }
        let text = String::from_utf8_lossy(&self.data[self.at..self.at + len]).into_owned();
        self.at += len + 1;
        Ok(text)
    }

    fn dbus_string(&mut self) -> Result<String, Error> {
        let len = self.u32()? as usize;
        if self.at + len + 1 > self.data.len() {
            return Err(tray_error("status icon watcher rejected the tray"));
        }
        let text = String::from_utf8_lossy(&self.data[self.at..self.at + len]).into_owned();
        self.at += len + 1;
        Ok(text)
    }

    fn string(&mut self) -> Result<String, Error> {
        self.dbus_string()
    }
}

// One trait object, so the handshake and the error paths are a single function.
pub(crate) trait Io: Read + Write {}

impl<T: Read + Write> Io for T {}

pub(crate) fn raise_terminal(path: &Path) {
    if let Ok(mut tty) = std::fs::OpenOptions::new().write(true).open(path) {
        let _ = tty.write_all(b"\x1b[5t");
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

#[cfg(test)]
unsafe extern "C" {
    fn setsockopt(fd: i32, level: i32, opt: i32, val: *const i32, len: u32) -> i32;
}

#[cfg(test)]
pub(crate) fn updates_fail_when_the_socket_stops_reading() {
    use std::os::unix::io::AsRawFd;
    let (stream, mut peer) = UnixStream::pair().unwrap();
    let size = 1024i32;
    let fd = stream.as_raw_fd();
    assert_eq!(unsafe { setsockopt(fd, 1, 7, &size, 4) }, 0);
    let _ = stream.set_write_timeout(Some(Duration::from_millis(50)));
    let mut live = Live {
        stream,
        name: ":1.1".to_owned(),
        serial: 2,
        count: 0,
        tty: PathBuf::from("/dev/null"),
    };
    // The first update fits. Reading a little back leaves room for the two
    // small signals and not for the pixmap, so the next update fails there.
    assert!(live.update(0).is_ok());
    let mut discard = [0u8; 200];
    assert_eq!(peer.read(&mut discard).ok(), Some(200));
    assert!(live.update(4).is_err());
    full_socket_rejects("Activate");
    full_socket_rejects("Nope");
}

#[cfg(test)]
fn full_socket_rejects(member: &str) {
    use std::os::unix::io::AsRawFd;
    let (mut stream, mut peer) = UnixStream::pair().unwrap();
    let size = 1024i32;
    let fd = stream.as_raw_fd();
    assert_eq!(unsafe { setsockopt(fd, 1, 7, &size, 4) }, 0);
    let _ = stream.set_write_timeout(Some(Duration::from_millis(50)));
    let _ = stream.set_read_timeout(Some(Duration::from_millis(200)));
    assert!(stream.write_all(&vec![0u8; 3000]).is_ok());
    let call = script_call(member, "s", &script_string(""));
    assert!(peer.write_all(&call).is_ok());
    let mut live = Live {
        stream,
        name: ":1.1".to_owned(),
        serial: 2,
        count: 0,
        tty: PathBuf::from("/dev/null"),
    };
    assert!(live.poll().is_err());
}

unsafe extern "C" {
    fn getuid() -> u32;
}

fn read_line(stream: &mut dyn Io) -> Result<String, Error> {
    let mut out = Vec::new();
    let mut byte = [0u8; 1];
    loop {
        let n = stream.read(&mut byte).map_err(|err| io_err(&err))?;
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

fn io_err(err: &std::io::Error) -> Error {
    tray_error(&err.to_string())
}

#[rustfmt::skip]
fn tray_error(message: &str) -> Error {
    Error::Failed { program: "tray".to_owned(), message: message.to_owned() } }
