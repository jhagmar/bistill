//! Block until `refresh` is created in the state directory.
//!
//! A directory change notification reports a new name. An event cancels the wait.

#![allow(unsafe_code)]

use std::io;
use std::os::windows::ffi::OsStrExt;
use std::path::{Path, PathBuf};

const FILE_NOTIFY_CHANGE_FILE_NAME: u32 = 0x1;

unsafe extern "system" {
    fn FindFirstChangeNotificationW(
        path: *const u16,
        watch_subtree: i32,
        filter: u32,
    ) -> *mut core::ffi::c_void;
    fn FindNextChangeNotification(handle: *mut core::ffi::c_void) -> i32;
    fn FindCloseChangeNotification(handle: *mut core::ffi::c_void) -> i32;
    fn WaitForMultipleObjects(
        count: u32,
        handles: *const *mut core::ffi::c_void,
        wait_all: i32,
        millis: u32,
    ) -> u32;
    fn CreateEventW(
        attrs: *mut core::ffi::c_void,
        manual: i32,
        initial: i32,
        name: *const u16,
    ) -> *mut core::ffi::c_void;
    fn SetEvent(handle: *mut core::ffi::c_void) -> i32;
    fn CloseHandle(handle: *mut core::ffi::c_void) -> i32;
}

/// A directory notification plus an event that [`RefreshWatch::cancel`] sets.
pub(crate) struct RefreshWatch {
    dir: PathBuf,
    notify: *mut core::ffi::c_void,
    cancel: *mut core::ffi::c_void,
}

unsafe impl Send for RefreshWatch {}
unsafe impl Sync for RefreshWatch {}

impl Drop for RefreshWatch {
    fn drop(&mut self) {
        unsafe {
            FindCloseChangeNotification(self.notify);
            CloseHandle(self.cancel);
        }
    }
}

impl RefreshWatch {
    /// `Ok(true)` when `refresh` exists. `Ok(false)` when [`Self::cancel`] ran.
    pub(crate) fn wait(&self) -> io::Result<bool> {
        loop {
            let handles = [self.notify, self.cancel];
            let ready = unsafe { WaitForMultipleObjects(2, handles.as_ptr(), 0, u32::MAX) };
            if ready == 1 {
                return Ok(false);
            }
            if ready != 0 {
                return Err(io::Error::last_os_error());
            }
            if self.dir.join("refresh").is_file() {
                return Ok(true);
            }
            if unsafe { FindNextChangeNotification(self.notify) } == 0 {
                return Err(io::Error::last_os_error());
            }
        }
    }

    /// Wakes [`Self::wait`].
    pub(crate) fn cancel(&self) {
        unsafe {
            SetEvent(self.cancel);
        }
    }
}

/// Watch `dir` for a file named `refresh`.
pub(crate) fn open(dir: &Path) -> io::Result<RefreshWatch> {
    let mut wide: Vec<u16> = dir.as_os_str().encode_wide().collect();
    wide.push(0);
    let notify =
        unsafe { FindFirstChangeNotificationW(wide.as_ptr(), 0, FILE_NOTIFY_CHANGE_FILE_NAME) };
    if notify as isize == -1 || notify.is_null() {
        return Err(io::Error::last_os_error());
    }
    let cancel = unsafe { CreateEventW(core::ptr::null_mut(), 0, 0, core::ptr::null()) };
    if cancel.is_null() {
        unsafe {
            FindCloseChangeNotification(notify);
        }
        return Err(io::Error::last_os_error());
    }
    Ok(RefreshWatch {
        dir: dir.to_owned(),
        notify,
        cancel,
    })
}
