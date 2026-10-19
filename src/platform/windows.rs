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
    title: String,
    app: String,
    rect: Rect,
}
pub struct NativeWindowController;

impl NativeWindowController {
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
                let app = process_name(pid);
                raw.push(RawWindow {
                    hwnd,
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
                    "{}\0{}\0{}|hwnd:{:x}",
                    w.app.to_lowercase(),
                    w.title,
                    *ordinal,
                    w.hwnd.0 as usize
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
                (window.id == window_id || window.id.starts_with(&format!("{window_id}|hwnd:")))
                    && window.native_handle == handle
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
                        } else {
                            // Do not touch z-order for the default game-friendly path.
                            let _ = ShowWindowAsync(hwnd, SW_SHOWNOACTIVATE);
                            Ok(())
                        }
                    } else {
                        if force_popup {
                            let _ = SetWindowPos(
                                hwnd,
                                HWND_NOTOPMOST,
                                0,
                                0,
                                0,
                                0,
                                SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE | SWP_NOOWNERZORDER,
                            );
                        }
                        let _ = ShowWindowAsync(hwnd, SW_HIDE);
                        Ok(())
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
