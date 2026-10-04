//! Unix console mode.
//!
//! The standard library cannot set terminal attributes, wait on a file
//! descriptor with a timeout, or read the window size. The `unsafe` blocks
//! below are the small wrapper around those calls.

#![allow(unsafe_code)]

use std::fs::File;
use std::io::{self, Read, Write};
use std::os::unix::io::{AsRawFd, FromRawFd};
use std::sync::{Mutex, MutexGuard, PoisonError};
use std::time::Duration;

const TCGETS: u64 = 0x5401;
const TCSETS: u64 = 0x5402;
const TIOCGWINSZ: u64 = 0x5413;
const F_DUPFD_CLOEXEC: i32 = 1030;
const POLLIN: i16 = 1;
#[cfg(test)]
const TIOCSWINSZ: u64 = 0x5414;
#[cfg(test)]
const TIOCGPTN: u64 = 0x80045430;
#[cfg(test)]
const TIOCSPTLCK: u64 = 0x40045431;
#[cfg(test)]
const O_RDWR: i32 = 2;
#[cfg(test)]
const O_NOCTTY: i32 = 0o400;
#[cfg(test)]
const O_CLOEXEC: i32 = 0x80000;

const IGNBRK: u32 = 0x1;
const BRKINT: u32 = 0x2;
const PARMRK: u32 = 0x8;
const ISTRIP: u32 = 0x20;
const INLCR: u32 = 0x40;
const IGNCR: u32 = 0x80;
const ICRNL: u32 = 0x100;
const IXON: u32 = 0x400;
const OPOST: u32 = 0x1;
const ECHO: u32 = 0x8;
const ECHONL: u32 = 0x40;
pub(crate) const ICANON: u32 = 0x2;
const ISIG: u32 = 0x1;
const IEXTEN: u32 = 0x8000;
const CSIZE: u32 = 0x30;
const PARENB: u32 = 0x100;
const CS8: u32 = 0x30;
const VTIME: usize = 5;
const VMIN: usize = 6;

#[repr(C)]
#[derive(Clone, Copy)]
struct Termios {
    iflag: u32,
    oflag: u32,
    cflag: u32,
    lflag: u32,
    line: u8,
    cc: [u8; 19],
}

#[repr(C)]
struct WinSize {
    row: u16,
    col: u16,
    xpixel: u16,
    ypixel: u16,
}

#[repr(C)]
struct PollFd {
    fd: i32,
    events: i16,
    revents: i16,
}

const _: () = assert!(size_of::<Termios>() == 36);
const _: () = assert!(size_of::<WinSize>() == 8);
const _: () = assert!(size_of::<PollFd>() == 8);

unsafe extern "C" {
    #[link_name = "ioctl"]
    fn unix_ioctl(fd: i32, request: u64, arg: *mut core::ffi::c_void) -> i32;
    #[link_name = "poll"]
    fn unix_poll(fds: *mut PollFd, nfds: u64, timeout: i32) -> i32;
    #[link_name = "write"]
    fn unix_write(fd: i32, buf: *const u8, count: usize) -> isize;
    #[link_name = "fcntl"]
    fn unix_fcntl(fd: i32, cmd: i32, arg: i32) -> i32;
}

#[cfg(test)]
unsafe extern "C" {
    #[link_name = "open"]
    fn unix_open(path: *const i8, flags: i32) -> i32;
    fn grantpt(fd: i32) -> i32;
}

struct Armed {
    input: i32,
    output: i32,
    saved: Termios,
}

static ARMED: Mutex<Option<Armed>> = Mutex::new(None);

/// A Unix terminal claimed for raw mode.
pub struct Console {
    input: File,
    output: File,
}

impl Console {
    /// Duplicates standard input and standard output and enables raw mode.
    pub fn stdio() -> io::Result<Self> {
        enable(dup_fd(0)?, dup_fd(1)?)
    }

    /// Columns and rows.
    pub fn size(&self) -> (u16, u16) {
        window_size(self.output.as_raw_fd()).unwrap_or((0, 0))
    }

    /// Writes `bytes` to the terminal.
    pub fn write(&mut self, bytes: &[u8]) {
        let _ = self.output.write_all(bytes);
        let _ = self.output.flush();
    }

    /// Waits until the terminal is readable or `timeout` elapses.
    pub fn wait(&self, timeout: Duration) -> bool {
        wait_fd(self.input.as_raw_fd(), timeout)
    }

    /// Reads pending input. A closed terminal yields zero.
    pub fn read(&mut self, buf: &mut [u8]) -> usize {
        read_count(&mut self.input, buf)
    }
}

impl Drop for Console {
    fn drop(&mut self) {
        disarm();
    }
}

#[cfg(test)]
pub(crate) fn from_files(input: File, output: File) -> io::Result<Console> {
    enable(input, output)
}

fn enable(input: File, output: File) -> io::Result<Console> {
    let saved = get_termios(input.as_raw_fd())?;
    let mut raw = saved;
    make_raw(&mut raw);
    set_termios(input.as_raw_fd(), &raw)?;
    arm(input.as_raw_fd(), output.as_raw_fd(), saved);
    Ok(Console { input, output })
}

fn make_raw(termios: &mut Termios) {
    termios.iflag &= !(IGNBRK | BRKINT | PARMRK | ISTRIP | INLCR | IGNCR | ICRNL | IXON);
    termios.oflag &= !OPOST;
    termios.lflag &= !(ECHO | ECHONL | ICANON | ISIG | IEXTEN);
    termios.cflag &= !(CSIZE | PARENB);
    termios.cflag |= CS8;
    termios.cc[VMIN] = 0;
    termios.cc[VTIME] = 0;
}

pub(crate) fn dup_fd(fd: i32) -> io::Result<File> {
    let cloned = unsafe { unix_fcntl(fd, F_DUPFD_CLOEXEC, 0) };
    if cloned < 0 {
        Err(io::Error::last_os_error())
    } else {
        // SAFETY: `fcntl` returned a new descriptor owned by this process.
        Ok(unsafe { File::from_raw_fd(cloned) })
    }
}

fn get_termios(fd: i32) -> io::Result<Termios> {
    let mut termios = blank_termios();
    if ioctl_ptr(
        fd,
        TCGETS,
        &mut termios as *mut Termios as *mut core::ffi::c_void,
    ) < 0
    {
        Err(io::Error::last_os_error())
    } else {
        Ok(termios)
    }
}

fn set_termios(fd: i32, termios: &Termios) -> io::Result<()> {
    let mut copy = *termios;
    if ioctl_ptr(
        fd,
        TCSETS,
        &mut copy as *mut Termios as *mut core::ffi::c_void,
    ) < 0
    {
        Err(io::Error::last_os_error())
    } else {
        Ok(())
    }
}

fn blank_termios() -> Termios {
    Termios {
        iflag: 0,
        oflag: 0,
        cflag: 0,
        lflag: 0,
        line: 0,
        cc: [0; 19],
    }
}

pub(crate) fn window_size(fd: i32) -> io::Result<(u16, u16)> {
    let mut size = WinSize {
        row: 0,
        col: 0,
        xpixel: 0,
        ypixel: 0,
    };
    if ioctl_ptr(
        fd,
        TIOCGWINSZ,
        &mut size as *mut WinSize as *mut core::ffi::c_void,
    ) < 0
    {
        Err(io::Error::last_os_error())
    } else {
        Ok((size.col, size.row))
    }
}

pub(crate) fn poll_millis(timeout: Duration) -> i32 {
    i32::try_from(timeout.as_millis()).unwrap_or(i32::MAX)
}

fn wait_fd(fd: i32, timeout: Duration) -> bool {
    let mut poll_fd = PollFd {
        fd,
        events: POLLIN,
        revents: 0,
    };
    // SAFETY: `poll_fd` is a live `pollfd` and `fd` is open.
    let ready = unsafe { unix_poll(&mut poll_fd, 1, poll_millis(timeout)) };
    ready > 0
}

pub(crate) fn read_count(file: &mut File, buf: &mut [u8]) -> usize {
    file.read(buf).unwrap_or_default()
}

fn arm(input: i32, output: i32, saved: Termios) {
    *lock_armed() = Some(Armed {
        input,
        output,
        saved,
    });
}

/// Leaves the alternate screen and restores the saved terminal mode.
pub fn disarm() {
    let armed = lock_armed().take();
    if let Some(armed) = armed {
        write_fd(armed.output, crate::terminal::LEAVE);
        let _ = set_termios(armed.input, &armed.saved);
    }
}

#[cfg(test)]
pub(crate) fn is_armed() -> bool {
    lock_armed().is_some()
}

fn lock_armed() -> MutexGuard<'static, Option<Armed>> {
    ARMED.lock().unwrap_or_else(PoisonError::into_inner)
}

fn write_fd(fd: i32, bytes: &[u8]) {
    // SAFETY: `bytes` is a live slice and `fd` is open while the console is armed.
    let _ = unsafe { unix_write(fd, bytes.as_ptr(), bytes.len()) };
}

fn ioctl_ptr(fd: i32, request: u64, arg: *mut core::ffi::c_void) -> i32 {
    // SAFETY: `arg` points at a live value whose layout matches `request`, and `fd` is open.
    unsafe { unix_ioctl(fd, request, arg) }
}

#[cfg(test)]
pub(crate) fn local_flags(file: &File) -> io::Result<u32> {
    get_termios(file.as_raw_fd()).map(|termios| termios.lflag)
}

#[cfg(test)]
pub(crate) fn set_winsize(file: &File, columns: u16, rows: u16) -> io::Result<()> {
    let mut size = WinSize {
        row: rows,
        col: columns,
        xpixel: 0,
        ypixel: 0,
    };
    if ioctl_ptr(
        file.as_raw_fd(),
        TIOCSWINSZ,
        &mut size as *mut WinSize as *mut core::ffi::c_void,
    ) < 0
    {
        Err(io::Error::last_os_error())
    } else {
        Ok(())
    }
}

#[cfg(test)]
pub(crate) fn open_pty() -> io::Result<(File, File)> {
    let master = open_noctty("/dev/ptmx")?;
    // SAFETY: `master` is an open pseudoterminal multiplexer.
    let _ = unsafe { grantpt(master.as_raw_fd()) };
    unlock_pty(master.as_raw_fd())?;
    let name = format!("/dev/pts/{}", pty_number(master.as_raw_fd())?);
    let slave = open_noctty(&name)?;
    Ok((master, slave))
}

#[cfg(test)]
pub(crate) fn open_noctty(path: &str) -> io::Result<File> {
    let mut bytes = path.as_bytes().to_vec();
    bytes.push(0);
    // SAFETY: `bytes` is a NUL-terminated path. A non-negative return is a new descriptor.
    let fd = unsafe { unix_open(bytes.as_ptr().cast(), O_RDWR | O_NOCTTY | O_CLOEXEC) };
    if fd < 0 {
        Err(io::Error::last_os_error())
    } else {
        Ok(unsafe { File::from_raw_fd(fd) })
    }
}

#[cfg(test)]
pub(crate) fn unlock_pty(fd: i32) -> io::Result<()> {
    let mut locked = 0i32;
    if ioctl_ptr(
        fd,
        TIOCSPTLCK,
        &mut locked as *mut i32 as *mut core::ffi::c_void,
    ) < 0
    {
        Err(io::Error::last_os_error())
    } else {
        Ok(())
    }
}

#[cfg(test)]
pub(crate) fn pty_number(fd: i32) -> io::Result<u32> {
    let mut number = 0u32;
    if ioctl_ptr(
        fd,
        TIOCGPTN,
        &mut number as *mut u32 as *mut core::ffi::c_void,
    ) < 0
    {
        Err(io::Error::last_os_error())
    } else {
        Ok(number)
    }
}

#[cfg(test)]
pub(crate) fn read_available(file: &mut File, timeout: Duration) -> Vec<u8> {
    let mut out = Vec::new();
    let mut buf = [0; 512];
    let mut remaining = timeout;
    loop {
        if !wait_fd(file.as_raw_fd(), remaining) {
            break;
        }
        let count = read_count(file, &mut buf);
        if count == 0 {
            break;
        }
        out.extend_from_slice(&buf[..count]);
        remaining = Duration::ZERO;
    }
    out
}

#[cfg(test)]
pub(crate) fn set_blank_termios(file: &File) -> io::Result<()> {
    set_termios(file.as_raw_fd(), &blank_termios())
}
