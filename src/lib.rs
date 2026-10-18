use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};

pub mod platform;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Rect {
    pub x: i32,
    pub y: i32,
    pub width: i32,
    pub height: i32,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WindowInfo {
    pub id: String,
    pub title: String,
    pub app: String,
    pub rect: Rect,
    pub visible: bool,
    pub native_handle: isize,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Group {
    pub name: String,
    pub window_ids: BTreeSet<String>,
    pub hotkey: Option<String>,
    pub is_visible: bool,
    #[serde(default)]
    pub force_popup: bool,
    #[serde(default)]
    pub restore_rects: BTreeMap<String, Rect>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct AppState {
    pub groups: Vec<Group>,
    #[serde(skip)]
    pub selected_windows: BTreeSet<String>,
    #[serde(skip)]
    pub new_group_name: String,
    #[serde(skip)]
    pub status: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum WindowAction {
    SetVisible {
        window_id: String,
        handle: isize,
        visible: bool,
        force_popup: bool,
    },
    SetRect {
        window_id: String,
        handle: isize,
        rect: Rect,
    },
}

impl AppState {
    pub fn create_group(&mut self, name: String) -> bool {
        let name = name.trim();
        if name.is_empty()
            || self.selected_windows.is_empty()
            || self
                .groups
                .iter()
                .any(|g| g.name.eq_ignore_ascii_case(name))
        {
            return false;
        }
        self.groups.push(Group {
            name: name.to_owned(),
            window_ids: std::mem::take(&mut self.selected_windows),
            hotkey: None,
            is_visible: true,
            force_popup: false,
            restore_rects: BTreeMap::new(),
        });
        self.new_group_name.clear();
        true
    }

    pub fn toggle_group(&mut self, index: usize, windows: &[WindowInfo]) -> Vec<WindowAction> {
        let Some(group) = self.groups.get_mut(index) else {
            return Vec::new();
        };
        let current: BTreeMap<&str, &WindowInfo> =
            windows.iter().map(|w| (w.id.as_str(), w)).collect();
        let mut actions = Vec::new();
        if !group
            .window_ids
            .iter()
            .any(|id| current.contains_key(id.as_str()))
        {
            return actions;
        }
        if group.is_visible {
            group.restore_rects.clear();
            for id in &group.window_ids {
                if let Some(window) = current.get(id.as_str()).filter(|w| w.visible) {
                    group.restore_rects.insert(id.clone(), window.rect);
                    actions.push(WindowAction::SetVisible {
                        window_id: id.clone(),
                        handle: window.native_handle,
                        visible: false,
                        force_popup: group.force_popup,
                    });
                }
            }
            group.is_visible = false;
        } else {
            for id in &group.window_ids {
                if let Some(window) = current.get(id.as_str()) {
                    if let Some(rect) = group.restore_rects.get(id) {
                        actions.push(WindowAction::SetRect {
                            window_id: id.clone(),
                            handle: window.native_handle,
                            rect: *rect,
                        });
                    }
                    actions.push(WindowAction::SetVisible {
                        window_id: id.clone(),
                        handle: window.native_handle,
                        visible: true,
                        force_popup: group.force_popup,
                    });
                }
            }
            group.is_visible = true;
        }
        actions
    }

    pub fn toggle_group_by_name(
        &mut self,
        name: &str,
        windows: &[WindowInfo],
    ) -> Vec<WindowAction> {
        let Some(index) = self.groups.iter().position(|group| group.name == name) else {
            return Vec::new();
        };
        self.toggle_group(index, windows)
    }
}

pub fn state_path() -> PathBuf {
    #[cfg(target_os = "windows")]
    let base = std::env::var_os("APPDATA")
        .map(PathBuf::from)
        .unwrap_or_else(std::env::temp_dir);
    #[cfg(not(target_os = "windows"))]
    let base = std::env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(std::env::temp_dir);
    #[cfg(target_os = "macos")]
    let dir = base
        .join("Library")
        .join("Application Support")
        .join("VeilKey");
    #[cfg(target_os = "windows")]
    let dir = base.join("VeilKey");
    #[cfg(not(any(target_os = "windows", target_os = "macos")))]
    let dir = base.join(".config").join("veilkey");
    dir.join("state.json")
}

pub fn load_state(path: &Path) -> AppState {
    fs::read(path)
        .ok()
        .and_then(|bytes| serde_json::from_slice(&bytes).ok())
        .unwrap_or_default()
}

pub fn save_state(path: &Path, state: &AppState) -> io::Result<()> {
    let parent = path
        .parent()
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "state file has no parent"))?;
    fs::create_dir_all(parent)?;
    let temp = path.with_extension("json.tmp");
    let bytes = serde_json::to_vec_pretty(state).map_err(io::Error::other)?;
    let mut file = fs::File::create(&temp)?;
    file.write_all(&bytes)?;
    file.sync_all()?;
    drop(file);
    replace_state_file(&temp, path)
}

#[cfg(not(target_os = "windows"))]
fn replace_state_file(temp: &Path, path: &Path) -> io::Result<()> {
    fs::rename(temp, path)
}

#[cfg(target_os = "windows")]
fn replace_state_file(temp: &Path, path: &Path) -> io::Result<()> {
    use std::os::windows::ffi::OsStrExt;
    use windows::Win32::Storage::FileSystem::{
        MOVEFILE_REPLACE_EXISTING, MOVEFILE_WRITE_THROUGH, MoveFileExW,
    };
    use windows::core::PCWSTR;
    let from: Vec<u16> = temp.as_os_str().encode_wide().chain(Some(0)).collect();
    let to: Vec<u16> = path.as_os_str().encode_wide().chain(Some(0)).collect();
    unsafe {
        MoveFileExW(
            PCWSTR(from.as_ptr()),
            PCWSTR(to.as_ptr()),
            MOVEFILE_REPLACE_EXISTING | MOVEFILE_WRITE_THROUGH,
        )
    }
    .map_err(io::Error::other)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_window(id: &str, handle: isize, x: i32) -> WindowInfo {
        WindowInfo {
            id: id.to_owned(),
            title: "MuMu Player".into(),
            app: "MuMu.exe".into(),
            rect: Rect {
                x,
                y: 20,
                width: 800,
                height: 600,
            },
            visible: true,
            native_handle: handle,
        }
    }

    #[test]
    fn group_toggle_restores_each_window_geometry() {
        let mut state = AppState::default();
        state.selected_windows.extend(["a".into(), "b".into()]);
        assert!(state.create_group("MuMu set".into()));
        let windows = [sample_window("a", 1, 10), sample_window("b", 2, 900)];
        assert_eq!(
            state.toggle_group(0, &windows),
            vec![
                WindowAction::SetVisible {
                    window_id: "a".into(),
                    handle: 1,
                    visible: false,
                    force_popup: false
                },
                WindowAction::SetVisible {
                    window_id: "b".into(),
                    handle: 2,
                    visible: false,
                    force_popup: false
                },
            ]
        );
        let hidden = [
            WindowInfo {
                visible: false,
                ..sample_window("a", 1, 300)
            },
            WindowInfo {
                visible: false,
                ..sample_window("b", 2, 1200)
            },
        ];
        assert_eq!(
            state.toggle_group(0, &hidden),
            vec![
                WindowAction::SetRect {
                    window_id: "a".into(),
                    handle: 1,
                    rect: Rect {
                        x: 10,
                        y: 20,
                        width: 800,
                        height: 600
                    }
                },
                WindowAction::SetVisible {
                    window_id: "a".into(),
                    handle: 1,
                    visible: true,
                    force_popup: false
                },
                WindowAction::SetRect {
                    window_id: "b".into(),
                    handle: 2,
                    rect: Rect {
                        x: 900,
                        y: 20,
                        width: 800,
                        height: 600
                    }
                },
                WindowAction::SetVisible {
                    window_id: "b".into(),
                    handle: 2,
                    visible: true,
                    force_popup: false
                },
            ]
        );
    }

    #[test]
    fn state_round_trip_preserves_groups_and_ignores_transient_ui_state() {
        let mut state = AppState::default();
        state.selected_windows.insert("a".into());
        state.create_group("A".into());
        state.new_group_name = "draft".into();
        state.status = "temporary".into();
        let path = std::env::temp_dir().join(format!("veilkey-state-{}.json", std::process::id()));
        save_state(&path, &state).unwrap();
        let loaded = load_state(&path);
        let _ = fs::remove_file(path);
        assert_eq!(loaded.groups, state.groups);
        assert!(loaded.selected_windows.is_empty());
        assert!(loaded.new_group_name.is_empty());
        assert!(loaded.status.is_empty());
    }
}
