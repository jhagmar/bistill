//! The local time-zone offset on Unix.
//!
//! The standard library does not report that offset. `localtime_r` is the
//! small wrapper that does.

#![allow(unsafe_code)]

use std::mem::MaybeUninit;

unsafe extern "C" {
    fn time(tloc: *mut i64) -> i64;
    fn localtime_r(timep: *const i64, result: *mut Tm) -> *mut Tm;
}

#[repr(C)]
struct Tm {
    tm_sec: i32,
    tm_min: i32,
    tm_hour: i32,
    tm_mday: i32,
    tm_mon: i32,
    tm_year: i32,
    tm_wday: i32,
    tm_yday: i32,
    tm_isdst: i32,
    tm_gmtoff: i64,
    tm_zone: *const i8,
}

pub(crate) fn local_offset_secs() -> i32 {
    let mut now = 0i64;
    unsafe {
        time(&mut now);
        let mut tm = MaybeUninit::<Tm>::zeroed();
        localtime_r(&now, tm.as_mut_ptr());
        tm.assume_init().tm_gmtoff as i32
    }
}
