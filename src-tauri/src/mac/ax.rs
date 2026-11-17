//! Thin, safe wrappers over the macOS Accessibility API (AXUIElement): the same interface screen
//! readers use to read and operate other apps. Requires the Accessibility permission (already
//! needed for the talk button). Every call has a messaging timeout, so a hung app can't hang us.
use core_foundation::array::{CFArray, CFArrayGetCount, CFArrayGetTypeID, CFArrayGetValueAtIndex};
use core_foundation::base::{CFGetTypeID, CFType, CFTypeID, CFTypeRef, TCFType};
use core_foundation::boolean::{CFBoolean, CFBooleanGetTypeID};
use core_foundation::number::{CFNumber, CFNumberGetTypeID};
use core_foundation::string::{CFString, CFStringGetTypeID};
use std::ffi::c_void;

#[link(name = "ApplicationServices", kind = "framework")]
extern "C" {
    fn AXUIElementCreateApplication(pid: i32) -> CFTypeRef;
    fn AXUIElementCreateSystemWide() -> CFTypeRef;
    fn AXUIElementCopyAttributeValue(el: CFTypeRef, attr: CFTypeRef, out: *mut CFTypeRef) -> i32;
    fn AXUIElementCopyMultipleAttributeValues(el: CFTypeRef, attrs: CFTypeRef, options: u32, out: *mut CFTypeRef) -> i32;
    fn AXUIElementSetAttributeValue(el: CFTypeRef, attr: CFTypeRef, v: CFTypeRef) -> i32;
    fn AXUIElementPerformAction(el: CFTypeRef, action: CFTypeRef) -> i32;
    fn AXUIElementCopyActionNames(el: CFTypeRef, out: *mut CFTypeRef) -> i32;
    fn AXUIElementGetPid(el: CFTypeRef, pid: *mut i32) -> i32;
    fn AXUIElementCopyElementAtPosition(app: CFTypeRef, x: f32, y: f32, out: *mut CFTypeRef) -> i32;
    fn AXUIElementSetMessagingTimeout(el: CFTypeRef, seconds: f32) -> i32;
    fn AXUIElementGetTypeID() -> CFTypeID;
    fn AXValueGetTypeID() -> CFTypeID;
    fn AXValueGetType(v: CFTypeRef) -> u32;
    fn AXValueGetValue(v: CFTypeRef, ty: u32, out: *mut c_void) -> bool;
    fn AXValueCreate(ty: u32, value: *const c_void) -> CFTypeRef;
}

const AX_POINT: u32 = 1;
const AX_SIZE: u32 = 2;
const AX_RANGE: u32 = 4;

/// A text range, in UTF-16 code units (CFRange).
#[repr(C)]
#[derive(Debug, Clone, Copy)]
struct CfRange {
    location: isize,
    length: isize,
}

/// An accessibility element (a window, a button, a text field…) of some app. Reference counted by
/// Core Foundation; the AX API may be called from any thread.
#[derive(Clone)]
pub struct El(CFType);
unsafe impl Send for El {}
unsafe impl Sync for El {}

/// A decoded attribute value.
pub enum V {
    Str(String),
    Bool(bool),
    Num(f64),
    Els(Vec<El>),
    El(El),
    Point(f64, f64),
    Size(f64, f64),
    /// A text range: start and length in UTF-16 code units.
    Range(usize, usize),
    None,
}

fn cfstr(s: &str) -> CFString {
    CFString::new(s)
}

/// Sets a global timeout for every AX message (seconds). Call once at startup of a session.
pub fn set_global_timeout(seconds: f32) {
    let sys = system_wide();
    unsafe { AXUIElementSetMessagingTimeout(sys.raw(), seconds) };
}

pub fn system_wide() -> El {
    El(unsafe { CFType::wrap_under_create_rule(AXUIElementCreateSystemWide()) })
}

/// The process that has keyboard focus right now (live, unlike cached app lists).
pub fn focused_pid() -> Option<i32> {
    match system_wide().attr("AXFocusedApplication") {
        V::El(e) => e.pid(),
        _ => None,
    }
}

/// The element with keyboard focus, in whatever app is in front.
pub fn focused_element() -> Option<El> {
    system_wide().element("AXFocusedUIElement")
}

/// The element at a screen point (in points, top-left origin), whatever app owns it.
pub fn element_at(x: f64, y: f64) -> Option<El> {
    let sys = system_wide();
    let mut out: CFTypeRef = std::ptr::null();
    let r = unsafe { AXUIElementCopyElementAtPosition(sys.raw(), x as f32, y as f32, &mut out) };
    (r == 0 && !out.is_null()).then(|| El(unsafe { CFType::wrap_under_create_rule(out) }))
}

fn decode(v: CFType) -> V {
    let ty = unsafe { CFGetTypeID(v.as_CFTypeRef()) };
    unsafe {
        if ty == CFStringGetTypeID() {
            V::Str(CFString::wrap_under_get_rule(v.as_CFTypeRef() as _).to_string())
        } else if ty == CFBooleanGetTypeID() {
            V::Bool(bool::from(CFBoolean::wrap_under_get_rule(v.as_CFTypeRef() as _)))
        } else if ty == CFNumberGetTypeID() {
            CFNumber::wrap_under_get_rule(v.as_CFTypeRef() as _).to_f64().map(V::Num).unwrap_or(V::None)
        } else if ty == AXUIElementGetTypeID() {
            V::El(El(v))
        } else if ty == CFArrayGetTypeID() {
            let arr = v.as_CFTypeRef() as core_foundation::array::CFArrayRef;
            let n = CFArrayGetCount(arr).clamp(0, 2000);
            let mut out = Vec::with_capacity(n as usize);
            for i in 0..n {
                let item = CFArrayGetValueAtIndex(arr, i);
                if !item.is_null() && CFGetTypeID(item) == AXUIElementGetTypeID() {
                    out.push(El(CFType::wrap_under_get_rule(item)));
                }
            }
            V::Els(out)
        } else if ty == AXValueGetTypeID() {
            let raw = v.as_CFTypeRef();
            match AXValueGetType(raw) {
                AX_POINT => {
                    let mut p = core_graphics::geometry::CGPoint::new(0.0, 0.0);
                    if AXValueGetValue(raw, AX_POINT, &mut p as *mut _ as *mut c_void) { V::Point(p.x, p.y) } else { V::None }
                }
                AX_SIZE => {
                    let mut s = core_graphics::geometry::CGSize::new(0.0, 0.0);
                    if AXValueGetValue(raw, AX_SIZE, &mut s as *mut _ as *mut c_void) { V::Size(s.width, s.height) } else { V::None }
                }
                AX_RANGE => {
                    let mut r = CfRange { location: 0, length: 0 };
                    if AXValueGetValue(raw, AX_RANGE, &mut r as *mut _ as *mut c_void) && r.location >= 0 && r.length >= 0 {
                        V::Range(r.location as usize, r.length as usize)
                    } else {
                        V::None
                    }
                }
                _ => V::None,
            }
        } else {
            V::None
        }
    }
}

impl El {
    pub fn app(pid: i32) -> El {
        El(unsafe { CFType::wrap_under_create_rule(AXUIElementCreateApplication(pid)) })
    }

    fn raw(&self) -> CFTypeRef {
        self.0.as_CFTypeRef()
    }

    pub fn attr(&self, name: &str) -> V {
        let mut out: CFTypeRef = std::ptr::null();
        let r = unsafe { AXUIElementCopyAttributeValue(self.raw(), cfstr(name).as_CFTypeRef(), &mut out) };
        if r != 0 || out.is_null() {
            return V::None;
        }
        decode(unsafe { CFType::wrap_under_create_rule(out) })
    }

    /// Several attributes in one round trip (much faster for big trees). Missing ones are `V::None`.
    pub fn attrs(&self, names: &[&str]) -> Vec<V> {
        let keys: Vec<CFString> = names.iter().map(|n| cfstr(n)).collect();
        let arr = CFArray::from_CFTypes(&keys);
        let mut out: CFTypeRef = std::ptr::null();
        let r = unsafe { AXUIElementCopyMultipleAttributeValues(self.raw(), arr.as_CFTypeRef(), 0, &mut out) };
        if r != 0 || out.is_null() {
            return names.iter().map(|_| V::None).collect();
        }
        let values = unsafe { CFType::wrap_under_create_rule(out) };
        let raw = values.as_CFTypeRef() as core_foundation::array::CFArrayRef;
        let n = unsafe { CFArrayGetCount(raw) };
        (0..names.len() as isize)
            .map(|i| {
                if i >= n {
                    return V::None;
                }
                let item = unsafe { CFArrayGetValueAtIndex(raw, i) };
                if item.is_null() { V::None } else { decode(unsafe { CFType::wrap_under_get_rule(item) }) }
            })
            .collect()
    }

    pub fn string(&self, name: &str) -> Option<String> {
        match self.attr(name) {
            V::Str(s) => Some(s),
            V::Num(n) => Some(format!("{n}")),
            _ => None,
        }
    }

    pub fn element(&self, name: &str) -> Option<El> {
        match self.attr(name) {
            V::El(e) => Some(e),
            _ => None,
        }
    }

    pub fn elements(&self, name: &str) -> Vec<El> {
        match self.attr(name) {
            V::Els(v) => v,
            _ => Vec::new(),
        }
    }

    pub fn frame(&self) -> Option<(f64, f64, f64, f64)> {
        match self.attrs(&["AXPosition", "AXSize"]).as_slice() {
            [V::Point(x, y), V::Size(w, h)] => Some((*x, *y, *w, *h)),
            _ => None,
        }
    }

    pub fn pid(&self) -> Option<i32> {
        let mut pid = 0;
        (unsafe { AXUIElementGetPid(self.raw(), &mut pid) } == 0).then_some(pid)
    }

    pub fn actions(&self) -> Vec<String> {
        let mut out: CFTypeRef = std::ptr::null();
        if unsafe { AXUIElementCopyActionNames(self.raw(), &mut out) } != 0 || out.is_null() {
            return Vec::new();
        }
        let arr = unsafe { CFType::wrap_under_create_rule(out) };
        let raw = arr.as_CFTypeRef() as core_foundation::array::CFArrayRef;
        let n = unsafe { CFArrayGetCount(raw) }.clamp(0, 50);
        (0..n)
            .filter_map(|i| {
                let item = unsafe { CFArrayGetValueAtIndex(raw, i) };
                match decode(unsafe { CFType::wrap_under_get_rule(item) }) {
                    V::Str(s) => Some(s),
                    _ => None,
                }
            })
            .collect()
    }

    pub fn perform(&self, action: &str) -> Result<(), String> {
        let r = unsafe { AXUIElementPerformAction(self.raw(), cfstr(action).as_CFTypeRef()) };
        if r == 0 { Ok(()) } else { Err(ax_error(r)) }
    }

    pub fn range(&self, name: &str) -> Option<(usize, usize)> {
        match self.attr(name) {
            V::Range(start, len) => Some((start, len)),
            _ => None,
        }
    }

    pub fn set_range(&self, name: &str, start: usize, len: usize) -> Result<(), String> {
        let r = CfRange { location: start as isize, length: len as isize };
        let v = unsafe { AXValueCreate(AX_RANGE, &r as *const _ as *const c_void) };
        if v.is_null() {
            return Err("could not make a text range".into());
        }
        let v = unsafe { CFType::wrap_under_create_rule(v) };
        let r = unsafe { AXUIElementSetAttributeValue(self.raw(), cfstr(name).as_CFTypeRef(), v.as_CFTypeRef()) };
        if r == 0 { Ok(()) } else { Err(ax_error(r)) }
    }

    pub fn set_string(&self, name: &str, v: &str) -> Result<(), String> {
        let r = unsafe { AXUIElementSetAttributeValue(self.raw(), cfstr(name).as_CFTypeRef(), cfstr(v).as_CFTypeRef()) };
        if r == 0 { Ok(()) } else { Err(ax_error(r)) }
    }

    pub fn set_bool(&self, name: &str, v: bool) -> Result<(), String> {
        let b = if v { CFBoolean::true_value() } else { CFBoolean::false_value() };
        let r = unsafe { AXUIElementSetAttributeValue(self.raw(), cfstr(name).as_CFTypeRef(), b.as_CFTypeRef()) };
        if r == 0 { Ok(()) } else { Err(ax_error(r)) }
    }
}

fn ax_error(code: i32) -> String {
    match code {
        -25211 => "the app refused (Accessibility permission is off for Xpu Command)".into(),
        -25202 | -25201 => "that control no longer exists; read the window again".into(),
        -25204 => "the app didn't answer in time".into(),
        -25205 | -25206 => "that control can't do this".into(),
        c => format!("accessibility error {c}"),
    }
}
