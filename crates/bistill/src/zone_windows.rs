//! The local time-zone offset on Windows.
//!
//! The standard library does not report that offset. `GetTimeZoneInformation`
//! is the small wrapper that does.

#![allow(unsafe_code)]

use std::mem::MaybeUninit;

const TIME_ZONE_ID_STANDARD: u32 = 1;
const TIME_ZONE_ID_DAYLIGHT: u32 = 2;
const TIME_ZONE_ID_INVALID: u32 = 0xFFFF_FFFF;

unsafe extern "system" {
    fn GetTimeZoneInformation(zone: *mut TimeZoneInformation) -> u32;
}

#[repr(C)]
struct SystemTime {
    year: u16,
    month: u16,
    day_of_week: u16,
    day: u16,
    hour: u16,
    minute: u16,
    second: u16,
    milliseconds: u16,
}

#[repr(C)]
struct TimeZoneInformation {
    bias: i32,
    standard_name: [u16; 32],
    standard_date: SystemTime,
    standard_bias: i32,
    daylight_name: [u16; 32],
    daylight_date: SystemTime,
    daylight_bias: i32,
}

pub(crate) fn local_offset_secs() -> i32 {
    let mut info = MaybeUninit::<TimeZoneInformation>::zeroed();
    let kind = unsafe { GetTimeZoneInformation(info.as_mut_ptr()) };
    if kind == TIME_ZONE_ID_INVALID {
        return 0;
    }
    let info = unsafe { info.assume_init() };
    let minutes = match kind {
        TIME_ZONE_ID_DAYLIGHT => info.bias.saturating_add(info.daylight_bias),
        TIME_ZONE_ID_STANDARD => info.bias.saturating_add(info.standard_bias),
        _ => info.bias,
    };
    minutes.saturating_mul(-60)
}
