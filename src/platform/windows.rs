use crate::{Rect, WindowAction, WindowInfo};
use std::collections::BTreeMap;
use std::ffi::OsString;
use std::os::windows::ffi::OsStringExt;
use windows::Win32::Foundation::{BOOL, CloseHandle, HWND, LPARAM, RECT};
use windows::Win32::System::Threading::{
    OpenProcess, PROCESS_NAME_FORMAT, PROCESS_QUERY_LIMITED_INFORMATION, QueryFullProcessImageNameW,
};
use windows::Win32::UI::WindowsAndMessaging::{
    EnumWindows, GetWindowRect, GetWindowTextLengthW, GetWindowTextW, GetWindowThreadProcessId,
    IsWindowVisible, SW_HIDE, SW_RESTORE, SWP_NOACTIVATE, SWP_NOZORDER, SetWindowPos,
    ShowWindowAsync,
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
                let id = format!("{}\0{}\0{}", w.app.to_lowercase(), w.title, *ordinal);
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
        let (window_id, handle, visibility) = match &action {
            WindowAction::SetVisible {
                window_id,
                handle,
                visible,
            } => (window_id.as_str(), *handle, Some(*visible)),
            WindowAction::SetRect {
                window_id, handle, ..
            } => (window_id.as_str(), *handle, None),
        };
        let current = Self::enumerate()?;
        let Some(window) = current
            .iter()
            .find(|window| window.id == window_id && window.native_handle == handle)
        else {
            return Err(
                "The selected window no longer exists. Refresh the list and rebind it.".into(),
            );
        };
        let hwnd = HWND(handle as *mut core::ffi::c_void);
        match action {
            WindowAction::SetVisible { visible, .. } => unsafe {
                let _ = ShowWindowAsync(hwnd, if visible { SW_RESTORE } else { SW_HIDE });
                Ok(())
            },
            WindowAction::SetRect { rect, .. } => unsafe {
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
            },
        }
        .and_then(|_| {
            if visibility.is_some() && window.title.is_empty() {
                Err("Window identity validation failed".into())
            } else {
                Ok(())
            }
        })
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
