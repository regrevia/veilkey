use crate::{Rect, WindowAction, WindowInfo};
use std::collections::{BTreeMap, HashMap};
use std::ffi::OsString;
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::os::windows::ffi::OsStringExt;
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};
use windows::Win32::Foundation::{BOOL, CloseHandle, HWND, LPARAM, RECT};
use windows::Win32::Graphics::Gdi::{
    GetMonitorInfoW, MONITOR_DEFAULTTONEAREST, MONITORINFO, MonitorFromRect, MonitorFromWindow,
    RDW_ALLCHILDREN, RDW_INVALIDATE, RedrawWindow,
};
use windows::Win32::System::Threading::{
    OpenProcess, PROCESS_NAME_FORMAT, PROCESS_QUERY_LIMITED_INFORMATION, QueryFullProcessImageNameW,
};
use windows::Win32::UI::WindowsAndMessaging::{
    EnumWindows, GWL_EXSTYLE, GetDesktopWindow, GetForegroundWindow, GetShellWindow,
    GetWindowLongPtrW, GetWindowPlacement, GetWindowRect, GetWindowTextLengthW, GetWindowTextW,
    GetWindowThreadProcessId, HWND_NOTOPMOST, HWND_TOP, HWND_TOPMOST, IsIconic, IsWindow,
    IsWindowVisible, SW_HIDE, SW_SHOWNOACTIVATE, SWP_NOACTIVATE, SWP_NOMOVE, SWP_NOOWNERZORDER,
    SWP_NOSIZE, SWP_NOZORDER, SWP_SHOWWINDOW, SetForegroundWindow, SetWindowPlacement,
    SetWindowPos, ShowWindowAsync, WINDOWPLACEMENT, WS_EX_TOOLWINDOW,
};

pub fn foreground_window_handle() -> isize {
    unsafe { GetForegroundWindow().0 as isize }
}

fn focused_window_id_for_handle(
    windows: &[WindowInfo],
    foreground_handle: isize,
) -> Option<String> {
    (foreground_handle != 0)
        .then(|| {
            windows
                .iter()
                .find(|window| window.native_handle == foreground_handle)
                .map(|window| window.id.clone())
        })
        .flatten()
}

fn is_desktop_or_shell_handle(handle: HWND) -> bool {
    unsafe { is_desktop_or_shell_handle_from(handle, GetDesktopWindow(), GetShellWindow()) }
}

fn is_desktop_or_shell_handle_from(handle: HWND, desktop: HWND, shell: HWND) -> bool {
    handle == desktop || handle == shell
}

const LOG_LIMIT: u64 = 2 * 1024 * 1024;

fn validate_main_window_enumeration(
    result: windows::core::Result<()>,
    target_found: bool,
    process_id: u32,
) -> Result<(), String> {
    if target_found {
        return Ok(());
    }
    match result {
        Err(error) => Err(format!("Could not enumerate VeilKey's own window: {error}")),
        Ok(()) => Err(format!(
            "No VeilKey main window was found for process {process_id}."
        )),
    }
}

pub fn debug_log(message: impl AsRef<str>) {
    let Some(local_app_data) = std::env::var_os("LOCALAPPDATA") else {
        return;
    };
    let directory = PathBuf::from(local_app_data).join("VeilKey").join("logs");
    if fs::create_dir_all(&directory).is_err() {
        return;
    }
    let path = directory.join("windows.log");
    if fs::metadata(&path).is_ok_and(|metadata| metadata.len() >= LOG_LIMIT) {
        let _ = fs::rename(&path, directory.join("windows.log.1"));
    }
    let Ok(mut file) = OpenOptions::new().create(true).append(true).open(path) else {
        return;
    };
    let timestamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis())
        .unwrap_or_default();
    let _ = writeln!(file, "[{timestamp}] {}", message.as_ref());
}

/// Makes the hidden VeilKey viewport visible without relying on egui to process
/// a queued viewport command (which is exactly what may stop while hidden).
pub fn wake_main_window() -> Result<isize, String> {
    use windows::Win32::System::Threading::GetCurrentProcessId;

    let current_pid = unsafe { GetCurrentProcessId() };
    let mut state = (current_pid, HWND::default());
    unsafe extern "system" fn find_main_window(hwnd: HWND, state: LPARAM) -> BOOL {
        unsafe {
            let (pid, target) = &mut *(state.0 as *mut (u32, HWND));
            let mut window_pid = 0u32;
            GetWindowThreadProcessId(hwnd, Some(&mut window_pid));
            if window_pid == *pid && window_title(hwnd).as_deref() == Some("VeilKey") {
                *target = hwnd;
                return BOOL(0);
            }
            BOOL(1)
        }
    }

    // EnumWindows reports ERROR_CANCELLED when a callback returns FALSE to
    // stop enumeration. We intentionally do that as soon as our window is
    // found, so only treat the API result as an error when no target was set.
    let enumeration_result = unsafe {
        EnumWindows(
            Some(find_main_window),
            LPARAM(&mut state as *mut _ as isize),
        )
    };
    let target = state.1;
    validate_main_window_enumeration(enumeration_result, !target.0.is_null(), current_pid)?;
    // The BOOL return value reports the previous visibility state; FALSE is
    // expected for the hidden-window case and does not mean the request failed.
    let was_visible = unsafe { ShowWindowAsync(target, SW_SHOWNOACTIVATE) }.as_bool();
    debug_log(format!(
        "native wake requested hwnd=0x{:x} previously_visible={was_visible}",
        target.0 as usize
    ));
    Ok(target.0 as isize)
}

unsafe fn refresh_window_surface(hwnd: HWND) {
    unsafe {
        // Invalidate without erasing or forcing synchronous cross-process
        // painting. GPU-backed windows can expose a black surface when their
        // background is erased before their renderer produces a new frame.
        let _ = RedrawWindow(hwnd, None, None, RDW_INVALIDATE | RDW_ALLCHILDREN);
    }
}

#[derive(Clone)]
struct RawWindow {
    hwnd: HWND,
    pid: u32,
    title: String,
    app: String,
    rect: Rect,
}
pub struct NativeWindowController;

impl NativeWindowController {
    pub fn is_fullscreen_window(window_id: &str, windows: &[WindowInfo]) -> bool {
        let Some(window) = windows
            .iter()
            .find(|window| crate::same_window_id(&window.id, window_id))
        else {
            return false;
        };
        let hwnd = HWND(window.native_handle as *mut core::ffi::c_void);
        unsafe {
            if !IsWindow(hwnd).as_bool() || !IsWindowVisible(hwnd).as_bool() {
                return false;
            }
            let monitor = MonitorFromWindow(hwnd, MONITOR_DEFAULTTONEAREST);
            let mut info = MONITORINFO {
                cbSize: std::mem::size_of::<MONITORINFO>() as u32,
                ..Default::default()
            };
            if !GetMonitorInfoW(monitor, &mut info).as_bool() {
                return false;
            }
            let mut rect = RECT::default();
            if GetWindowRect(hwnd, &mut rect).is_err() {
                return false;
            }
            let monitor = info.rcMonitor;
            (rect.left - monitor.left).abs() <= 2
                && (rect.top - monitor.top).abs() <= 2
                && (rect.right - monitor.right).abs() <= 2
                && (rect.bottom - monitor.bottom).abs() <= 2
        }
    }

    pub fn focused_window_snapshot(
        logically_hidden_window_ids: &[String],
    ) -> Result<(Option<String>, Vec<WindowInfo>), String> {
        Self::focused_window_snapshot_for_handle(
            foreground_window_handle(),
            logically_hidden_window_ids,
        )
    }

    pub fn focused_window_snapshot_for_handle(
        foreground_handle: isize,
        logically_hidden_window_ids: &[String],
    ) -> Result<(Option<String>, Vec<WindowInfo>), String> {
        let mut windows = Self::enumerate_fast()?;
        for window in &mut windows {
            if logically_hidden_window_ids
                .iter()
                .any(|id| crate::same_window_id(&window.id, id))
            {
                window.visible = false;
            }
        }
        let focused = if is_desktop_or_shell_handle(HWND(foreground_handle as *mut _)) {
            None
        } else {
            focused_window_id_for_handle(&windows, foreground_handle)
        };
        Ok((focused, windows))
    }

    pub fn enumerate_fast() -> Result<Vec<WindowInfo>, String> {
        Self::enumerate()
    }

    pub fn screen_capture_authorized() -> bool {
        true
    }

    pub fn request_screen_capture_access() -> bool {
        true
    }

    pub fn foreground_window_id() -> Result<Option<String>, String> {
        let foreground = unsafe { GetForegroundWindow() };
        if foreground.0.is_null() {
            return Ok(None);
        }
        if is_desktop_or_shell_handle(foreground) {
            return Ok(None);
        }
        Ok(Self::enumerate()?
            .into_iter()
            .find(|window| window.native_handle == foreground.0 as isize)
            .map(|window| window.id))
    }

    pub fn enumerate() -> Result<Vec<WindowInfo>, String> {
        let mut raw = Vec::<RawWindow>::new();
        unsafe extern "system" fn collect(hwnd: HWND, state: LPARAM) -> BOOL {
            unsafe {
                let raw = &mut *(state.0 as *mut Vec<RawWindow>);
                let length = GetWindowTextLengthW(hwnd);
                if length <= 0 {
                    return BOOL(1);
                }
                let mut title = vec![0u16; (length + 1) as usize];
                let copied = GetWindowTextW(hwnd, &mut title);
                if copied <= 0 {
                    return BOOL(1);
                }
                title.truncate(copied as usize);
                let title = String::from_utf16_lossy(&title).trim().to_owned();
                if title.is_empty() {
                    return BOOL(1);
                }
                let minimized = IsIconic(hwnd).as_bool();
                let mut rect = RECT::default();
                if minimized {
                    if let Some(normal_rect) = minimized_window_rect(hwnd) {
                        rect = normal_rect;
                    } else if GetWindowRect(hwnd, &mut rect).is_err() {
                        return BOOL(1);
                    }
                } else if GetWindowRect(hwnd, &mut rect).is_err() {
                    return BOOL(1);
                }
                let mut pid = 0u32;
                GetWindowThreadProcessId(hwnd, Some(&mut pid));
                let app = process_name(pid);
                raw.push(RawWindow {
                    hwnd,
                    pid,
                    title,
                    app,
                    rect: Rect {
                        x: rect.left,
                        y: rect.top,
                        width: rect.right - rect.left,
                        height: rect.bottom - rect.top,
                    },
                });
                BOOL(1)
            }
        }
        unsafe {
            EnumWindows(Some(collect), LPARAM(&mut raw as *mut _ as isize))
                .map_err(|e| e.to_string())?;
        }
        raw.sort_by(|a, b| {
            (&a.app, &a.title, a.rect.x, a.rect.y).cmp(&(&b.app, &b.title, b.rect.x, b.rect.y))
        });
        let mut counts = BTreeMap::<(String, String), usize>::new();
        let mut process_names = HashMap::<u32, String>::new();
        Ok(raw
            .into_iter()
            .map(|w| {
                let key = (w.app.to_lowercase(), w.title.clone());
                let ordinal = counts.entry(key).or_default();
                // Keep the legacy prefix so groups created by older VeilKey builds
                // remain readable, then add the HWND.  The old ordinal changed when
                // a window moved; HWND stays stable for the lifetime of that window.
                let id = format!(
                    "{}\0{}\0{}|hwnd:{:x}|pid:{}",
                    w.app.to_lowercase(),
                    w.title,
                    *ordinal,
                    w.hwnd.0 as usize,
                    w.pid
                );
                *ordinal += 1;
                let app = process_names
                    .entry(w.pid)
                    .or_insert_with(|| w.app.clone())
                    .clone();
                // IsWindowVisible remains true for minimized windows; those have
                // no on-screen surface and must not appear as visible previews.
                let visible =
                    unsafe { IsWindowVisible(w.hwnd).as_bool() && !IsIconic(w.hwnd).as_bool() };
                WindowInfo {
                    id,
                    title: w.title,
                    app,
                    rect: w.rect,
                    visible,
                    native_handle: w.hwnd.0 as isize,
                    process_id: w.pid,
                    preview: None,
                }
            })
            .collect())
    }

    pub fn apply(action: WindowAction) -> Result<(), String> {
        Self::apply_actions(vec![action])
    }

    pub fn apply_actions(actions: Vec<WindowAction>) -> Result<(), String> {
        if actions.is_empty() {
            return Ok(());
        }
        debug_log(format!("apply_actions count={}", actions.len()));
        let foreground_before = unsafe { GetForegroundWindow() };
        let hiding = actions
            .iter()
            .any(|action| matches!(action, WindowAction::SetVisible { visible: false, .. }));
        let hidden_handles: Vec<isize> = actions
            .iter()
            .filter_map(|action| match action {
                WindowAction::SetVisible {
                    handle,
                    visible: false,
                    ..
                } => Some(*handle),
                _ => None,
            })
            .collect();
        let mut errors = Vec::new();
        let mut pending_visibility = Vec::new();
        for action in actions {
            let (window_id, handle) = match &action {
                WindowAction::SetVisible {
                    window_id, handle, ..
                }
                | WindowAction::SetRect {
                    window_id, handle, ..
                } => (window_id.as_str(), *handle),
            };
            let requested_visibility = match &action {
                WindowAction::SetVisible { visible, .. } => Some(*visible),
                WindowAction::SetRect { .. } => None,
            };
            let hwnd = HWND(handle as *mut core::ffi::c_void);
            debug_log(format!("action window_id={window_id:?} hwnd=0x{handle:x}"));
            let Some(window_title) = validated_window_title(hwnd, window_id) else {
                debug_log(format!(
                    "action rejected hwnd=0x{handle:x}: identity mismatch"
                ));
                errors.push(format!(
                    "Window '{window_id}' disappeared or changed identity; refresh and rebind it."
                ));
                continue;
            };
            let result = match action {
                WindowAction::SetVisible {
                    visible,
                    force_popup,
                    restore_rect,
                    ..
                } => unsafe {
                    if visible {
                        if let Some(rect) = restore_rect {
                            let rect = rect_to_native(rect);
                            let flags = SWP_NOACTIVATE | SWP_SHOWWINDOW | SWP_NOOWNERZORDER;
                            let flags = if force_popup {
                                flags
                            } else {
                                flags | SWP_NOZORDER
                            };
                            SetWindowPos(
                                hwnd,
                                if force_popup { HWND_TOPMOST } else { HWND_TOP },
                                rect.left,
                                rect.top,
                                rect.right - rect.left,
                                rect.bottom - rect.top,
                                flags,
                            )
                            .map_err(|e| e.to_string())
                            .map(|()| {
                                refresh_window_surface(hwnd);
                            })
                        } else if force_popup {
                            SetWindowPos(
                                hwnd,
                                HWND_TOPMOST,
                                0,
                                0,
                                0,
                                0,
                                SWP_NOMOVE
                                    | SWP_NOSIZE
                                    | SWP_NOACTIVATE
                                    | SWP_SHOWWINDOW
                                    | SWP_NOOWNERZORDER,
                            )
                            .map_err(|e| e.to_string())
                            .map(|()| {
                                refresh_window_surface(hwnd);
                            })
                        } else {
                            // Do not touch z-order for the default game-friendly path.
                            // ShowWindowAsync returns the previous visibility state, not success.
                            let _ = ShowWindowAsync(hwnd, SW_SHOWNOACTIVATE);
                            refresh_window_surface(hwnd);
                            Ok(())
                        }
                    } else {
                        let z_order_result = if force_popup {
                            SetWindowPos(
                                hwnd,
                                HWND_NOTOPMOST,
                                0,
                                0,
                                0,
                                0,
                                SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE | SWP_NOOWNERZORDER,
                            )
                            .map_err(|e| e.to_string())
                        } else {
                            Ok(())
                        };
                        // ShowWindowAsync returns the previous visibility state, not success.
                        let _ = ShowWindowAsync(hwnd, SW_HIDE);
                        let hide_result = Ok(());
                        match (z_order_result, hide_result) {
                            (Ok(()), result) | (result, Ok(())) => result,
                            (Err(z_order_error), Err(hide_error)) => Err(format!(
                                "Could not restore normal z-order ({z_order_error}); hiding also failed ({hide_error})"
                            )),
                        }
                    }
                },
                WindowAction::SetRect { rect, .. } => {
                    let mut current_rect = RECT::default();
                    if unsafe { GetWindowRect(hwnd, &mut current_rect) }.is_ok()
                        && current_rect.left == rect.x
                        && current_rect.top == rect.y
                        && current_rect.right - current_rect.left == rect.width
                        && current_rect.bottom - current_rect.top == rect.height
                    {
                        Ok(())
                    } else {
                        unsafe {
                            if IsIconic(hwnd).as_bool() {
                                let mut placement = window_placement(hwnd)?;
                                let screen_rect = rect_to_native(rect);
                                placement.rcNormalPosition = screen_to_placement_rect(
                                    hwnd,
                                    screen_rect,
                                    MonitorFromRect(&screen_rect, MONITOR_DEFAULTTONEAREST),
                                );
                                SetWindowPlacement(hwnd, &placement).map_err(|e| e.to_string())
                            } else {
                                SetWindowPos(
                                    hwnd,
                                    None,
                                    rect.x,
                                    rect.y,
                                    rect.width,
                                    rect.height,
                                    SWP_NOZORDER | SWP_NOACTIVATE,
                                )
                                .map_err(|e| e.to_string())
                            }
                        }
                    }
                }
            };
            if let Err(error) = result {
                debug_log(format!(
                    "action failed title={window_title:?} hwnd=0x{handle:x}: {error}"
                ));
                errors.push(format!("{window_title}: {error}"));
            } else {
                if let Some(expected) = requested_visibility {
                    pending_visibility.push((hwnd, expected, window_title, handle));
                } else {
                    debug_log(format!(
                        "action completed title={window_title:?} hwnd=0x{handle:x}"
                    ));
                }
            }
        }
        // Dispatch every member of the group before waiting for individual
        // windows. This makes a group appear or disappear as one coherent
        // operation instead of visibly stepping through each window.
        for (hwnd, expected, window_title, handle) in pending_visibility {
            match wait_for_visibility(hwnd, expected) {
                Ok(()) => {
                    let observed = unsafe { IsWindowVisible(hwnd).as_bool() };
                    debug_log(format!(
                        "action completed title={window_title:?} hwnd=0x{handle:x} requested_visible={expected} observed_visible={observed}"
                    ));
                }
                Err(error) => {
                    debug_log(format!(
                        "action failed title={window_title:?} hwnd=0x{handle:x}: {error}"
                    ));
                    errors.push(format!("{window_title}: {error}"));
                }
            }
        }
        // When hiding windows behind a game, restore the already-focused foreground app only
        // if Windows transferred focus as a side effect. Never activate a VeilKey group window.
        if hiding
            && !foreground_before.0.is_null()
            && !hidden_handles.contains(&(foreground_before.0 as isize))
        {
            unsafe {
                if IsWindow(foreground_before).as_bool()
                    && GetForegroundWindow() != foreground_before
                {
                    let _ = SetForegroundWindow(foreground_before);
                }
            }
        }
        if errors.is_empty() {
            debug_log("apply_actions completed");
            Ok(())
        } else {
            debug_log(format!("apply_actions errors={}", errors.len()));
            Err(format!(
                "Some group window actions failed: {}",
                errors.join("; ")
            ))
        }
    }
}

fn expected_process_id(window_id: &str) -> Option<u32> {
    let (_, identity) = window_id.rsplit_once("|hwnd:")?;
    identity
        .split('|')
        .find_map(|part| part.strip_prefix("pid:"))?
        .parse()
        .ok()
}

fn validated_window_title(hwnd: HWND, window_id: &str) -> Option<String> {
    let expected_pid = expected_process_id(window_id)?;
    unsafe {
        if !IsWindow(hwnd).as_bool() {
            return None;
        }
        let mut actual_pid = 0u32;
        GetWindowThreadProcessId(hwnd, Some(&mut actual_pid));
        if actual_pid == 0 || actual_pid != expected_pid {
            return None;
        }
        let title = window_title(hwnd)?;
        (Some(title.as_str()) == window_id.split('\0').nth(1)).then_some(title)
    }
}

fn window_title(hwnd: HWND) -> Option<String> {
    unsafe {
        if !IsWindow(hwnd).as_bool() {
            return None;
        }
        let length = GetWindowTextLengthW(hwnd);
        if length <= 0 {
            return None;
        }
        let mut title = vec![0u16; length as usize + 1];
        let copied = GetWindowTextW(hwnd, &mut title);
        if copied <= 0 {
            return None;
        }
        title.truncate(copied as usize);
        Some(String::from_utf16_lossy(&title).trim().to_owned())
    }
}

fn wait_for_visibility(hwnd: HWND, expected: bool) -> Result<(), String> {
    let deadline = std::time::Instant::now() + std::time::Duration::from_millis(500);
    loop {
        if !unsafe { IsWindow(hwnd).as_bool() } {
            return Err("Window closed while its visibility change was pending.".into());
        }
        let visible = unsafe { IsWindowVisible(hwnd).as_bool() };
        if visible == expected {
            return Ok(());
        }
        if std::time::Instant::now() >= deadline {
            return Err(format!(
                "Window visibility did not become {expected} within 500 ms (still {visible})."
            ));
        }
        std::thread::sleep(std::time::Duration::from_millis(1));
    }
}

fn window_placement(hwnd: HWND) -> Result<WINDOWPLACEMENT, String> {
    let mut placement = WINDOWPLACEMENT {
        length: std::mem::size_of::<WINDOWPLACEMENT>() as u32,
        ..Default::default()
    };
    unsafe { GetWindowPlacement(hwnd, &mut placement) }
        .map_err(|error| format!("Could not read the window's normal placement: {error}"))?;
    Ok(placement)
}

fn minimized_window_rect(hwnd: HWND) -> Option<RECT> {
    let placement = window_placement(hwnd).ok()?;
    let monitor = unsafe { MonitorFromWindow(hwnd, MONITOR_DEFAULTTONEAREST) };
    Some(if uses_workspace_coordinates(hwnd) {
        workspace_to_screen_rect(placement.rcNormalPosition, monitor)
    } else {
        placement.rcNormalPosition
    })
}

fn uses_workspace_coordinates(hwnd: HWND) -> bool {
    let extended_style = unsafe { GetWindowLongPtrW(hwnd, GWL_EXSTYLE) } as u32;
    extended_style & WS_EX_TOOLWINDOW.0 == 0
}

fn screen_to_placement_rect(
    hwnd: HWND,
    rect: RECT,
    monitor: windows::Win32::Graphics::Gdi::HMONITOR,
) -> RECT {
    if uses_workspace_coordinates(hwnd) {
        screen_to_workspace_rect(rect, monitor)
    } else {
        rect
    }
}

fn monitor_work_area_offset(monitor: windows::Win32::Graphics::Gdi::HMONITOR) -> (i32, i32) {
    if monitor.0.is_null() {
        return (0, 0);
    }
    let mut info = MONITORINFO {
        cbSize: std::mem::size_of::<MONITORINFO>() as u32,
        ..Default::default()
    };
    if !unsafe { GetMonitorInfoW(monitor, &mut info) }.as_bool() {
        return (0, 0);
    }
    (
        info.rcWork.left - info.rcMonitor.left,
        info.rcWork.top - info.rcMonitor.top,
    )
}

fn workspace_to_screen_rect(rect: RECT, monitor: windows::Win32::Graphics::Gdi::HMONITOR) -> RECT {
    let (offset_x, offset_y) = monitor_work_area_offset(monitor);
    translate_rect(rect, offset_x, offset_y)
}

fn screen_to_workspace_rect(rect: RECT, monitor: windows::Win32::Graphics::Gdi::HMONITOR) -> RECT {
    let (offset_x, offset_y) = monitor_work_area_offset(monitor);
    translate_rect(rect, -offset_x, -offset_y)
}

fn translate_rect(mut rect: RECT, offset_x: i32, offset_y: i32) -> RECT {
    rect.left += offset_x;
    rect.right += offset_x;
    rect.top += offset_y;
    rect.bottom += offset_y;
    rect
}

fn rect_to_native(rect: Rect) -> RECT {
    RECT {
        left: rect.x,
        top: rect.y,
        right: rect.x + rect.width,
        bottom: rect.y + rect.height,
    }
}

fn process_name(pid: u32) -> String {
    let Ok(handle) = (unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid) }) else {
        return format!("pid:{pid}");
    };
    let mut buffer = vec![0u16; 1024];
    let mut size = buffer.len() as u32;
    let result = unsafe {
        QueryFullProcessImageNameW(
            handle,
            PROCESS_NAME_FORMAT(0),
            windows::core::PWSTR(buffer.as_mut_ptr()),
            &mut size,
        )
    };
    unsafe {
        let _ = CloseHandle(handle);
    }
    if result.is_err() {
        return format!("pid:{pid}");
    }
    let path = OsString::from_wide(&buffer[..size as usize]);
    std::path::Path::new(&path)
        .file_name()
        .unwrap_or_default()
        .to_string_lossy()
        .into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stopping_window_enumeration_after_finding_target_is_success() {
        // A callback returning FALSE can leave EnumWindows reporting a stale
        // last-error value. The found HWND is authoritative in that case.
        let stopped_early = Err(windows::core::Error::from_win32());
        assert!(validate_main_window_enumeration(stopped_early, true, 42).is_ok());
    }

    #[test]
    fn missing_main_window_preserves_enumeration_error() {
        let error = windows::core::Error::from_win32();
        let message = error.to_string();
        let result = validate_main_window_enumeration(Err(error), false, 42);
        assert_eq!(
            result.expect_err("missing window must fail"),
            format!("Could not enumerate VeilKey's own window: {message}")
        );
    }

    #[test]
    fn repeated_desktop_enumeration_returns_unique_window_identities() {
        for _ in 0..200 {
            let windows = NativeWindowController::enumerate().expect("enumerate desktop windows");
            let mut handles = std::collections::HashSet::new();
            let mut ids = std::collections::HashSet::new();
            for window in windows {
                assert!(handles.insert(window.native_handle));
                assert!(ids.insert(window.id));
            }
        }
    }

    #[test]
    fn windows_preview_controller_exposes_fast_snapshot_and_capture_access() {
        assert!(NativeWindowController::screen_capture_authorized());
        assert!(NativeWindowController::request_screen_capture_access());
        let (_, windows) = NativeWindowController::focused_window_snapshot(&[])
            .expect("read foreground and window snapshot");
        assert!(!windows.iter().any(|window| window.preview.is_some()));
    }

    #[test]
    fn workspace_screen_conversion_round_trips_negative_monitor_coordinates() {
        // A secondary monitor left and above the primary display, with taskbars
        // consuming space from the left and top edges of its work area.
        let screen_rect = RECT {
            left: -1800,
            top: -850,
            right: -900,
            bottom: -250,
        };
        let workspace_rect = translate_rect(screen_rect, -48, -40);
        let restored_screen_rect = translate_rect(workspace_rect, 48, 40);
        assert_eq!(restored_screen_rect, screen_rect);
    }

    #[test]
    fn parses_process_identity_from_current_and_legacy_window_ids() {
        assert_eq!(
            expected_process_id("app.exe\\0title\\00|hwnd:1a2b|pid:42"),
            Some(42)
        );
        assert_eq!(expected_process_id("app.exe\\0title\\00|hwnd:1a2b"), None);
        assert_eq!(expected_process_id("app.exe\\0title\\00"), None);
    }

    #[test]
    fn foreground_snapshot_uses_hotkey_time_handle_after_veilkey_wake() {
        let windows = vec![
            WindowInfo {
                id: "files\\0release|hwnd:10|pid:2".into(),
                title: "release".into(),
                app: "files.exe".into(),
                rect: Default::default(),
                visible: true,
                native_handle: 10,
                process_id: 2,
                preview: None,
            },
            WindowInfo {
                id: "veilkey\\0VeilKey|hwnd:20|pid:1".into(),
                title: "VeilKey".into(),
                app: "veilkey.exe".into(),
                rect: Default::default(),
                visible: true,
                native_handle: 20,
                process_id: 1,
                preview: None,
            },
        ];
        assert_eq!(
            focused_window_id_for_handle(&windows, 10),
            Some(windows[0].id.clone())
        );
        assert_eq!(
            focused_window_id_for_handle(&windows, 20),
            Some(windows[1].id.clone())
        );
        assert_eq!(focused_window_id_for_handle(&windows, 0), None);
    }

    #[test]
    fn desktop_and_shell_handles_are_never_dynamic_binding_targets() {
        let desktop = HWND(10 as *mut _);
        let shell = HWND(20 as *mut _);
        assert!(is_desktop_or_shell_handle_from(desktop, desktop, shell));
        assert!(is_desktop_or_shell_handle_from(shell, desktop, shell));
        assert!(!is_desktop_or_shell_handle_from(
            HWND(30 as *mut _),
            desktop,
            shell
        ));
    }
}
