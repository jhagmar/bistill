//! Windows console mode.
//!
//! `std` cannot enable virtual-terminal processing. The `unsafe` blocks below
//! are the wrapper around the console handle calls.

#![allow(unsafe_code)]

use std::io;
use std::sync::{Mutex, MutexGuard, PoisonError};
use std::time::Duration;

const STD_INPUT_HANDLE: u32 = -10i32 as u32;
const STD_OUTPUT_HANDLE: u32 = -11i32 as u32;
const ENABLE_VIRTUAL_TERMINAL_PROCESSING: u32 = 0x0004;
const ENABLE_VIRTUAL_TERMINAL_INPUT: u32 = 0x0200;
const INVALID_HANDLE: isize = -1;

#[repr(C)]
struct Coord {
    x: i16,
    y: i16,
}

#[repr(C)]
struct SmallRect {
    left: i16,
    top: i16,
    right: i16,
    bottom: i16,
}

#[repr(C)]
struct ConsoleInfo {
    size: Coord,
    cursor: Coord,
    attributes: u16,
    window: SmallRect,
    max: Coord,
}

const _: () = assert!(size_of::<ConsoleInfo>() == 22);

unsafe extern "system" {
    fn GetStdHandle(kind: u32) -> *mut core::ffi::c_void;
    fn GetConsoleMode(handle: *mut core::ffi::c_void, mode: *mut u32) -> i32;
    fn SetConsoleMode(handle: *mut core::ffi::c_void, mode: u32) -> i32;
    fn GetConsoleScreenBufferInfo(handle: *mut core::ffi::c_void, info: *mut ConsoleInfo) -> i32;
    fn WaitForSingleObject(handle: *mut core::ffi::c_void, millis: u32) -> u32;
    fn ReadFile(
        handle: *mut core::ffi::c_void,
        buf: *mut u8,
        len: u32,
        read: *mut u32,
        overlapped: *mut core::ffi::c_void,
    ) -> i32;
    fn WriteFile(
        handle: *mut core::ffi::c_void,
        buf: *const u8,
        len: u32,
        written: *mut u32,
        overlapped: *mut core::ffi::c_void,
    ) -> i32;
}

struct Armed {
    input: isize,
    output: isize,
    saved_in: u32,
    saved_out: u32,
}

static ARMED: Mutex<Option<Armed>> = Mutex::new(None);

/// A Windows console with virtual-terminal processing enabled.
pub struct Console {
    input: isize,
    output: isize,
}

impl Console {
    /// Claims the standard input and output console handles.
    pub fn stdio() -> io::Result<Self> {
        enable(
            std_handle(STD_INPUT_HANDLE)?,
            std_handle(STD_OUTPUT_HANDLE)?,
        )
    }

    /// Columns and rows of the console window.
    pub fn size(&self) -> (u16, u16) {
        screen_size(self.output).unwrap_or((0, 0))
    }

    /// Writes `bytes` to the console.
    pub fn write(&mut self, bytes: &[u8]) {
        let _ = write_handle(self.output, bytes);
    }

    /// Waits until the console has input or `timeout` elapses.
    pub fn wait(&self, timeout: Duration) -> bool {
        let millis = match u32::try_from(timeout.as_millis()) {
            Ok(millis) => millis,
            Err(_) => u32::MAX,
        };
        // SAFETY: `input` is the console handle returned by `GetStdHandle`.
        unsafe { WaitForSingleObject(as_handle(self.input), millis) == 0 }
    }

    /// Reads pending input. A failed read yields zero.
    pub fn read(&mut self, buf: &mut [u8]) -> usize {
        read_handle(self.input, buf)
    }
}

impl Drop for Console {
    fn drop(&mut self) {
        disarm();
    }
}

fn enable(input: isize, output: isize) -> io::Result<Console> {
    let saved_in = console_mode(input)?;
    let saved_out = console_mode(output)?;
    set_mode(input, ENABLE_VIRTUAL_TERMINAL_INPUT)?;
    set_mode(output, saved_out | ENABLE_VIRTUAL_TERMINAL_PROCESSING)?;
    *lock_armed() = Some(Armed {
        input,
        output,
        saved_in,
        saved_out,
    });
    Ok(Console { input, output })
}

/// Leaves the alternate screen and restores the saved console modes.
pub fn disarm() {
    let armed = lock_armed().take();
    if let Some(armed) = armed {
        let _ = write_handle(armed.output, crate::terminal::LEAVE);
        let _ = set_mode(armed.input, armed.saved_in);
        let _ = set_mode(armed.output, armed.saved_out);
    }
}

fn std_handle(kind: u32) -> io::Result<isize> {
    // SAFETY: `kind` is a standard-handle identifier.
    let handle = unsafe { GetStdHandle(kind) };
    if handle.is_null() || handle == (INVALID_HANDLE as *mut core::ffi::c_void) {
        Err(io::Error::last_os_error())
    } else {
        Ok(handle as isize)
    }
}

fn console_mode(handle: isize) -> io::Result<u32> {
    let mut mode = 0u32;
    // SAFETY: `handle` is an open console handle.
    let ok = unsafe { GetConsoleMode(as_handle(handle), &mut mode) };
    if ok == 0 {
        Err(io::Error::last_os_error())
    } else {
        Ok(mode)
    }
}

fn set_mode(handle: isize, mode: u32) -> io::Result<()> {
    // SAFETY: `handle` is an open console handle.
    let ok = unsafe { SetConsoleMode(as_handle(handle), mode) };
    if ok == 0 {
        Err(io::Error::last_os_error())
    } else {
        Ok(())
    }
}

fn screen_size(handle: isize) -> Option<(u16, u16)> {
    let mut info = ConsoleInfo {
        size: Coord { x: 0, y: 0 },
        cursor: Coord { x: 0, y: 0 },
        attributes: 0,
        window: SmallRect {
            left: 0,
            top: 0,
            right: 0,
            bottom: 0,
        },
        max: Coord { x: 0, y: 0 },
    };
    // SAFETY: `info` is a live `CONSOLE_SCREEN_BUFFER_INFO` and `handle` is an open console.
    let ok = unsafe { GetConsoleScreenBufferInfo(as_handle(handle), &mut info) };
    if ok == 0 {
        None
    } else {
        Some((
            span(info.window.left, info.window.right),
            span(info.window.top, info.window.bottom),
        ))
    }
}

fn span(start: i16, end: i16) -> u16 {
    let width = i32::from(end) - i32::from(start) + 1;
    if width > 0 && width <= i32::from(u16::MAX) {
        width as u16
    } else {
        0
    }
}

fn read_handle(handle: isize, buf: &mut [u8]) -> usize {
    let mut read = 0u32;
    let len = u32::try_from(buf.len()).unwrap_or(u32::MAX);
    // SAFETY: `buf` is a live buffer and `handle` is an open console handle.
    let ok = unsafe {
        ReadFile(
            as_handle(handle),
            buf.as_mut_ptr(),
            len,
            &mut read,
            core::ptr::null_mut(),
        )
    };
    if ok == 0 {
        0
    } else {
        usize::try_from(read).unwrap_or(0)
    }
}

fn write_handle(handle: isize, bytes: &[u8]) -> io::Result<()> {
    let mut written = 0u32;
    let len = u32::try_from(bytes.len()).unwrap_or(u32::MAX);
    // SAFETY: `bytes` is a live slice and `handle` is an open console handle.
    let ok = unsafe {
        WriteFile(
            as_handle(handle),
            bytes.as_ptr(),
            len,
            &mut written,
            core::ptr::null_mut(),
        )
    };
    if ok == 0 {
        Err(io::Error::last_os_error())
    } else {
        Ok(())
    }
}

fn as_handle(value: isize) -> *mut core::ffi::c_void {
    value as *mut core::ffi::c_void
}

fn lock_armed() -> MutexGuard<'static, Option<Armed>> {
    ARMED.lock().unwrap_or_else(PoisonError::into_inner)
}
