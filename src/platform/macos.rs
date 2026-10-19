use crate::{Rect, WindowAction, WindowInfo};
use accessibility_sys::{
    AXIsProcessTrusted, AXUIElementCopyAttributeValue, AXUIElementCreateApplication,
    AXUIElementCreateSystemWide, AXUIElementRef, AXUIElementSetAttributeValue,
    AXUIElementSetMessagingTimeout, AXValueCreate, AXValueGetValue, kAXErrorSuccess,
    kAXFocusedWindowAttribute, kAXMinimizedAttribute, kAXPositionAttribute, kAXSizeAttribute,
    kAXTitleAttribute, kAXValueTypeCGPoint, kAXValueTypeCGSize, kAXWindowsAttribute,
};
use core_foundation_sys::array::{CFArrayGetCount, CFArrayGetValueAtIndex, CFArrayRef};
use core_foundation_sys::base::{
    CFEqual, CFHash, CFRelease, CFRetain, CFTypeRef, kCFAllocatorDefault,
};
use core_foundation_sys::number::{
    CFBooleanGetTypeID, CFBooleanGetValue, CFBooleanRef, kCFBooleanFalse, kCFBooleanTrue,
};
use core_foundation_sys::string::{
    CFStringCreateWithCString, CFStringGetCString, CFStringGetLength,
    CFStringGetMaximumSizeForEncoding, CFStringRef, kCFStringEncodingUTF8,
};
use std::collections::BTreeMap;
use std::ffi::{CString, c_char, c_void};
use std::process::Command;

#[repr(C)]
#[derive(Default)]
struct CGPoint {
    x: f64,
    y: f64,
}
#[repr(C)]
#[derive(Default)]
struct CGSize {
    width: f64,
    height: f64,
}
struct RawWindow {
    pid: i32,
    app: String,
    title: String,
    rect: Rect,
    visible: bool,
    element: AXUIElementRef,
}
pub struct NativeWindowController;

impl NativeWindowController {
    pub fn foreground_window_id() -> Result<Option<String>, String> {
        if !unsafe { AXIsProcessTrusted() } {
            return Err(
                "Enable VeilKey in Accessibility settings before binding a focused window.".into(),
            );
        }
        let system = unsafe { AXUIElementCreateSystemWide() };
        if system.is_null() {
            return Ok(None);
        }
        let focused = copy_value(system, kAXFocusedWindowAttribute);
        unsafe { CFRelease(system as CFTypeRef) };
        let Some(focused) = focused else {
            return Ok(None);
        };
        let raw = match collect_windows() {
            Ok(raw) => raw,
            Err(error) => {
                unsafe { CFRelease(focused) };
                return Err(error);
            }
        };
        let mut counts = BTreeMap::<(String, String), usize>::new();
        let mut found = None;
        for window in &raw {
            let key = (window.app.to_lowercase(), window.title.clone());
            let ordinal = counts.entry(key.clone()).or_default();
            if unsafe { CFEqual(focused, window.element as CFTypeRef) } != 0 {
                found = Some(window_identity(window, *ordinal));
            }
            *ordinal += 1;
        }
        unsafe {
            CFRelease(focused);
            for window in raw {
                CFRelease(window.element as CFTypeRef);
            }
        }
        Ok(found)
    }

    pub fn enumerate() -> Result<Vec<WindowInfo>, String> {
        let raw = collect_windows()?;
        let mut counts = BTreeMap::<(String, String), usize>::new();
        let windows = raw
            .iter()
            .map(|w| {
                let key = (w.app.to_lowercase(), w.title.clone());
                let ordinal = counts.entry(key).or_default();
                let id = window_identity(w, *ordinal);
                *ordinal += 1;
                WindowInfo {
                    native_handle: window_handle(w),
                    id,
                    title: w.title.clone(),
                    app: w.app.clone(),
                    rect: w.rect,
                    visible: w.visible,
                }
            })
            .collect();
        for window in raw {
            unsafe {
                CFRelease(window.element as CFTypeRef);
            }
        }
        Ok(windows)
    }

    pub fn apply(action: WindowAction) -> Result<(), String> {
        Self::apply_actions(vec![action])
    }

    pub fn apply_actions(actions: Vec<WindowAction>) -> Result<(), String> {
        if actions.is_empty() {
            return Ok(());
        }
        let raw = collect_windows()?;
        let indexed: Vec<_> = raw
            .iter()
            .map(|window| (window_handle(window), window.element))
            .collect();
        let mut errors = Vec::new();
        for action in actions {
            let handle = match &action {
                WindowAction::SetVisible { handle, .. } | WindowAction::SetRect { handle, .. } => {
                    *handle
                }
            };
            match indexed.iter().find(|(candidate, _)| *candidate == handle) {
                Some((_, element)) => {
                    if let Err(error) = apply_to_window(*element, action) {
                        errors.push(error);
                    }
                }
                None => errors.push(
                    "The selected macOS window no longer exists. Refresh the list and rebind it."
                        .into(),
                ),
            }
        }
        for window in raw {
            unsafe {
                CFRelease(window.element as CFTypeRef);
            }
        }
        if errors.is_empty() {
            Ok(())
        } else {
            Err(errors.join("; "))
        }
    }
}

fn collect_windows() -> Result<Vec<RawWindow>, String> {
    if !unsafe { AXIsProcessTrusted() } {
        return Err("Enable VeilKey in System Settings → Privacy & Security → Accessibility, then refresh the window list.".into());
    }
    let output = Command::new("ps")
        .args(["-axo", "pid=,comm="])
        .output()
        .map_err(|e| format!("Cannot list running apps: {e}"))?;
    if !output.status.success() {
        return Err("macOS process list is unavailable".into());
    }
    let mut raw = Vec::<RawWindow>::new();
    for line in String::from_utf8_lossy(&output.stdout).lines() {
        let Some((pid, path)) = line.trim().split_once(char::is_whitespace) else {
            continue;
        };
        let Ok(pid) = pid.trim().parse::<i32>() else {
            continue;
        };
        if pid == std::process::id() as i32 {
            continue;
        }
        let app = std::path::Path::new(path.trim())
            .file_name()
            .unwrap_or_default()
            .to_string_lossy()
            .into_owned();
        if app.is_empty() {
            continue;
        }
        let application = unsafe { AXUIElementCreateApplication(pid) };
        if application.is_null() {
            continue;
        }
        unsafe {
            AXUIElementSetMessagingTimeout(application, 0.25);
        }
        let Some(windows) = copy_value(application, kAXWindowsAttribute) else {
            unsafe {
                CFRelease(application as CFTypeRef);
            }
            continue;
        };
        let array = windows as CFArrayRef;
        for index in 0..unsafe { CFArrayGetCount(array) } {
            let element = unsafe { CFArrayGetValueAtIndex(array, index) as AXUIElementRef };
            if element.is_null() {
                continue;
            }
            unsafe {
                AXUIElementSetMessagingTimeout(element, 0.25);
            }
            let Some(title) = copy_string_attribute(element, kAXTitleAttribute) else {
                continue;
            };
            if title.trim().is_empty() {
                continue;
            }
            let Some(position_value) = copy_value(element, kAXPositionAttribute) else {
                continue;
            };
            let Some(size_value) = copy_value(element, kAXSizeAttribute) else {
                unsafe {
                    CFRelease(position_value);
                }
                continue;
            };
            let mut point = CGPoint::default();
            let mut size = CGSize::default();
            let has_point = unsafe {
                AXValueGetValue(
                    position_value as _,
                    kAXValueTypeCGPoint,
                    &mut point as *mut _ as *mut c_void,
                )
            };
            let has_size = unsafe {
                AXValueGetValue(
                    size_value as _,
                    kAXValueTypeCGSize,
                    &mut size as *mut _ as *mut c_void,
                )
            };
            let visible = !copy_bool_attribute(element, kAXMinimizedAttribute).unwrap_or(false);
            unsafe {
                CFRelease(position_value);
                CFRelease(size_value);
            }
            if has_point && has_size {
                unsafe {
                    CFRetain(element as CFTypeRef);
                }
                raw.push(RawWindow {
                    pid,
                    app: app.clone(),
                    title,
                    rect: Rect {
                        x: point.x.round() as i32,
                        y: point.y.round() as i32,
                        width: size.width.round() as i32,
                        height: size.height.round() as i32,
                    },
                    visible,
                    element,
                });
            }
        }
        unsafe {
            CFRelease(windows);
            CFRelease(application as CFTypeRef);
        }
    }
    raw.sort_by(|a, b| {
        (&a.app, &a.title, a.rect.x, a.rect.y, a.pid)
            .cmp(&(&b.app, &b.title, b.rect.x, b.rect.y, b.pid))
    });
    Ok(raw)
}

fn window_identity(window: &RawWindow, ordinal: usize) -> String {
    let accessibility_id = unsafe { CFHash(window.element as CFTypeRef) };
    format!(
        "{}\0{}\0{}\0pid:{}|ax:{accessibility_id:x}",
        window.app.to_lowercase(),
        window.title,
        ordinal,
        window.pid
    )
}

fn window_handle(window: &RawWindow) -> isize {
    let accessibility_id = unsafe { CFHash(window.element as CFTypeRef) };
    stable_handle(window.pid, &format!("ax:{accessibility_id:x}"))
}

fn apply_to_window(window: AXUIElementRef, action: WindowAction) -> Result<(), String> {
    match action {
        WindowAction::SetVisible { visible, .. } => {
            let attribute = cf_string(kAXMinimizedAttribute)?;
            let value = unsafe {
                if visible {
                    kCFBooleanFalse
                } else {
                    kCFBooleanTrue
                }
            };
            let result =
                unsafe { AXUIElementSetAttributeValue(window, attribute, value as CFTypeRef) };
            unsafe {
                CFRelease(attribute as CFTypeRef);
            }
            ax_result(result)
        }
        WindowAction::SetRect { rect, .. } => {
            let point = CGPoint {
                x: rect.x as f64,
                y: rect.y as f64,
            };
            let size = CGSize {
                width: rect.width as f64,
                height: rect.height as f64,
            };
            let point_value =
                unsafe { AXValueCreate(kAXValueTypeCGPoint, &point as *const _ as *const c_void) };
            let size_value =
                unsafe { AXValueCreate(kAXValueTypeCGSize, &size as *const _ as *const c_void) };
            if point_value.is_null() || size_value.is_null() {
                if !point_value.is_null() {
                    unsafe {
                        CFRelease(point_value as CFTypeRef);
                    }
                }
                if !size_value.is_null() {
                    unsafe {
                        CFRelease(size_value as CFTypeRef);
                    }
                }
                return Err("Could not create macOS geometry values".into());
            }
            let position = cf_string(kAXPositionAttribute)?;
            let position_result =
                unsafe { AXUIElementSetAttributeValue(window, position, point_value as CFTypeRef) };
            unsafe {
                CFRelease(position as CFTypeRef);
                CFRelease(point_value as CFTypeRef);
            }
            if position_result != kAXErrorSuccess {
                unsafe {
                    CFRelease(size_value as CFTypeRef);
                }
                return ax_result(position_result);
            }
            let size_attribute = cf_string(kAXSizeAttribute)?;
            let size_result = unsafe {
                AXUIElementSetAttributeValue(window, size_attribute, size_value as CFTypeRef)
            };
            unsafe {
                CFRelease(size_attribute as CFTypeRef);
                CFRelease(size_value as CFTypeRef);
            }
            ax_result(size_result)
        }
    }
}
fn copy_value(element: AXUIElementRef, name: &str) -> Option<CFTypeRef> {
    let attribute = cf_string(name).ok()?;
    let mut value: CFTypeRef = std::ptr::null();
    let result = unsafe { AXUIElementCopyAttributeValue(element, attribute, &mut value) };
    unsafe {
        CFRelease(attribute as CFTypeRef);
    }
    (result == kAXErrorSuccess && !value.is_null()).then_some(value)
}
fn copy_string_attribute(element: AXUIElementRef, name: &str) -> Option<String> {
    let value = copy_value(element, name)?;
    let string = cf_string_to_string(value as CFStringRef);
    unsafe {
        CFRelease(value);
    }
    string
}
fn copy_bool_attribute(element: AXUIElementRef, name: &str) -> Option<bool> {
    let value = copy_value(element, name)?;
    let result = if unsafe { core_foundation_sys::base::CFGetTypeID(value) }
        == unsafe { CFBooleanGetTypeID() }
    {
        Some(unsafe { CFBooleanGetValue(value as CFBooleanRef) })
    } else {
        None
    };
    unsafe {
        CFRelease(value);
    }
    result
}
fn cf_string(value: &str) -> Result<CFStringRef, String> {
    let value = CString::new(value).map_err(|e| e.to_string())?;
    let string = unsafe {
        CFStringCreateWithCString(kCFAllocatorDefault, value.as_ptr(), kCFStringEncodingUTF8)
    };
    if string.is_null() {
        Err("Could not allocate Accessibility attribute name".into())
    } else {
        Ok(string)
    }
}
fn cf_string_to_string(value: CFStringRef) -> Option<String> {
    if value.is_null() {
        return None;
    }
    let max = unsafe {
        CFStringGetMaximumSizeForEncoding(CFStringGetLength(value), kCFStringEncodingUTF8)
    } + 1;
    if max <= 1 {
        return Some(String::new());
    }
    let mut buffer = vec![0 as c_char; max as usize];
    if unsafe { CFStringGetCString(value, buffer.as_mut_ptr(), max, kCFStringEncodingUTF8) } == 0 {
        return None;
    }
    Some(
        unsafe { std::ffi::CStr::from_ptr(buffer.as_ptr()) }
            .to_string_lossy()
            .into_owned(),
    )
}
fn stable_handle(pid: i32, id: &str) -> isize {
    id.bytes()
        .chain(pid.to_le_bytes())
        .fold(0xcbf29ce484222325u64, |h, b| {
            (h ^ u64::from(b)).wrapping_mul(0x100000001b3)
        }) as isize
}
fn ax_result(error: i32) -> Result<(), String> {
    if error == kAXErrorSuccess {
        Ok(())
    } else {
        Err(format!("macOS Accessibility API error {error}"))
    }
}
