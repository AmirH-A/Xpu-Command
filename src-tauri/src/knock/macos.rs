//! macOS: the accelerometer of Apple Silicon MacBooks, an "AppleSPUHIDDriver" HID service
//! (vendor usage page 0xFF00, usage 3). Reports are 22 bytes; x, y, z are little-endian i32 at
//! offsets 6, 10, 14 in units of 1/65536 g. Undocumented by Apple: checked defensively.
use super::{Detector, Event, KnockConfig};
use crate::error::{msg, Result};
use core_foundation::base::{CFType, TCFType};
use core_foundation::dictionary::CFDictionary;
use core_foundation::number::CFNumber;
use core_foundation::runloop::{kCFRunLoopDefaultMode, CFRunLoop};
use core_foundation::string::CFString;
use std::ffi::{c_char, c_void};
use std::sync::atomic::Ordering;
use std::sync::mpsc::Sender;
use std::time::Instant;

type IoObject = u32;
type IoReturn = i32;
type CFTypeRef = *const c_void;
type ReportCallback = extern "C" fn(*mut c_void, IoReturn, *mut c_void, u32, u32, *mut u8, isize);

#[link(name = "IOKit", kind = "framework")]
extern "C" {
    fn IOServiceMatching(name: *const c_char) -> *mut c_void;
    fn IOServiceGetMatchingServices(port: u32, matching: *mut c_void, it: *mut IoObject) -> IoReturn;
    fn IOIteratorNext(it: IoObject) -> IoObject;
    fn IOObjectRelease(o: IoObject) -> IoReturn;
    fn IORegistryEntryCreateCFProperty(e: IoObject, key: CFTypeRef, alloc: CFTypeRef, opts: u32) -> CFTypeRef;
    fn IORegistryEntrySetCFProperty(e: IoObject, key: CFTypeRef, value: CFTypeRef) -> IoReturn;
    fn IOHIDManagerCreate(alloc: CFTypeRef, opts: u32) -> *mut c_void;
    fn IOHIDManagerSetDeviceMatching(m: *mut c_void, matching: CFTypeRef);
    fn IOHIDManagerOpen(m: *mut c_void, opts: u32) -> IoReturn;
    fn IOHIDManagerCopyDevices(m: *mut c_void) -> CFTypeRef;
    fn IOHIDManagerRegisterInputReportCallback(m: *mut c_void, cb: ReportCallback, ctx: *mut c_void);
    fn IOHIDManagerScheduleWithRunLoop(m: *mut c_void, rl: CFTypeRef, mode: CFTypeRef);
}

extern "C" {
    fn CFSetGetCount(set: CFTypeRef) -> isize;
    fn CFRelease(o: CFTypeRef);
}

const PAGE_VENDOR: i32 = 0xFF00;
const USAGE_ACCEL: i32 = 3;
const REPORT_LEN: isize = 22;
/// 2500 µs = 400 samples a second, enough to see a knock's sharp edge.
const INTERVAL_US: i32 = 2500;

/// Turns the accelerometer's reporting on. Returns false when there is no such sensor.
fn enable_sensor() -> bool {
    let mut found = false;
    unsafe {
        let mut it: IoObject = 0;
        if IOServiceGetMatchingServices(0, IOServiceMatching(c"AppleSPUHIDDriver".as_ptr()), &mut it) != 0 {
            return false;
        }
        loop {
            let s = IOIteratorNext(it);
            if s == 0 {
                break;
            }
            let key = CFString::from_static_string("Product");
            let p = IORegistryEntryCreateCFProperty(s, key.as_CFTypeRef(), std::ptr::null(), 0);
            if !p.is_null() {
                let v = CFType::wrap_under_create_rule(p as _);
                if v.downcast::<CFString>().map(|s| s.to_string()).as_deref() == Some("accel") {
                    found = true;
                    for (k, n) in [("SensorPropertyReportingState", 1), ("SensorPropertyPowerState", 1), ("ReportInterval", INTERVAL_US)] {
                        let r = IORegistryEntrySetCFProperty(s, CFString::new(k).as_CFTypeRef(), CFNumber::from(n).as_CFTypeRef());
                        if r != 0 {
                            log::debug!("knock: setting {k} returned {r:#x}");
                        }
                    }
                }
            }
            IOObjectRelease(s);
        }
        IOObjectRelease(it);
    }
    found
}

fn i32_at(r: &[u8], o: usize) -> f32 {
    i32::from_le_bytes([r[o], r[o + 1], r[o + 2], r[o + 3]]) as f32 / 65536.0
}

struct Ctx {
    cfg: &'static KnockConfig,
    tx: Sender<Event>,
    det: Detector,
    t0: Instant,
}

extern "C" fn on_report(ctx: *mut c_void, result: IoReturn, _sender: *mut c_void, _ty: u32, _id: u32, report: *mut u8, len: isize) {
    if ctx.is_null() || report.is_null() || result != 0 || len != REPORT_LEN {
        return;
    }
    let c = unsafe { &mut *(ctx as *mut Ctx) };
    let r = unsafe { std::slice::from_raw_parts(report, len as usize) };
    let a = [i32_at(r, 6), i32_at(r, 10), i32_at(r, 14)];
    if a.iter().any(|v| !v.is_finite() || v.abs() > 16.0) {
        return;
    }
    super::SAMPLES.fetch_add(1, Ordering::Relaxed);
    let ev = c.det.feed(c.t0.elapsed().as_secs_f64(), a, c.cfg.threshold());
    if let Some(ev) = ev {
        if matches!(ev, Event::DoubleKnock { .. }) && !super::input_quiet(&seconds_since_input()) {
            log::debug!("knock ignored: the keyboard or trackpad was just used");
            return;
        }
        if c.cfg.enabled.load(Ordering::Relaxed) {
            let _ = c.tx.send(ev);
        }
    }
}

extern "C" {
    fn CGEventSourceSecondsSinceLastEventType(state: i32, event_type: u32) -> f64;
}

/// Seconds since the last key press, modifier change, click and scroll (any app). Only timings are
/// read, never which key: typing and clicking shake the laptop like a knock.
fn seconds_since_input() -> [f64; 5] {
    const HID_STATE: i32 = 1;
    [10u32, 12, 1, 3, 22].map(|t| unsafe { CGEventSourceSecondsSinceLastEventType(HID_STATE, t) })
}

pub fn start(cfg: &'static KnockConfig, tx: Sender<Event>) -> Result<()> {
    if !enable_sensor() {
        return Err(msg("this Mac has no supported motion sensor (double knock needs a MacBook with Apple silicon)"));
    }
    let (ready_tx, ready_rx) = std::sync::mpsc::channel::<Result<()>>();
    std::thread::Builder::new()
        .name("xpu-knock".into())
        .spawn(move || unsafe {
            let m = IOHIDManagerCreate(std::ptr::null(), 0);
            if m.is_null() {
                let _ = ready_tx.send(Err(msg("motion sensor: could not create the HID manager")));
                return;
            }
            let matching = CFDictionary::from_CFType_pairs(&[
                (CFString::new("PrimaryUsagePage").as_CFType(), CFNumber::from(PAGE_VENDOR).as_CFType()),
                (CFString::new("PrimaryUsage").as_CFType(), CFNumber::from(USAGE_ACCEL).as_CFType()),
            ]);
            IOHIDManagerSetDeviceMatching(m, matching.as_CFTypeRef());
            let r = IOHIDManagerOpen(m, 0);
            let devices = IOHIDManagerCopyDevices(m);
            let count = if devices.is_null() { 0 } else { CFSetGetCount(devices) };
            if !devices.is_null() {
                CFRelease(devices);
            }
            if r != 0 || count == 0 {
                let _ = ready_tx.send(Err(msg(format!("motion sensor not readable (open {r:#x}, {count} devices)"))));
                return;
            }
            let ctx = Box::into_raw(Box::new(Ctx { cfg, tx, det: Detector::default(), t0: Instant::now() }));
            IOHIDManagerRegisterInputReportCallback(m, on_report, ctx as *mut c_void);
            IOHIDManagerScheduleWithRunLoop(m, CFRunLoop::get_current().as_CFTypeRef(), kCFRunLoopDefaultMode as CFTypeRef);
            let _ = ready_tx.send(Ok(()));
            CFRunLoop::run_current();
        })
        .map_err(|e| msg(format!("motion sensor thread: {e}")))?;
    ready_rx.recv().unwrap_or_else(|_| Err(msg("motion sensor thread exited")))
}
