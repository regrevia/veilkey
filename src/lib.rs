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
    pub process_id: u32,
    pub preview: Option<WindowPreview>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WindowPreview {
    pub width: usize,
    pub height: usize,
    pub rgba: Vec<u8>,
}

pub fn bgra_to_rgba(mut pixels: Vec<u8>) -> Vec<u8> {
    for pixel in pixels.as_chunks_mut::<4>().0 {
        pixel.swap(0, 2);
    }
    pixels
}

impl WindowPreview {
    pub fn new(width: usize, height: usize, rgba: Vec<u8>) -> Option<Self> {
        let expected_len = width.checked_mul(height)?.checked_mul(4)?;
        (width > 0 && height > 0 && rgba.len() == expected_len).then_some(Self {
            width,
            height,
            rgba,
        })
    }
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
    pub macos_hide_application: bool,
    #[serde(default)]
    pub restore_rects: BTreeMap<String, Rect>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct AppState {
    pub groups: Vec<Group>,
    #[serde(default, skip)]
    pub dynamic_groups: BTreeMap<String, Group>,
    #[serde(default = "default_dynamic_toggle_modifiers")]
    pub dynamic_toggle_modifiers: Vec<String>,
    #[serde(default = "default_dynamic_suffixes")]
    pub dynamic_suffixes: Vec<String>,
    #[serde(default)]
    pub english_ui: bool,
    #[serde(default)]
    pub bind_mode: BindMode,
    #[serde(skip)]
    pub selected_windows: BTreeSet<String>,
    #[serde(skip)]
    pub new_group_name: String,
    #[serde(skip)]
    pub status: String,
}

impl Default for AppState {
    fn default() -> Self {
        Self {
            groups: Vec::new(),
            dynamic_groups: BTreeMap::new(),
            dynamic_toggle_modifiers: default_dynamic_toggle_modifiers(),
            dynamic_suffixes: default_dynamic_suffixes(),
            english_ui: false,
            bind_mode: BindMode::default(),
            selected_windows: Default::default(),
            new_group_name: String::new(),
            status: String::new(),
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BindMode {
    #[default]
    SelectFromList,
    OneWindowAtATime,
    ActiveApplication,
}

fn default_dynamic_toggle_modifiers() -> Vec<String> {
    #[cfg(target_os = "windows")]
    return vec!["ctrl".into(), "alt".into()];
    #[cfg(not(target_os = "windows"))]
    vec!["super".into()]
}

fn default_dynamic_suffixes() -> Vec<String> {
    ["Digit9", "Digit0", "Minus", "Equal"]
        .into_iter()
        .map(str::to_owned)
        .collect()
}

pub fn dynamic_hotkey_combinations(modifiers: &[String], suffixes: &[String]) -> Vec<String> {
    let mut modifiers: Vec<_> = modifiers
        .iter()
        .map(|modifier| modifier.to_ascii_lowercase())
        .collect();
    modifiers.sort();
    modifiers.dedup();
    let suffixes: Vec<_> = suffixes
        .iter()
        .map(|suffix| suffix.trim().to_owned())
        .filter(|suffix| !suffix.is_empty())
        .collect();
    let mut suffixes = suffixes;
    let mut seen = BTreeSet::new();
    suffixes.retain(|suffix| seen.insert(suffix.clone()));
    let modifier_sets: Vec<Vec<&str>> = if modifiers.len() < 2 {
        vec![modifiers.iter().map(String::as_str).collect()]
    } else {
        let mut pairs = Vec::new();
        for first in 0..modifiers.len() {
            for second in (first + 1)..modifiers.len() {
                pairs.push(vec![modifiers[first].as_str(), modifiers[second].as_str()]);
            }
        }
        pairs
    };
    suffixes
        .into_iter()
        .flat_map(|suffix| {
            modifier_sets.iter().map(move |combination| {
                if combination.is_empty() {
                    suffix.clone()
                } else {
                    format!("{}+{suffix}", combination.join("+"))
                }
            })
        })
        .collect()
}

impl BindMode {
    pub fn is_dynamic(self) -> bool {
        matches!(self, Self::OneWindowAtATime | Self::ActiveApplication)
    }
}

pub fn window_is_displayable(window: &WindowInfo) -> bool {
    window.visible && window.rect.width > 1 && window.rect.height > 1
}

pub fn dynamic_target_ids(
    mode: BindMode,
    focused_window_id: &str,
    windows: &[WindowInfo],
) -> Option<Vec<String>> {
    let focused = windows
        .iter()
        .find(|window| window.id == focused_window_id && window_is_displayable(window))?;
    match mode {
        BindMode::OneWindowAtATime => Some(vec![focused.id.clone()]),
        BindMode::ActiveApplication => Some(
            windows
                .iter()
                .filter(|window| {
                    window.process_id == focused.process_id && window_is_displayable(window)
                })
                .map(|window| window.id.clone())
                .collect(),
        ),
        BindMode::SelectFromList => None,
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DynamicTransition {
    pub show_ids: Vec<String>,
    pub hide_ids: Vec<String>,
}

pub fn dynamic_transition(
    mode: BindMode,
    focused_window_id: &str,
    windows: &[WindowInfo],
    hidden_ids: &[String],
) -> Option<DynamicTransition> {
    let mut next_ids = dynamic_target_ids(mode, focused_window_id, windows)?;
    next_ids.sort();
    let mut current_hidden = hidden_ids.to_vec();
    current_hidden.sort();
    if next_ids == current_hidden {
        return Some(DynamicTransition {
            show_ids: current_hidden,
            hide_ids: Vec::new(),
        });
    }
    Some(DynamicTransition {
        show_ids: current_hidden,
        hide_ids: next_ids,
    })
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum WindowAction {
    SetVisible {
        window_id: String,
        handle: isize,
        visible: bool,
        force_popup: bool,
        hide_application: bool,
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
            macos_hide_application: false,
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
        let any_hidden = group
            .window_ids
            .iter()
            .filter_map(|id| find_window(id))
            .any(|window| !window.visible);
        // Native window operations can partially succeed. In a mixed state,
        // honor the last requested target so the next shortcut converges the
        // whole group instead of merely repeating the action on visible windows.
        let should_hide = if any_visible && any_hidden {
            group.is_visible
        } else {
            any_visible
        };
        if should_hide {
            for id in &group.window_ids {
                if let Some(window) = find_window(id).filter(|w| w.visible) {
                    group.restore_rects.insert(id.clone(), window.rect);
                    actions.push(WindowAction::SetVisible {
                        window_id: id.clone(),
                        handle: window.native_handle,
                        visible: false,
                        force_popup: group.force_popup,
                        hide_application: group.macos_hide_application,
                    });
                }
            }
            group.is_visible = false;
        } else {
            for id in &group.window_ids {
                if let Some(window) = find_window(id).filter(|window| !window.visible) {
                    if !group.macos_hide_application
                        && let Some(rect) = group.restore_rects.get(id)
                    {
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
                        hide_application: group.macos_hide_application,
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

pub fn same_window_id(current: &str, saved: &str) -> bool {
    if current == saved {
        return true;
    }
    // macOS Accessibility element hashes remain stable when same-titled windows
    // move and their legacy display-order ordinal changes.
    if let Some((saved_prefix, saved_ax)) = saved.rsplit_once("|ax:")
        && let Some((current_prefix, current_ax)) = current.rsplit_once("|ax:")
        && saved_ax == current_ax
    {
        let saved_pid = saved_prefix.rsplit_once("pid:").map(|(_, pid)| pid);
        let current_pid = current_prefix.rsplit_once("pid:").map(|(_, pid)| pid);
        return saved_pid.is_some() && saved_pid == current_pid;
    }
    // Windows bindings carry a stable HWND suffix. The app/title/ordinal prefix
    // may change when a same-titled window moves and is re-sorted.
    if let Some((_, saved_handle)) = saved.rsplit_once("|hwnd:") {
        let Some((_, current_handle)) = current.rsplit_once("|hwnd:") else {
            return false;
        };
        let saved_parts: Vec<_> = saved_handle.split('|').collect();
        let current_parts: Vec<_> = current_handle.split('|').collect();
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
            (Some(_), None) => false,
            (None, _) => true,
        };
    }
    // Keep 0.2.0/0.2.1 IDs readable while users migrate their groups.
    current.starts_with(&format!("{saved}\0"))
        || current.starts_with(&format!("{saved}|hwnd:"))
        || current.starts_with(&format!("{saved}|ax:"))
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

pub fn acquire_instance_lock(path: &Path) -> io::Result<fs::File> {
    let parent = path.parent().ok_or_else(|| {
        io::Error::new(io::ErrorKind::InvalidInput, "instance lock has no parent")
    })?;
    fs::create_dir_all(parent)?;
    let lock_file = fs::OpenOptions::new()
        .create(true)
        .read(true)
        .write(true)
        .truncate(false)
        .open(path)?;
    fs2::FileExt::try_lock_exclusive(&lock_file)?;
    Ok(lock_file)
}

pub fn load_state(path: &Path) -> AppState {
    let Ok(bytes) = fs::read(path) else {
        return AppState::default();
    };
    match serde_json::from_slice(&bytes) {
        Ok(state) => state,
        Err(_) => {
            // Preserve a damaged file for diagnosis instead of silently destroying
            // the user's groups when the next save replaces it.
            let backup = path.with_extension(format!("json.corrupt-{}", std::process::id()));
            let _ = fs::copy(path, backup);
            AppState::default()
        }
    }
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
            process_id: 1,
            preview: None,
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
                    force_popup: false,
                    hide_application: false
                },
                WindowAction::SetVisible {
                    window_id: "b".into(),
                    handle: 2,
                    visible: false,
                    force_popup: false,
                    hide_application: false
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
                    force_popup: false,
                    hide_application: false
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
                    force_popup: false,
                    hide_application: false
                },
            ]
        );
    }

    #[test]
    fn partial_hide_retry_preserves_geometry_of_windows_already_hidden() {
        let mut state = AppState::default();
        state.selected_windows.extend(["a".into(), "b".into()]);
        assert!(state.create_group("MuMu set".into()));

        // The saved state says shown, but a is already hidden and b has moved.
        let partially_hidden = [
            WindowInfo {
                visible: false,
                ..sample_window("a", 1, 400)
            },
            sample_window("b", 2, 1200),
        ];
        state.toggle_group(0, &partially_hidden);

        let all_hidden = [
            WindowInfo {
                visible: false,
                ..sample_window("a", 1, 400)
            },
            WindowInfo {
                visible: false,
                ..sample_window("b", 2, 1200)
            },
        ];
        let restore_actions = state.toggle_group(0, &all_hidden);
        let restored_rects: BTreeMap<_, _> = restore_actions
            .into_iter()
            .filter_map(|action| match action {
                WindowAction::SetRect {
                    window_id, rect, ..
                } => Some((window_id, rect)),
                _ => None,
            })
            .collect();

        assert_eq!(restored_rects.get("a").map(|rect| rect.x), None);
        assert_eq!(restored_rects.get("b").map(|rect| rect.x), Some(1200));
    }

    #[test]
    fn mixed_visibility_toggle_recovers_toward_last_requested_group_state() {
        let mut state = AppState::default();
        state.selected_windows.extend(["a".into(), "b".into()]);
        assert!(state.create_group("MuMu set".into()));
        state.toggle_group(0, &[sample_window("a", 1, 10), sample_window("b", 2, 900)]);

        // Hiding only succeeded for a. The next toggle should show a, while
        // leaving b at its current position instead of moving it to its old one.
        let mixed = [
            WindowInfo {
                visible: false,
                ..sample_window("a", 1, 10)
            },
            sample_window("b", 2, 1200),
        ];
        let actions = state.toggle_group(0, &mixed);
        assert_eq!(actions.len(), 2);
        assert!(matches!(
            actions[0],
            WindowAction::SetRect { ref window_id, rect, .. }
                if window_id == "a" && rect.x == 10
        ));
        assert!(matches!(
            actions[1],
            WindowAction::SetVisible { ref window_id, visible: true, .. }
                if window_id == "a"
        ));
        assert!(state.groups[0].is_visible);
    }

    #[test]
    fn macos_hide_application_option_is_carried_by_group_visibility_actions() {
        let mut state = AppState::default();
        state.selected_windows.insert("a".into());
        assert!(state.create_group("App group".into()));
        state.groups[0].macos_hide_application = true;
        let windows = [sample_window("a", 1, 10)];

        let hide = state.toggle_group(0, &windows);
        assert!(matches!(
            hide.as_slice(),
            [WindowAction::SetVisible {
                visible: false,
                hide_application: true,
                ..
            }]
        ));

        let hidden = [WindowInfo {
            visible: false,
            ..sample_window("a", 1, 10)
        }];
        let show = state.toggle_group(0, &hidden);
        assert!(show.iter().any(|action| matches!(
            action,
            WindowAction::SetVisible {
                visible: true,
                hide_application: true,
                ..
            }
        )));
        assert!(
            !show
                .iter()
                .any(|action| matches!(action, WindowAction::SetRect { .. }))
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
    fn older_saved_groups_default_to_per_window_minimizing() {
        let mut state = AppState::default();
        state.selected_windows.insert("a".into());
        assert!(state.create_group("Legacy group".into()));
        let mut value = serde_json::to_value(state).unwrap();
        value["groups"][0]
            .as_object_mut()
            .unwrap()
            .remove("macos_hide_application");

        let loaded: AppState = serde_json::from_value(value).unwrap();
        assert!(!loaded.groups[0].macos_hide_application);
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
    fn corrupt_state_is_preserved_before_recovery() {
        let path =
            std::env::temp_dir().join(format!("veilkey-corrupt-{}.json", std::process::id()));
        let backup = path.with_extension(format!("json.corrupt-{}", std::process::id()));
        fs::write(&path, b"{not valid json").unwrap();
        let recovered = load_state(&path);
        assert!(recovered.groups.is_empty());
        assert_eq!(fs::read_to_string(&backup).unwrap(), "{not valid json");
        let _ = fs::remove_file(path);
        let _ = fs::remove_file(backup);
    }

    #[test]
    fn only_one_process_can_hold_the_application_instance_lock() {
        let path = std::env::temp_dir().join(format!(
            "veilkey-instance-{}-{:?}.lock",
            std::process::id(),
            std::thread::current().id()
        ));
        let first = acquire_instance_lock(&path).unwrap();
        assert!(acquire_instance_lock(&path).is_err());
        drop(first);
        let reacquired = acquire_instance_lock(&path).unwrap();
        drop(reacquired);
        fs::remove_file(path).unwrap();
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

    #[test]
    fn group_toggle_does_not_match_reused_handle_from_another_process() {
        let mut state = AppState::default();
        state
            .selected_windows
            .insert(format!("mumu\0Player\0{}|hwnd:2a|pid:100", 0));
        assert!(state.create_group("MuMu set".into()));
        let reused_handle = [WindowInfo {
            id: format!("mumu\0Player\0{}|hwnd:2a|pid:200", 0),
            ..sample_window("ignored", 0x2a, 500)
        }];
        assert!(state.toggle_group(0, &reused_handle).is_empty());
    }

    #[test]
    fn macos_accessibility_identity_survives_window_resorting_but_not_pid_reuse() {
        let saved = format!("app\0Same title\0{}\0pid:100|ax:abc", 2);
        let moved = format!("app\0Same title\0{}\0pid:100|ax:abc", 0);
        let other_process = format!("app\0Same title\0{}\0pid:200|ax:abc", 0);
        assert!(same_window_id(&moved, &saved));
        assert!(!same_window_id(&other_process, &saved));
        let legacy = format!("app\0Same title\0{}\0pid:100", 2);
        let compatible = format!("{legacy}|ax:abc");
        assert!(same_window_id(&compatible, &legacy));
    }

    #[test]
    fn window_list_excludes_hidden_and_zero_sized_windows() {
        let visible = sample_window("visible", 1, 0);
        let hidden = WindowInfo {
            visible: false,
            ..sample_window("hidden", 2, 0)
        };
        let empty = WindowInfo {
            rect: Rect {
                width: 0,
                height: 0,
                ..Rect::default()
            },
            ..sample_window("empty", 3, 0)
        };
        assert!(window_is_displayable(&visible));
        assert!(!window_is_displayable(&hidden));
        assert!(!window_is_displayable(&empty));
    }

    #[test]
    fn window_preview_rejects_incomplete_rgba_buffers() {
        assert!(WindowPreview::new(2, 1, vec![0; 8]).is_some());
        assert!(WindowPreview::new(2, 1, vec![0; 7]).is_none());
        assert!(WindowPreview::new(0, 1, Vec::new()).is_none());
    }

    #[test]
    fn thumbnail_bgra_conversion_preserves_alpha_and_swaps_red_blue() {
        assert_eq!(bgra_to_rgba(vec![10, 20, 30, 255]), vec![30, 20, 10, 255]);
    }

    #[test]
    fn dynamic_window_mode_targets_only_the_focused_window() {
        let windows = [
            sample_window("focused", 1, 0),
            sample_window("sibling", 2, 800),
        ];
        assert_eq!(
            dynamic_target_ids(BindMode::OneWindowAtATime, "focused", &windows),
            Some(vec!["focused".to_owned()])
        );
    }

    #[test]
    fn dynamic_application_mode_targets_visible_windows_from_same_process() {
        let windows = [
            WindowInfo {
                id: "focused".into(),
                process_id: 100,
                ..sample_window("focused", 1, 0)
            },
            WindowInfo {
                id: "sibling".into(),
                process_id: 100,
                ..sample_window("sibling", 2, 800)
            },
            WindowInfo {
                id: "other-app".into(),
                process_id: 200,
                ..sample_window("other-app", 3, 1600)
            },
        ];
        assert_eq!(
            dynamic_target_ids(BindMode::ActiveApplication, "focused", &windows),
            Some(vec!["focused".to_owned(), "sibling".to_owned()])
        );
    }

    #[test]
    fn only_manual_multi_window_mode_is_not_dynamic() {
        assert!(!BindMode::SelectFromList.is_dynamic());
        assert!(BindMode::OneWindowAtATime.is_dynamic());
        assert!(BindMode::ActiveApplication.is_dynamic());
    }

    #[test]
    fn fresh_install_and_legacy_settings_have_same_shortcut_range() {
        let fresh = AppState::default();
        let legacy: AppState = serde_json::from_str(r#"{"groups":[]}"#).unwrap();
        assert_eq!(fresh.dynamic_suffixes, legacy.dynamic_suffixes);
        assert!(!fresh.dynamic_suffixes.is_empty());
    }

    #[test]
    fn shortcut_suffixes_are_deduplicated_without_reordering() {
        assert_eq!(
            dynamic_hotkey_combinations(
                &["super".into()],
                &["Digit9".into(), "Digit0".into(), "Digit9".into()]
            ),
            vec!["super+Digit9", "super+Digit0"]
        );
    }

    #[test]
    fn dynamic_shortcut_range_expands_all_modifier_pairs_for_each_suffix() {
        assert_eq!(
            dynamic_hotkey_combinations(
                &["shift".into(), "ctrl".into(), "alt".into()],
                &["KeyU".into(), "KeyI".into(), "KeyO".into()]
            ),
            vec![
                "alt+ctrl+KeyU",
                "alt+shift+KeyU",
                "ctrl+shift+KeyU",
                "alt+ctrl+KeyI",
                "alt+shift+KeyI",
                "ctrl+shift+KeyI",
                "alt+ctrl+KeyO",
                "alt+shift+KeyO",
                "ctrl+shift+KeyO",
            ]
        );
    }

    #[test]
    fn dynamic_hotkey_reveals_same_target_and_restores_old_target_on_switch() {
        let windows = [
            sample_window("first", 1, 0),
            sample_window("second", 2, 800),
        ];
        let hidden = vec!["first".to_owned()];
        assert_eq!(
            dynamic_transition(BindMode::OneWindowAtATime, "first", &windows, &hidden),
            Some(DynamicTransition {
                show_ids: vec!["first".into()],
                hide_ids: vec![],
            })
        );
        assert_eq!(
            dynamic_transition(BindMode::OneWindowAtATime, "second", &windows, &hidden),
            Some(DynamicTransition {
                show_ids: vec!["first".into()],
                hide_ids: vec!["second".into()],
            })
        );
    }
}
