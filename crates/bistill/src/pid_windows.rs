//! Whether a pid is running on Windows.
//!
//! `std` cannot query a process. The `unsafe` block is the wrapper around
//! `OpenProcess`.

#![allow(unsafe_code)]

use std::ffi::c_void;

const PROCESS_QUERY_LIMITED_INFORMATION: u32 = 0x1000;
const ERROR_ACCESS_DENIED: u32 = 5;

unsafe extern "system" {
    fn OpenProcess(access: u32, inherit: i32, pid: u32) -> *mut c_void;
    fn CloseHandle(handle: *mut c_void) -> i32;
    fn GetLastError() -> u32;
}

pub(crate) fn pid_alive(pid: u32) -> bool {
    if pid == 0 {
        false
    } else {
        unsafe { process_exists(pid) }
    }
}

unsafe fn process_exists(pid: u32) -> bool {
    let handle = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid);
    if handle.is_null() {
        GetLastError() == ERROR_ACCESS_DENIED
    } else {
        let _ = CloseHandle(handle);
        true
    }
}
