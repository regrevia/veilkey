#[cfg(target_os = "macos")]
mod macos;
#[cfg(target_os = "windows")]
mod windows;
#[cfg(target_os = "macos")]
pub use macos::NativeWindowController;
#[cfg(target_os = "windows")]
pub use windows::NativeWindowController;
#[cfg(target_os = "windows")]
pub use windows::debug_log;
#[cfg(target_os = "windows")]
pub use windows::wake_main_window;

#[cfg(target_os = "windows")]
pub fn is_fullscreen_window(window_id: &str, windows: &[crate::WindowInfo]) -> bool {
    windows::NativeWindowController::is_fullscreen_window(window_id, windows)
}

#[cfg(not(target_os = "windows"))]
pub fn is_fullscreen_window(_window_id: &str, _windows: &[crate::WindowInfo]) -> bool {
    false
}

#[cfg(not(any(target_os = "windows", target_os = "macos")))]
pub struct NativeWindowController;
#[cfg(not(any(target_os = "windows", target_os = "macos")))]
impl NativeWindowController {
    pub fn foreground_window_id() -> Result<Option<String>, String> {
        Ok(None)
    }
    pub fn enumerate() -> Result<Vec<crate::WindowInfo>, String> {
        Ok(Vec::new())
    }
    pub fn apply_actions(actions: Vec<crate::WindowAction>) -> Result<(), String> {
        for action in actions {
            Self::apply(action)?;
        }
        Ok(())
    }
    pub fn apply(_action: crate::WindowAction) -> Result<(), String> {
        Err("VeilKey currently supports Windows and macOS only".into())
    }
}
