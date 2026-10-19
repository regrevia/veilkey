use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

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
    #[serde(skip)]
    pub hotkey_draft: String,
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
            hotkey_draft: String::new(),
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
        let find_window = |id: &str| windows.iter().find(|window| same_window_id(&window.id, id));
        let mut actions = Vec::new();
        if !group.window_ids.iter().any(|id| find_window(id).is_some()) {
            return actions;
        }
        let any_visible = group
            .window_ids
            .iter()
            .filter_map(|id| find_window(id))
            .any(|window| window.visible);
        if any_visible {
            group.restore_rects.clear();
            for id in &group.window_ids {
                if let Some(window) = find_window(id).filter(|w| w.visible) {
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
                if let Some(window) = find_window(id) {
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

fn same_window_id(current: &str, saved: &str) -> bool {
    if current == saved {
        return true;
    }
    // Windows bindings carry a stable HWND suffix. The app/title/ordinal prefix
    // may change when a same-titled window moves and is re-sorted.
    if let Some((_, saved_handle)) = saved.rsplit_once("|hwnd:") {
        return current
            .rsplit_once("|hwnd:")
            .is_some_and(|(_, current_handle)| current_handle == saved_handle);
    }
    // Keep 0.2.0/0.2.1 IDs readable while users migrate their groups.
    current.starts_with(&format!("{saved}\0")) || current.starts_with(&format!("{saved}|hwnd:"))
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
    static NEXT_TEMP_ID: AtomicU64 = AtomicU64::new(0);
    let parent = path
        .parent()
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "state file has no parent"))?;
    fs::create_dir_all(parent)?;
    let lock_path = path.with_extension("json.lock");
    let lock_file = fs::OpenOptions::new()
        .create(true)
        .read(true)
        .write(true)
        .truncate(false)
        .open(&lock_path)?;
    fs2::FileExt::lock_exclusive(&lock_file)?;
    let mut temp_name = path.as_os_str().to_os_string();
    temp_name.push(format!(
        ".{}.{}.tmp",
        std::process::id(),
        NEXT_TEMP_ID.fetch_add(1, Ordering::Relaxed)
    ));
    let temp = PathBuf::from(temp_name);
    let bytes = serde_json::to_vec_pretty(state).map_err(io::Error::other)?;
    let result = (|| {
        let mut file = fs::File::create(&temp)?;
        file.write_all(&bytes)?;
        file.sync_all()?;
        drop(file);
        replace_state_file(&temp, path)
    })();
    if result.is_err() {
        let _ = fs::remove_file(&temp);
    }
    let unlock_result = fs2::FileExt::unlock(&lock_file);
    result?;
    unlock_result?;
    Ok(())
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
        let _ = fs::remove_file(&path);
        let _ = fs::remove_file(path.with_extension("json.lock"));
        assert_eq!(loaded.groups, state.groups);
        assert!(loaded.selected_windows.is_empty());
        assert!(loaded.new_group_name.is_empty());
        assert!(loaded.status.is_empty());
    }

    #[test]
    fn concurrent_state_saves_never_share_temporary_files_or_corrupt_json() {
        let path =
            std::env::temp_dir().join(format!("veilkey-concurrent-{}.json", std::process::id()));
        let writers: Vec<_> = (0..8)
            .map(|index| {
                let path = path.clone();
                std::thread::spawn(move || {
                    let mut state = AppState::default();
                    state.selected_windows.insert(format!("window-{index}"));
                    assert!(state.create_group(format!("Group {index}")));
                    save_state(&path, &state).unwrap();
                })
            })
            .collect();
        for writer in writers {
            writer.join().unwrap();
        }
        let loaded = load_state(&path);
        assert_eq!(loaded.groups.len(), 1);
        assert!(loaded.groups[0].name.starts_with("Group "));
        let _ = fs::remove_file(path);
        let _ = fs::remove_file(std::env::temp_dir().join(format!(
            "veilkey-concurrent-{}.json.lock",
            std::process::id()
        )));
    }

    #[test]
    fn group_toggle_recovers_when_saved_visibility_is_stale() {
        let mut state = AppState::default();
        state.selected_windows.extend(["a".into(), "b".into()]);
        assert!(state.create_group("MuMu set".into()));
        // Saved state says shown, but the OS says one group window is already hidden.
        let windows = [
            sample_window("a", 1, 10),
            WindowInfo {
                visible: false,
                ..sample_window("b", 2, 900)
            },
        ];
        let actions = state.toggle_group(0, &windows);
        assert_eq!(actions.len(), 1);
        assert!(matches!(
            actions[0],
            WindowAction::SetVisible { visible: false, .. }
        ));
        assert!(!state.groups[0].is_visible);
    }

    #[test]
    fn group_toggle_matches_windows_by_stable_suffix_after_resort() {
        let mut state = AppState::default();
        state
            .selected_windows
            .insert(format!("mumu\0Player\0{}|hwnd:2a", 0));
        assert!(state.create_group("MuMu set".into()));
        let moved = [WindowInfo {
            id: format!("mumu\0Player\0{}|hwnd:2a", 1),
            ..sample_window("ignored", 0x2a, 500)
        }];
        let actions = state.toggle_group(0, &moved);
        assert_eq!(actions.len(), 1);
        assert!(matches!(
            actions[0],
            WindowAction::SetVisible { visible: false, .. }
        ));
    }
}
