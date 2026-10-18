#[cfg(target_os = "macos")]
mod macos;
#[cfg(target_os = "windows")]
mod windows;
#[cfg(target_os = "macos")]
pub use macos::NativeWindowController;
#[cfg(target_os = "windows")]
pub use windows::NativeWindowController;

#[cfg(not(any(target_os = "windows", target_os = "macos")))]
pub struct NativeWindowController;
#[cfg(not(any(target_os = "windows", target_os = "macos")))]
impl NativeWindowController {
    pub fn enumerate() -> Result<Vec<crate::WindowInfo>, String> {
        Ok(Vec::new())
    }
    pub fn apply(_action: crate::WindowAction) -> Result<(), String> {
        Err("VeilKey currently supports Windows and macOS only".into())
    }
}
