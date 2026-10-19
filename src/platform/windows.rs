use crate::{Rect, WindowAction, WindowInfo};
use std::collections::BTreeMap;
use std::ffi::OsString;
use std::os::windows::ffi::OsStringExt;
use windows::Win32::Foundation::{BOOL, CloseHandle, HWND, LPARAM, RECT};
use windows::Win32::Graphics::Gdi::{
    GetMonitorInfoW, MONITOR_DEFAULTTONEAREST, MONITORINFO, MonitorFromRect, MonitorFromWindow,
};
use windows::Win32::System::Threading::{
    OpenProcess, PROCESS_NAME_FORMAT, PROCESS_QUERY_LIMITED_INFORMATION, QueryFullProcessImageNameW,
};
use windows::Win32::UI::WindowsAndMessaging::{
    EnumWindows, GWL_EXSTYLE, GetForegroundWindow, GetWindowLongPtrW, GetWindowPlacement,
    GetWindowRect, GetWindowTextLengthW, GetWindowTextW, GetWindowThreadProcessId, HWND_NOTOPMOST,
    HWND_TOPMOST, IsIconic, IsWindow, IsWindowVisible, SW_HIDE, SW_SHOWNOACTIVATE, SWP_NOACTIVATE,
    SWP_NOMOVE, SWP_NOOWNERZORDER, SWP_NOSIZE, SWP_NOZORDER, SWP_SHOWWINDOW, SetForegroundWindow,
    SetWindowPlacement, SetWindowPos, ShowWindowAsync, WINDOWPLACEMENT, WS_EX_TOOLWINDOW,
};

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
    pub fn foreground_window_id() -> Result<Option<String>, String> {
        let foreground = unsafe { GetForegroundWindow() };
        if foreground.0.is_null() {
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
                if pid == std::process::id() {
                    return BOOL(1);
                }
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
                // IsWindowVisible remains true for minimized windows; those have
                // no on-screen surface and must not appear as visible previews.
                let visible =
                    unsafe { IsWindowVisible(w.hwnd).as_bool() && !IsIconic(w.hwnd).as_bool() };
                WindowInfo {
                    id,
                    title: w.title,
                    app: w.app,
                    rect: w.rect,
                    visible,
                    native_handle: w.hwnd.0 as isize,
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
        let current = Self::enumerate()?;
        let mut errors = Vec::new();
        for action in actions {
            let (window_id, handle) = match &action {
                WindowAction::SetVisible {
                    window_id, handle, ..
                }
                | WindowAction::SetRect {
                    window_id, handle, ..
                } => (window_id.as_str(), *handle),
            };
            let Some(window) = current.iter().find(|window| {
                same_window_id(&window.id, window_id) && window.native_handle == handle
            }) else {
                errors.push(format!(
                    "Window '{window_id}' disappeared; refresh and rebind it."
                ));
                continue;
            };
            let hwnd = HWND(handle as *mut core::ffi::c_void);
            let result = match action {
                WindowAction::SetVisible {
                    visible,
                    force_popup,
                    ..
                } => unsafe {
                    if visible {
                        if force_popup {
                            // Place above normal windows without activating or changing display mode.
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
                            .and_then(|()| wait_for_visibility(hwnd, true))
                        } else {
                            // Do not touch z-order for the default game-friendly path.
                            // ShowWindowAsync returns the previous visibility state, not success.
                            let _ = ShowWindowAsync(hwnd, SW_SHOWNOACTIVATE);
                            wait_for_visibility(hwnd, true)
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
                        let hide_result = wait_for_visibility(hwnd, false);
                        match (z_order_result, hide_result) {
                            (Ok(()), result) | (result, Ok(())) => result,
                            (Err(z_order_error), Err(hide_error)) => Err(format!(
                                "Could not restore normal z-order ({z_order_error}); hiding also failed ({hide_error})"
                            )),
                        }
                    }
                },
                WindowAction::SetRect { rect, .. } => {
                    if window.rect == rect {
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
                errors.push(format!("{}: {error}", window.title));
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
            Ok(())
        } else {
            Err(format!(
                "Some group window actions failed: {}",
                errors.join("; ")
            ))
        }
    }
}

fn wait_for_visibility(hwnd: HWND, visible: bool) -> Result<(), String> {
    for _ in 0..30 {
        unsafe {
            if !IsWindow(hwnd).as_bool() {
                return Err("Window closed before its visibility change completed.".into());
            }
            if IsWindowVisible(hwnd).as_bool() == visible {
                return Ok(());
            }
        }
        std::thread::sleep(std::time::Duration::from_millis(25));
    }
    Err(if visible {
        "Timed out waiting for the window to become visible.".into()
    } else {
        "Timed out waiting for the window to hide.".into()
    })
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

fn same_window_id(current: &str, saved: &str) -> bool {
    if current == saved {
        return true;
    }
    if let Some((_, saved_identity)) = saved.rsplit_once("|hwnd:") {
        let Some((_, current_identity)) = current.rsplit_once("|hwnd:") else {
            return false;
        };
        let saved_parts: Vec<_> = saved_identity.split('|').collect();
        let current_parts: Vec<_> = current_identity.split('|').collect();
        if saved_parts[0] != current_parts[0] {
            return false;
        }
        let saved_pid = saved_parts
            .iter()
            .find_map(|part| part.strip_prefix("pid:"));
        let current_pid = current_parts
            .iter()
            .find_map(|part| part.strip_prefix("pid:"));
        return match (saved_pid, current_pid) {
            (Some(saved_pid), Some(current_pid)) => saved_pid == current_pid,
            // Older bindings only stored the HWND. Keep them usable until users
            // rebind, while all newly created IDs protect against handle reuse.
            (Some(_), None) => false,
            (None, _) => true,
        };
    }
    current.starts_with(&format!("{saved}|hwnd:"))
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
    fn repeated_desktop_enumeration_returns_unique_window_identities() {
        for _ in 0..50 {
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
}
