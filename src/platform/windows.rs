use crate::{Rect, WindowAction, WindowInfo};
use std::collections::BTreeMap;
use std::ffi::OsString;
use std::os::windows::ffi::OsStringExt;
use windows::Win32::Foundation::{BOOL, CloseHandle, HWND, LPARAM, RECT};
use windows::Win32::System::Threading::{
    OpenProcess, PROCESS_NAME_FORMAT, PROCESS_QUERY_LIMITED_INFORMATION, QueryFullProcessImageNameW,
};
use windows::Win32::UI::WindowsAndMessaging::{
    EnumWindows, GetForegroundWindow, GetWindowRect, GetWindowTextLengthW, GetWindowTextW,
    GetWindowThreadProcessId, HWND_NOTOPMOST, HWND_TOPMOST, IsWindow, IsWindowVisible, SW_HIDE,
    SW_SHOWNOACTIVATE, SWP_NOACTIVATE, SWP_NOMOVE, SWP_NOOWNERZORDER, SWP_NOSIZE, SWP_NOZORDER,
    SWP_SHOWWINDOW, SetForegroundWindow, SetWindowPos, ShowWindowAsync,
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
                let mut rect = RECT::default();
                if GetWindowRect(hwnd, &mut rect).is_err() {
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
                let visible = unsafe { IsWindowVisible(w.hwnd).as_bool() };
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
