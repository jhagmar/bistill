//! The notification area icon.
//!
//! The standard library cannot register one. The `unsafe` block is the small
//! wrapper around `Shell_NotifyIconW`.

#![allow(unsafe_code)]

use crate::Error;
use std::ffi::c_void;

const NIM_ADD: u32 = 0;
const NIM_MODIFY: u32 = 1;
const NIF_MESSAGE: u32 = 0x1;
const NIF_ICON: u32 = 0x2;
const NIF_TIP: u32 = 0x4;
const WM_LBUTTONUP: usize = 0x0202;

#[repr(C)]
struct NotifyIcon {
    size: u32,
    window: *mut c_void,
    id: u32,
    flags: u32,
    callback: u32,
    icon: *mut c_void,
    tip: [u16; 128],
}

unsafe extern "system" {
    fn Shell_NotifyIconW(message: u32, data: *const NotifyIcon) -> i32;
    fn LoadIconW(instance: *mut c_void, name: usize) -> *mut c_void;
    fn CreateWindowExW(
        extra: u32,
        class: *const u16,
        window: *const u16,
        style: u32,
        x: i32,
        y: i32,
        width: i32,
        height: i32,
        parent: *mut c_void,
        menu: *mut c_void,
        instance: *mut c_void,
        param: *mut c_void,
    ) -> *mut c_void;
    fn GetModuleHandleW(name: *const u16) -> *mut c_void;
}

pub(crate) fn set_unread(count: u64) -> Result<(), Error> {
    let tip = crate::tray_tip(count);
    unsafe { apply(count, &tip) }
}

unsafe fn apply(count: u64, tip: &str) -> Result<(), Error> {
    let _ = count;
    let window = CreateWindowExW(
        0,
        wide("STATIC").as_ptr(),
        wide("bistill").as_ptr(),
        0,
        0,
        0,
        0,
        0,
        std::ptr::null_mut(),
        std::ptr::null_mut(),
        GetModuleHandleW(std::ptr::null()),
        std::ptr::null_mut(),
    );
    if window.is_null() {
        return Err(Error::Failed {
            program: "tray".to_owned(),
            message: "no status icon watcher".to_owned(),
        });
    }
    let mut data = NotifyIcon {
        size: std::mem::size_of::<NotifyIcon>() as u32,
        window,
        id: 1,
        flags: NIF_MESSAGE | NIF_ICON | NIF_TIP,
        callback: WM_LBUTTONUP as u32,
        icon: LoadIconW(std::ptr::null_mut(), 32512),
        tip: [0; 128],
    };
    write_tip(&mut data.tip, tip);
    let added = Shell_NotifyIconW(NIM_ADD, &data);
    if added == 0 {
        let modified = Shell_NotifyIconW(NIM_MODIFY, &data);
        if modified == 0 {
            return Err(Error::Failed {
                program: "tray".to_owned(),
                message: "no status icon watcher".to_owned(),
            });
        }
    }
    Ok(())
}

fn write_tip(tip: &mut [u16; 128], text: &str) {
    for (index, unit) in text.encode_utf16().take(127).enumerate() {
        tip[index] = unit;
    }
}

fn wide(text: &str) -> Vec<u16> {
    text.encode_utf16().chain(std::iter::once(0)).collect()
}
