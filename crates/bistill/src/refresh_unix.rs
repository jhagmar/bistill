//! Block until `refresh` is created in the state directory.
//!
//! `inotify` reports the new name. A byte on a pipe cancels the wait.

#![allow(unsafe_code)]

use std::ffi::CString;
use std::io;
use std::os::unix::ffi::OsStrExt;
use std::path::Path;

const IN_CLOEXEC: i32 = 0x80000;
const IN_CREATE: u32 = 0x100;
const IN_MOVED_TO: u32 = 0x80;
const PIPE_CLOEXEC: i32 = 0x80000;
const O_NONBLOCK: i32 = 0o4000;
const POLLIN: i16 = 1;
const POLLERR: i16 = 8;
const POLLHUP: i16 = 16;

#[repr(C)]
struct PollFd {
    fd: i32,
    events: i16,
    revents: i16,
}

unsafe extern "C" {
    fn inotify_init1(flags: i32) -> i32;
    fn inotify_add_watch(fd: i32, path: *const core::ffi::c_char, mask: u32) -> i32;
    fn pipe2(fds: *mut i32, flags: i32) -> i32;
    fn poll(fds: *mut PollFd, nfds: u64, timeout: i32) -> i32;
    fn read(fd: i32, buf: *mut u8, count: usize) -> isize;
    fn write(fd: i32, buf: *const u8, count: usize) -> isize;
    fn close(fd: i32) -> i32;
}

/// An `inotify` watch plus a pipe that [`RefreshWatch::cancel`] writes.
pub(crate) struct RefreshWatch {
    notify: i32,
    poke_r: i32,
    poke_w: i32,
}

impl Drop for RefreshWatch {
    fn drop(&mut self) {
        close_fd(self.notify);
        close_fd(self.poke_r);
        close_fd(self.poke_w);
    }
}

impl RefreshWatch {
    /// `Ok(true)` when `refresh` was created. `Ok(false)` when [`Self::cancel`] ran.
    pub(crate) fn wait(&self) -> io::Result<bool> {
        loop {
            let (notify, poke) = poll_pair(self.notify, self.poke_r)?;
            if poke {
                drain_fd(self.poke_r);
                return Ok(false);
            }
            if notify && read_names(self.notify)? {
                return Ok(true);
            }
        }
    }

    /// Wakes [`Self::wait`].
    pub(crate) fn cancel(&self) {
        unsafe {
            write(self.poke_w, &1u8, 1);
        }
    }
}

/// Watch `dir` for a file named `refresh`.
pub(crate) fn open(dir: &Path) -> io::Result<RefreshWatch> {
    open_with(dir, IN_CLOEXEC)
}

pub(crate) fn open_with(dir: &Path, flags: i32) -> io::Result<RefreshWatch> {
    let (poke_r, poke_w) = pipe_pair(PIPE_CLOEXEC | O_NONBLOCK)?;
    let notify = match notify_fd(flags) {
        Ok(fd) => fd,
        Err(err) => {
            close_fd(poke_r);
            close_fd(poke_w);
            return Err(err);
        }
    };
    let path = match c_path(dir) {
        Ok(path) => path,
        Err(err) => {
            close_fd(notify);
            close_fd(poke_r);
            close_fd(poke_w);
            return Err(err);
        }
    };
    let wd = unsafe { inotify_add_watch(notify, path.as_ptr(), IN_CREATE | IN_MOVED_TO) };
    if wd < 0 {
        let err = io::Error::last_os_error();
        close_fd(notify);
        close_fd(poke_r);
        close_fd(poke_w);
        return Err(err);
    }
    Ok(RefreshWatch {
        notify,
        poke_r,
        poke_w,
    })
}

pub(crate) fn pipe_pair(flags: i32) -> io::Result<(i32, i32)> {
    let mut fds = [-1, -1];
    let rc = unsafe { pipe2(fds.as_mut_ptr(), flags) };
    if rc < 0 {
        Err(io::Error::last_os_error())
    } else {
        Ok((fds[0], fds[1]))
    }
}

pub(crate) fn notify_fd(flags: i32) -> io::Result<i32> {
    let fd = unsafe { inotify_init1(flags) };
    if fd < 0 {
        Err(io::Error::last_os_error())
    } else {
        Ok(fd)
    }
}

pub(crate) fn c_path(dir: &Path) -> io::Result<CString> {
    let bytes = dir.as_os_str().as_bytes();
    match CString::new(bytes) {
        Ok(path) => Ok(path),
        Err(_) => Err(io::Error::new(io::ErrorKind::InvalidInput, "state dir")),
    }
}

pub(crate) fn close_fd(fd: i32) {
    if fd >= 0 {
        unsafe {
            close(fd);
        }
    }
}

pub(crate) fn poll_pair(notify: i32, poke: i32) -> io::Result<(bool, bool)> {
    poll_for(notify, poke, -1)
}

pub(crate) fn poll_for(notify: i32, poke: i32, timeout: i32) -> io::Result<(bool, bool)> {
    if notify < 0 || poke < 0 {
        return Err(io::Error::other("watch"));
    }
    let mut fds = [
        PollFd {
            fd: notify,
            events: POLLIN,
            revents: 0,
        },
        PollFd {
            fd: poke,
            events: POLLIN,
            revents: 0,
        },
    ];
    let ready = unsafe { poll(fds.as_mut_ptr(), 2, timeout) };
    if ready <= 0 {
        return Err(io::Error::last_os_error());
    }
    let mark = POLLIN | POLLERR | POLLHUP;
    Ok(((fds[0].revents & mark) != 0, (fds[1].revents & mark) != 0))
}

fn read_names(fd: i32) -> io::Result<bool> {
    let mut buf = [0u8; 4096];
    let n = unsafe { read(fd, buf.as_mut_ptr(), buf.len()) };
    let n = bytes_read(n)?;
    Ok(names_refresh(&buf[..n]))
}

pub(crate) fn bytes_read(n: isize) -> io::Result<usize> {
    if n <= 0 {
        Err(io::Error::new(
            io::ErrorKind::UnexpectedEof,
            "inotify closed",
        ))
    } else {
        Ok(n as usize)
    }
}

pub(crate) fn names_refresh(buf: &[u8]) -> bool {
    let mut at = 0;
    while at + 16 <= buf.len() {
        let len =
            u32::from_ne_bytes([buf[at + 12], buf[at + 13], buf[at + 14], buf[at + 15]]) as usize;
        if len == 0 {
            at += 16;
            continue;
        }
        let name_at = at + 16;
        let name_end = name_at.saturating_add(len);
        if name_end > buf.len() {
            break;
        }
        if is_refresh(&buf[name_at..name_end]) {
            return true;
        }
        at = name_end;
    }
    false
}

fn is_refresh(name: &[u8]) -> bool {
    let end = match name.iter().position(|byte| *byte == 0) {
        Some(end) => end,
        None => name.len(),
    };
    &name[..end] == b"refresh"
}

fn drain_fd(fd: i32) {
    let mut buf = [0u8; 32];
    loop {
        let n = unsafe { read(fd, buf.as_mut_ptr(), buf.len()) };
        if n <= 0 {
            break;
        }
    }
}
