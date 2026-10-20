use eframe::egui;
use global_hotkey::hotkey::HotKey;
use global_hotkey::{GlobalHotKeyEvent, GlobalHotKeyManager, HotKeyState};
use std::collections::VecDeque;
use std::collections::{HashMap, HashSet};
use std::fs::File;
use std::str::FromStr;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver};
use std::thread;
use std::time::{Duration, Instant};
use veilkey::platform::NativeWindowController;
use veilkey::{
    AppState, BindMode, Group, WindowAction, WindowInfo, acquire_instance_lock, dynamic_target_ids,
    load_state, save_state, state_path, window_is_displayable,
};

#[cfg(target_os = "windows")]
mod windows_keyboard;

fn tr<'a>(english: bool, en: &'a str, zh: &'a str) -> &'a str {
    if english { en } else { zh }
}

fn localized_status(english: bool, status: &str) -> String {
    if english {
        return status.to_owned();
    }
    match status {
        "Shortcut registered." => "快捷键已绑定。".into(),
        "Dynamic shortcut ranges registered." => "快捷键范围已生效。".into(),
        "Shortcut range saved. Select a dynamic mode to activate it." => {
            "快捷键范围已保存；切换到动态模式后即可启用。".into()
        }
        "VeilKey was updated. Re-enable Accessibility and Screen Recording permissions." => {
            "VeilKey 已更新并重置授权，请重新开启辅助功能和屏幕录制权限。".into()
        }
        "Window state recovered after a brief retry." => "窗口状态已自动恢复。".into(),
        "Shortcuts recovered." => "快捷键已恢复。".into(),
        "Keyboard listener recovered." => "全局按键监听已恢复。".into(),
        "Window operation is already in progress." => "窗口操作正在执行，请稍候。".into(),
        "Window operation worker stopped." => "窗口操作服务已停止，请重启应用。".into(),
        "Too many window shortcuts are queued." => "窗口快捷键请求过多，部分请求已忽略。".into(),
        "Press the shortcut now." => "请按下要绑定的快捷键。".into(),
        "That shortcut is already assigned to another group." => {
            "这个快捷键已分配给其他窗口组。".into()
        }
        "This shortcut is already assigned to the group." => "该快捷键已经绑定到此组。".into(),
        "Global shortcuts are unavailable on this system." => "当前系统无法启用全局快捷键。".into(),
        _ if status.starts_with("Global key capture unavailable:") => {
            format!(
                "全局按键监听暂不可用，正在重试：{}",
                status.trim_start_matches("Global key capture unavailable:")
            )
        }
        _ if status.starts_with("Could not register shortcut:") => {
            format!(
                "快捷键注册失败：{}",
                status.trim_start_matches("Could not register shortcut:")
            )
        }
        _ if status.starts_with("Could not register shortcut range:") => format!(
            "快捷键范围注册失败，已保留原配置：{}",
            status.trim_start_matches("Could not register shortcut range:")
        ),
        _ if status.starts_with("Shortcut range partially registered:") => format!(
            "部分快捷键因系统或其他程序占用而未启用，其余可用按键仍然有效。{}",
            status.trim_start_matches("Shortcut range partially registered:")
        ),
        _ if status.starts_with("Some group window actions failed:") => {
            format!(
                "部分窗口操作失败：{}",
                status.trim_start_matches("Some group window actions failed:")
            )
        }
        _ => status.to_owned(),
    }
}

#[cfg(any(target_os = "macos", target_os = "windows"))]
fn create_tray(
    ctx: &egui::Context,
) -> Result<(tray_icon::TrayIcon, Receiver<TrayCommand>, Arc<AtomicBool>), String> {
    use tray_icon::menu::{Menu, MenuEvent, MenuItem, PredefinedMenuItem};
    use tray_icon::{Icon, TrayIconBuilder, TrayIconEvent};

    const SHOW_ID: &str = "veilkey.show";
    const EXIT_ID: &str = "veilkey.exit";
    let show = MenuItem::with_id(SHOW_ID, "打开 VeilKey", true, None);
    let exit = MenuItem::with_id(EXIT_ID, "退出 VeilKey", true, None);
    let menu = Menu::new();
    menu.append_items(&[&show, &PredefinedMenuItem::separator(), &exit])
        .map_err(|error| error.to_string())?;
    let pixels = (0_i32..32)
        .flat_map(|y| {
            (0_i32..32).flat_map(move |x| {
                let stroke = (5..27).contains(&y)
                    && ((x - (7 + (y - 5) / 2)).abs() < 2 || (x - (25 - (y - 5) / 2)).abs() < 2);
                [0, 0, 0, if stroke { 255 } else { 0 }]
            })
        })
        .collect();
    let icon = Icon::from_rgba(pixels, 32, 32).map_err(|error| error.to_string())?;
    let builder = TrayIconBuilder::new().with_tooltip("VeilKey");
    #[cfg(target_os = "macos")]
    let builder = builder.with_icon_templated(icon);
    #[cfg(target_os = "windows")]
    let builder = builder.with_icon(icon);
    let tray = builder
        .with_menu(Box::new(menu))
        .with_menu_on_left_click(false)
        .build()
        .map_err(|error| error.to_string())?;

    let (sender, receiver) = mpsc::channel();
    let menu_sender = sender.clone();
    let menu_ctx = ctx.clone();
    MenuEvent::set_event_handler(Some(move |event: MenuEvent| {
        let command = match event.id().as_ref() {
            SHOW_ID => Some(TrayCommand::Show),
            EXIT_ID => Some(TrayCommand::Exit),
            _ => None,
        };
        if let Some(command) = command {
            let _ = menu_sender.send(command);
            menu_ctx.request_repaint();
        }
    }));
    let tray_ctx = ctx.clone();
    TrayIconEvent::set_event_handler(Some(move |event| {
        if matches!(
            event,
            TrayIconEvent::Click {
                button: tray_icon::MouseButton::Left,
                button_state: tray_icon::MouseButtonState::Up,
                ..
            } | TrayIconEvent::DoubleClick {
                button: tray_icon::MouseButton::Left,
                ..
            }
        ) {
            let _ = sender.send(TrayCommand::Show);
            tray_ctx.request_repaint();
        }
    }));
    Ok((tray, receiver, Arc::new(AtomicBool::new(false))))
}

fn capture_retry_delay(failures: u32) -> Duration {
    Duration::from_secs(2_u64.saturating_mul(1_u64 << failures.saturating_sub(1).min(5)))
        .min(Duration::from_secs(60))
}

fn capture_retry_delay_after_failure(failures: &mut u32, listener_uptime: Duration) -> Duration {
    if listener_uptime >= Duration::from_secs(300) {
        *failures = 0;
    }
    *failures = failures.saturating_add(1);
    capture_retry_delay(*failures)
}

fn request_window_refresh(sender: &mpsc::SyncSender<bool>, include_previews: bool) {
    let _ = sender.try_send(include_previews);
}

fn enqueue_toggle<T: PartialEq>(queue: &mut VecDeque<T>, value: T, capacity: usize) -> bool {
    if queue.back() == Some(&value) {
        queue.pop_back();
        true
    } else if queue.len() < capacity {
        queue.push_back(value);
        true
    } else {
        false
    }
}

fn pending_hotkeys(groups: &[Group], registered_ids: &HashSet<u32>) -> Vec<(String, HotKey)> {
    groups
        .iter()
        .filter_map(|group| {
            let value = group.hotkey.as_deref()?;
            let hotkey = HotKey::from_str(value).ok()?;
            (!registered_ids.contains(&hotkey.id)).then(|| (group.name.clone(), hotkey))
        })
        .collect()
}

fn group_index_by_name(groups: &[Group], name: &str) -> Option<usize> {
    groups.iter().position(|group| group.name == name)
}

#[cfg(not(target_os = "windows"))]
fn is_modifier(key: rdev::Key) -> bool {
    matches!(
        key,
        rdev::Key::ControlLeft
            | rdev::Key::ControlRight
            | rdev::Key::Alt
            | rdev::Key::AltGr
            | rdev::Key::ShiftLeft
            | rdev::Key::ShiftRight
            | rdev::Key::MetaLeft
            | rdev::Key::MetaRight
    )
}

#[cfg(not(target_os = "windows"))]
fn hotkey_key_name(key: rdev::Key) -> Option<&'static str> {
    use rdev::Key::*;
    Some(match key {
        KeyA => "KeyA",
        KeyB => "KeyB",
        KeyC => "KeyC",
        KeyD => "KeyD",
        KeyE => "KeyE",
        KeyF => "KeyF",
        KeyG => "KeyG",
        KeyH => "KeyH",
        KeyI => "KeyI",
        KeyJ => "KeyJ",
        KeyK => "KeyK",
        KeyL => "KeyL",
        KeyM => "KeyM",
        KeyN => "KeyN",
        KeyO => "KeyO",
        KeyP => "KeyP",
        KeyQ => "KeyQ",
        KeyR => "KeyR",
        KeyS => "KeyS",
        KeyT => "KeyT",
        KeyU => "KeyU",
        KeyV => "KeyV",
        KeyW => "KeyW",
        KeyX => "KeyX",
        KeyY => "KeyY",
        KeyZ => "KeyZ",
        Num0 => "Digit0",
        Num1 => "Digit1",
        Num2 => "Digit2",
        Num3 => "Digit3",
        Num4 => "Digit4",
        Num5 => "Digit5",
        Num6 => "Digit6",
        Num7 => "Digit7",
        Num8 => "Digit8",
        Num9 => "Digit9",
        F1 => "F1",
        F2 => "F2",
        F3 => "F3",
        F4 => "F4",
        F5 => "F5",
        F6 => "F6",
        F7 => "F7",
        F8 => "F8",
        F9 => "F9",
        F10 => "F10",
        F11 => "F11",
        F12 => "F12",
        Space => "Space",
        Tab => "Tab",
        Return => "Enter",
        Escape => "Escape",
        Backspace => "Backspace",
        Delete => "Delete",
        Home => "Home",
        End => "End",
        PageUp => "PageUp",
        PageDown => "PageDown",
        LeftArrow => "ArrowLeft",
        RightArrow => "ArrowRight",
        UpArrow => "ArrowUp",
        DownArrow => "ArrowDown",
        Minus => "Minus",
        Equal => "Equal",
        BackQuote => "Backquote",
        _ => return None,
    })
}

struct VeilKeyApp {
    state: AppState,
    windows: Vec<WindowInfo>,
    window_rx: Receiver<Result<Vec<WindowInfo>, String>>,
    refresh_tx: mpsc::SyncSender<bool>,
    hotkeys: Option<GlobalHotKeyManager>,
    state_file: std::path::PathBuf,
    _instance_lock: File,
    filter: String,
    hotkey_retry_needed: bool,
    last_hotkey_retry: Instant,
    registered_hotkey_ids: HashSet<u32>,
    recording_group: Option<String>,
    capture_armed: Arc<AtomicBool>,
    capture_listener_alive: Arc<AtomicBool>,
    capture_listener_was_alive: bool,
    capture_rx: Receiver<Result<String, String>>,
    focused_binding_rx: Receiver<FocusedBindingLookup>,
    focused_binding_tx: mpsc::Sender<FocusedBindingLookup>,
    focused_binding_in_flight: bool,
    pending_bind_group: Option<String>,
    dynamic_toggle_rx: Receiver<DynamicToggleLookup>,
    dynamic_toggle_tx: mpsc::Sender<DynamicToggleLookup>,
    dynamic_toggle_in_flight: bool,
    dynamic_hotkey_roles: HashMap<u32, DynamicHotkeyRole>,
    dynamic_range_draft: AppState,
    dynamic_suffix_input: String,
    dynamic_config_error: String,
    dynamic_recovery: Vec<Group>,
    action_tx: mpsc::SyncSender<Vec<WindowAction>>,
    action_rx: Receiver<ActionCompletion>,
    action_in_flight: bool,
    queued_toggles: VecDeque<String>,
    preview_textures: HashMap<String, egui::TextureHandle>,
    preview_pixels: HashMap<String, veilkey::WindowPreview>,
    #[cfg(any(target_os = "macos", target_os = "windows"))]
    tray_icon: Option<tray_icon::TrayIcon>,
    #[cfg(any(target_os = "macos", target_os = "windows"))]
    tray_events: Receiver<TrayCommand>,
    #[cfg(any(target_os = "macos", target_os = "windows"))]
    exit_requested: Arc<AtomicBool>,
}

type ActionCompletion = (Result<(), String>, Result<Vec<WindowInfo>, String>);
type FocusedBindingLookup = (
    String,
    Result<Option<String>, String>,
    Result<Vec<WindowInfo>, String>,
);
type DynamicToggleLookup = (
    BindMode,
    String,
    Result<Option<String>, String>,
    Result<Vec<WindowInfo>, String>,
);

#[derive(Clone)]
struct DynamicHotkeyRole {
    suffix: String,
    hotkey: HotKey,
}

fn plan_dynamic_hotkeys(state: &AppState) -> Result<Vec<DynamicHotkeyRole>, String> {
    if !state.bind_mode.is_dynamic() {
        return Ok(Vec::new());
    }
    if state.dynamic_suffixes.is_empty() {
        return Err("请至少选择一个可用按键。".into());
    }
    if state.dynamic_toggle_modifiers.is_empty()
        || state
            .dynamic_toggle_modifiers
            .iter()
            .any(|key| !["super", "ctrl", "alt", "shift"].contains(&key.as_str()))
        || !state
            .dynamic_toggle_modifiers
            .iter()
            .any(|key| key != "shift")
    {
        return Err("请至少允许 Command / Win、Control / Ctrl 或 Option / Alt 中的一项。".into());
    }
    let mut roles = Vec::new();
    let mut ids = HashSet::new();
    let mut suffixes = HashSet::new();
    for suffix in &state.dynamic_suffixes {
        if !suffixes.insert(suffix) || !available_suffixes().iter().any(|(key, _)| key == suffix) {
            return Err(format!("无效或重复的按键：{suffix}"));
        }
    }
    let modifiers = &state.dynamic_toggle_modifiers;
    if modifiers.is_empty()
        || modifiers
            .iter()
            .any(|key| !["ctrl", "alt", "shift", "super"].contains(&key.as_str()))
    {
        return Err("请选择有效的修饰键组合。".into());
    }
    for chord in veilkey::dynamic_hotkey_combinations(modifiers, &state.dynamic_suffixes) {
        let suffix = chord.rsplit('+').next().unwrap_or_default();
        let hotkey = HotKey::from_str(&chord).map_err(|error| error.to_string())?;
        if !ids.insert(hotkey.id) {
            return Err("快捷键不能重复。".into());
        }
        if state.groups.iter().any(|group| {
            group
                .hotkey
                .as_deref()
                .and_then(|saved| HotKey::from_str(saved).ok())
                .is_some_and(|saved| saved.id == hotkey.id)
        }) {
            return Err(format!("快捷键与窗口组合冲突：{chord}"));
        }
        roles.push(DynamicHotkeyRole {
            suffix: suffix.to_owned(),
            hotkey,
        });
    }
    Ok(roles)
}

fn readable_chord(chord: &str, macos: bool) -> String {
    let mut parts: Vec<_> = chord.split('+').map(str::to_owned).collect();
    let suffix = parts.pop().unwrap_or_default();
    readable_shortcut(&parts, &suffix, macos)
}

fn is_bound_in_session(group: &Group) -> bool {
    !group.window_ids.is_empty()
}

fn available_suffixes() -> Vec<(String, String)> {
    (0..=9)
        .map(|n| (format!("Digit{n}"), n.to_string()))
        .chain(('A'..='Z').map(|ch| (format!("Key{ch}"), ch.to_string())))
        .chain((1..=12).map(|n| (format!("F{n}"), format!("F{n}"))))
        .chain([
            ("Minus".into(), "-".into()),
            ("Equal".into(), "=".into()),
            ("Semicolon".into(), ";".into()),
        ])
        .collect()
}

fn suffix_input(keys: &[String]) -> String {
    let available = available_suffixes();
    keys.iter()
        .filter_map(|key| {
            available
                .iter()
                .find(|(code, _)| code == key)
                .map(|(code, label)| {
                    if code.starts_with("Key") {
                        label.to_ascii_lowercase()
                    } else {
                        label.clone()
                    }
                })
        })
        .collect::<Vec<_>>()
        .join(" ")
}

fn parse_suffix_input(input: &str) -> Result<Vec<String>, String> {
    let available = available_suffixes();
    let mut keys = Vec::new();
    for token in input.split(|ch: char| ch.is_whitespace() || ch == ',' || ch == '，') {
        if token.is_empty() {
            continue;
        }
        let labels = if token
            .get(..1)
            .is_some_and(|first| first.eq_ignore_ascii_case("f"))
            && token
                .get(1..)
                .is_some_and(|rest| rest.parse::<u8>().is_ok())
        {
            vec![token.to_ascii_uppercase()]
        } else {
            token
                .chars()
                .map(|ch| ch.to_ascii_uppercase().to_string())
                .collect()
        };
        for label in labels {
            let Some((code, _)) = available.iter().find(|(_, name)| name == &label) else {
                return Err(format!("不支持的按键：{label}"));
            };
            if !keys.contains(code) {
                keys.push(code.clone());
            }
        }
    }
    if keys.is_empty() {
        return Err("请至少填写一个后缀键。".into());
    }
    Ok(keys)
}

fn readable_shortcut(modifiers: &[String], suffix: &str, macos: bool) -> String {
    let mut parts = Vec::new();
    for (key, label) in [
        ("super", if macos { "Command" } else { "Win" }),
        ("ctrl", if macos { "Control" } else { "Ctrl" }),
        ("alt", if macos { "Option" } else { "Alt" }),
        ("shift", "Shift"),
    ] {
        if modifiers.iter().any(|value| value == key) {
            parts.push(label.to_owned());
        }
    }
    let label = available_suffixes()
        .into_iter()
        .find(|(key, _)| key == suffix)
        .map(|(key, label)| {
            if key.starts_with("Key") {
                label.to_ascii_lowercase()
            } else {
                label
            }
        })
        .unwrap_or_else(|| suffix.to_owned());
    parts.push(label);
    parts.join(" + ")
}

fn slot_actions(group: &mut Group, windows: &[WindowInfo]) -> Vec<WindowAction> {
    let mut state = AppState {
        groups: vec![group.clone()],
        ..Default::default()
    };
    let actions = state.toggle_group(0, windows);
    *group = state.groups.remove(0);
    actions
}

fn should_bind_slot(group: Option<&Group>, focused: Option<&str>, windows: &[WindowInfo]) -> bool {
    let Some(group) = group.filter(|group| !group.window_ids.is_empty()) else {
        return true;
    };
    if !group.window_ids.iter().any(|id| {
        windows
            .iter()
            .any(|window| veilkey::same_window_id(&window.id, id))
    }) {
        return true;
    }
    if focused.is_some_and(|id| {
        windows
            .iter()
            .any(|window| window.id == id && window.process_id == std::process::id())
    }) {
        return false;
    }
    // Hiding transfers focus elsewhere. The next press must restore this slot,
    // rather than accidentally capturing whichever application received focus.
    if !group.is_visible
        || group.window_ids.iter().any(|id| {
            windows
                .iter()
                .any(|window| veilkey::same_window_id(&window.id, id) && !window.visible)
        })
    {
        return false;
    }
    focused.is_some_and(|focused| {
        !group
            .window_ids
            .iter()
            .any(|saved| veilkey::same_window_id(focused, saved))
    })
}

fn restore_group_actions(group: &Group, windows: &[WindowInfo]) -> Vec<WindowAction> {
    let mut actions = Vec::new();
    for id in &group.window_ids {
        if let Some(window) = windows
            .iter()
            .find(|window| veilkey::same_window_id(&window.id, id) && !window.visible)
        {
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
    actions
}

fn restore_session_groups<'a>(
    groups: impl Iterator<Item = &'a mut Group>,
    windows: &[WindowInfo],
) -> Vec<WindowAction> {
    let mut actions = Vec::new();
    for group in groups {
        if !group.is_visible {
            actions.extend(restore_group_actions(group, windows));
            group.is_visible = true;
        }
    }
    actions
}

fn retain_unrestored_targets(recovery: &mut Vec<Group>, windows: &[WindowInfo]) {
    for group in recovery.iter_mut() {
        group.window_ids.retain(|id| {
            windows
                .iter()
                .any(|window| veilkey::same_window_id(&window.id, id) && !window.visible)
        });
    }
    recovery.retain(|group| !group.window_ids.is_empty());
}

#[cfg(any(target_os = "macos", target_os = "windows"))]
#[derive(Clone, Copy)]
enum TrayCommand {
    Show,
    Exit,
}

impl VeilKeyApp {
    fn new(instance_lock: File, ctx: &egui::Context) -> Self {
        let state_file = state_path();
        let mut state = load_state(&state_file);
        #[cfg(target_os = "windows")]
        if state.dynamic_toggle_modifiers == ["super"]
            && state.dynamic_suffixes == ["Digit9", "Digit0", "Minus", "Equal"]
        {
            // Older Windows builds defaulted to Win+number, which Windows
            // reserves for taskbar app switching. Migrate only that exact
            // untouched default so existing installs remain usable.
            state.dynamic_toggle_modifiers = vec!["ctrl".into(), "alt".into()];
        }
        let mut dynamic_recovery: Vec<Group> = std::mem::take(&mut state.dynamic_groups)
            .into_values()
            .filter(|group| !group.is_visible)
            .collect();
        if !dynamic_recovery.is_empty()
            && let Ok(windows) = NativeWindowController::enumerate_fast()
        {
            let recovery_actions: Vec<_> = dynamic_recovery
                .iter()
                .flat_map(|group| restore_group_actions(group, &windows))
                .collect();
            if let Err(error) = NativeWindowController::apply_actions(recovery_actions) {
                eprintln!("Could not restore windows from the previous VeilKey session: {error}");
            }
            dynamic_recovery.clear();
        }
        #[cfg(any(target_os = "macos", target_os = "windows"))]
        let (tray_icon, tray_events, exit_requested) = match create_tray(ctx) {
            Ok(tray) => (Some(tray.0), tray.1, tray.2),
            Err(error) => {
                eprintln!("Could not create VeilKey notification-area icon: {error}");
                let (_sender, receiver) = mpsc::channel();
                (None, receiver, Arc::new(AtomicBool::new(false)))
            }
        };
        for group in &mut state.groups {
            group.hotkey_draft = group.hotkey.clone().unwrap_or_default();
        }
        let dynamic_range_draft = state.clone();
        let dynamic_suffix_input = suffix_input(&state.dynamic_suffixes);
        // A slow Accessibility query must never let stale snapshots accumulate.
        let (window_tx, window_rx) = mpsc::sync_channel(1);
        let (refresh_tx, refresh_rx) = mpsc::sync_channel(1);
        thread::spawn(move || {
            loop {
                let include_previews = match refresh_rx.recv_timeout(Duration::from_secs(10)) {
                    Ok(include_previews) => include_previews,
                    Err(mpsc::RecvTimeoutError::Timeout) => false,
                    Err(mpsc::RecvTimeoutError::Disconnected) => break,
                };
                let mut include_previews = include_previews;
                while let Ok(requested) = refresh_rx.try_recv() {
                    include_previews |= requested;
                }
                let windows = if include_previews {
                    NativeWindowController::enumerate()
                } else {
                    NativeWindowController::enumerate_fast()
                };
                if window_tx.send(windows).is_err() {
                    break;
                }
            }
        });
        let (action_tx, action_requests) = mpsc::sync_channel::<Vec<WindowAction>>(1);
        let (action_results, action_rx) = mpsc::channel();
        let action_refresh_tx = refresh_tx.clone();
        thread::spawn(move || {
            while let Ok(actions) = action_requests.recv() {
                let retry = actions.clone();
                let applied = match NativeWindowController::apply_actions(actions) {
                    Ok(()) => Ok(()),
                    Err(first) => match NativeWindowController::apply_actions(retry) {
                        Ok(()) => Ok(()),
                        Err(second) => Err(format!("{first}; retry failed: {second}")),
                    },
                };
                if action_results.send((applied, Ok(Vec::new()))).is_err() {
                    break;
                }
                let _ = action_refresh_tx.try_send(false);
            }
        });
        let capture_armed = Arc::new(AtomicBool::new(false));
        let listener_capture_armed = Arc::clone(&capture_armed);
        let capture_listener_alive = Arc::new(AtomicBool::new(false));
        let listener_alive = Arc::clone(&capture_listener_alive);
        let (capture_tx, capture_rx) = mpsc::sync_channel(32);
        let (focused_binding_tx, focused_binding_rx) = mpsc::channel();
        let (dynamic_toggle_tx, dynamic_toggle_rx) = mpsc::channel();
        thread::spawn(move || {
            let mut failures = 0_u32;
            loop {
                let attempt_started = Instant::now();
                #[cfg(target_os = "windows")]
                let error = windows_keyboard::listen(
                    Arc::clone(&listener_capture_armed),
                    Arc::clone(&listener_alive),
                    capture_tx.clone(),
                )
                .err()
                .unwrap_or_else(|| "listener stopped unexpectedly".into());
                #[cfg(not(target_os = "windows"))]
                let error = {
                    let mut down = HashSet::<rdev::Key>::new();
                    let listener_armed = Arc::clone(&listener_capture_armed);
                    let callback_tx = capture_tx.clone();
                    listener_alive.store(true, Ordering::Release);
                    let callback = move |event: rdev::Event| match event.event_type {
                        rdev::EventType::KeyPress(key) => {
                            down.insert(key);
                            if listener_armed.load(Ordering::Relaxed) && !is_modifier(key) {
                                let ctrl = down.contains(&rdev::Key::ControlLeft)
                                    || down.contains(&rdev::Key::ControlRight);
                                let alt = down.contains(&rdev::Key::Alt)
                                    || down.contains(&rdev::Key::AltGr);
                                let shift = down.contains(&rdev::Key::ShiftLeft)
                                    || down.contains(&rdev::Key::ShiftRight);
                                let super_key = down.contains(&rdev::Key::MetaLeft)
                                    || down.contains(&rdev::Key::MetaRight);
                                if (ctrl || alt || shift || super_key)
                                    && let Some(key_name) = hotkey_key_name(key)
                                {
                                    let mut parts = Vec::new();
                                    if ctrl {
                                        parts.push("ctrl");
                                    }
                                    if alt {
                                        parts.push("alt");
                                    }
                                    if shift {
                                        parts.push("shift");
                                    }
                                    if super_key {
                                        parts.push("super");
                                    }
                                    parts.push(key_name);
                                    if listener_armed.swap(false, Ordering::AcqRel) {
                                        let _ = callback_tx.send(Ok(parts.join("+")));
                                    }
                                }
                            }
                        }
                        rdev::EventType::KeyRelease(key) => {
                            down.remove(&key);
                        }
                        _ => {}
                    };
                    rdev::listen(callback)
                        .err()
                        .map(|error| format!("{error:?}"))
                        .unwrap_or_else(|| "listener stopped unexpectedly".into())
                };
                listener_alive.store(false, Ordering::Release);
                let _ = capture_tx.send(Err(format!("Global key capture unavailable: {error}")));
                let delay =
                    capture_retry_delay_after_failure(&mut failures, attempt_started.elapsed());
                thread::sleep(delay);
            }
        });
        let hotkeys = GlobalHotKeyManager::new().ok();
        let mut app = Self {
            state,
            windows: Vec::new(),
            window_rx,
            refresh_tx,
            hotkeys,
            state_file,
            _instance_lock: instance_lock,
            filter: String::new(),
            hotkey_retry_needed: false,
            last_hotkey_retry: Instant::now(),
            registered_hotkey_ids: HashSet::new(),
            recording_group: None,
            capture_armed,
            capture_listener_alive,
            capture_listener_was_alive: false,
            capture_rx,
            focused_binding_rx,
            focused_binding_tx,
            focused_binding_in_flight: false,
            pending_bind_group: None,
            dynamic_toggle_rx,
            dynamic_toggle_tx,
            dynamic_toggle_in_flight: false,
            dynamic_hotkey_roles: HashMap::new(),
            dynamic_range_draft,
            dynamic_suffix_input,
            dynamic_config_error: String::new(),
            dynamic_recovery,
            action_tx,
            action_rx,
            action_in_flight: false,
            queued_toggles: VecDeque::new(),
            preview_textures: HashMap::new(),
            preview_pixels: HashMap::new(),
            #[cfg(any(target_os = "macos", target_os = "windows"))]
            tray_icon,
            #[cfg(any(target_os = "macos", target_os = "windows"))]
            tray_events,
            #[cfg(any(target_os = "macos", target_os = "windows"))]
            exit_requested,
        };
        let permission_status = app.state.status.clone();
        app.register_saved_hotkeys();
        if !app.register_dynamic_hotkeys() {
            app.hotkey_retry_needed = true;
        }
        if !permission_status.is_empty() {
            app.state.status = localized_status(app.state.english_ui, &permission_status);
        }
        request_window_refresh(&app.refresh_tx, false);
        app
    }

    fn persist(&mut self) {
        match save_state(&self.state_file, &self.state) {
            Ok(()) => {}
            Err(error) => self.state.status = format!("Could not save settings: {error}"),
        }
    }

    fn register_saved_hotkeys(&mut self) {
        let Some(manager) = &self.hotkeys else {
            self.hotkey_retry_needed = true;
            self.state.status = "Global shortcuts are unavailable on this system.".into();
            return;
        };
        let mut failed = Vec::new();
        for group in &self.state.groups {
            if let Some(value) = group.hotkey.as_deref() {
                match HotKey::from_str(value) {
                    Ok(hotkey) if manager.register(hotkey).is_ok() => {
                        self.registered_hotkey_ids.insert(hotkey.id);
                    }
                    Ok(_) => failed.push(format!("{} ({value})", group.name)),
                    Err(_) => failed.push(format!("{} (invalid shortcut)", group.name)),
                }
            }
        }
        for role in self.dynamic_hotkey_roles.values() {
            if manager.register(role.hotkey).is_ok() {
                self.registered_hotkey_ids.insert(role.hotkey.id);
            } else {
                failed.push(format!("Dynamic shortcut ({})", role.hotkey));
            }
        }
        if !failed.is_empty() {
            self.hotkey_retry_needed = true;
            self.state.status = format!(
                "Some shortcuts could not be registered: {}",
                failed.join(", ")
            );
        }
    }

    fn register_group_hotkey(&mut self, index: usize, value: String) {
        let Some(manager) = &self.hotkeys else {
            self.state.status = "Global shortcuts are unavailable on this system.".into();
            return;
        };
        let Some(hotkey) = HotKey::from_str(&value).ok() else {
            self.state.status = "Use a shortcut such as ctrl+alt+KeyH or super+shift+KeyH.".into();
            return;
        };
        if self.dynamic_hotkey_roles.contains_key(&hotkey.id) {
            self.state.status = "这个快捷键已用于动态窗口范围。".into();
            return;
        }
        if self.state.groups.iter().enumerate().any(|(i, group)| {
            i != index
                && group
                    .hotkey
                    .as_deref()
                    .and_then(|s| HotKey::from_str(s).ok())
                    .is_some_and(|other| other.id == hotkey.id)
        }) {
            self.state.status = "That shortcut is already assigned to another group.".into();
            return;
        }
        let old_value = self
            .state
            .groups
            .get(index)
            .and_then(|group| group.hotkey.clone());
        if old_value
            .as_deref()
            .and_then(|old| HotKey::from_str(old).ok())
            .is_some_and(|old| old.id == hotkey.id)
        {
            self.state.status = "This shortcut is already assigned to the group.".into();
            return;
        }
        if let Some(old) = old_value
            .as_deref()
            .and_then(|old| HotKey::from_str(old).ok())
            && manager.unregister(old).is_ok()
        {
            self.registered_hotkey_ids.remove(&old.id);
        }
        match manager.register(hotkey) {
            Ok(()) => {
                self.registered_hotkey_ids.insert(hotkey.id);
                if let Some(group) = self.state.groups.get_mut(index) {
                    group.hotkey = Some(value);
                    group.hotkey_draft = group.hotkey.clone().unwrap_or_default();
                }
                self.state.status = "Shortcut registered.".into();
                self.persist();
            }
            Err(error) => {
                self.hotkey_retry_needed = true;
                if let Some(old) = old_value
                    .as_deref()
                    .and_then(|old| HotKey::from_str(old).ok())
                    && manager.register(old).is_ok()
                {
                    self.registered_hotkey_ids.insert(old.id);
                }
                self.state.status = format!("Could not register shortcut: {error}");
            }
        }
    }

    fn register_dynamic_hotkeys(&mut self) -> bool {
        let planned = match plan_dynamic_hotkeys(&self.state) {
            Ok(roles) => roles,
            Err(error) => {
                self.state.status = error;
                return false;
            }
        };
        let Some(manager) = &self.hotkeys else {
            self.state.status = "Global shortcuts are unavailable on this system.".into();
            return false;
        };
        let previous: Vec<_> = self.dynamic_hotkey_roles.values().cloned().collect();
        let mut registered: Vec<DynamicHotkeyRole> = Vec::new();
        let mut registration_errors = Vec::new();
        for role in &planned {
            let hotkey = role.hotkey;
            if self.dynamic_hotkey_roles.contains_key(&hotkey.id)
                && self.registered_hotkey_ids.contains(&hotkey.id)
            {
                continue;
            }
            if let Err(error) = manager.register(hotkey) {
                registration_errors.push(error.to_string());
                continue;
            }
            self.registered_hotkey_ids.insert(hotkey.id);
            registered.push(role.clone());
        }
        let mut removed: Vec<DynamicHotkeyRole> = Vec::new();
        for role in &previous {
            if planned.iter().any(|next| next.hotkey.id == role.hotkey.id) {
                continue;
            }
            if let Err(error) = manager.unregister(role.hotkey) {
                for added in &registered {
                    if manager.unregister(added.hotkey).is_ok() {
                        self.registered_hotkey_ids.remove(&added.hotkey.id);
                    }
                }
                for old in removed {
                    if manager.register(old.hotkey).is_ok() {
                        self.registered_hotkey_ids.insert(old.hotkey.id);
                    }
                }
                self.hotkey_retry_needed = true;
                self.state.status = format!("Could not replace shortcut range: {error}");
                return false;
            }
            self.registered_hotkey_ids.remove(&role.hotkey.id);
            removed.push(role.clone());
        }
        self.dynamic_hotkey_roles = planned
            .into_iter()
            .filter(|role| self.registered_hotkey_ids.contains(&role.hotkey.id))
            .map(|role| (role.hotkey.id, role))
            .collect();
        self.hotkey_retry_needed = !registration_errors.is_empty();
        self.state.status = if registration_errors.is_empty() {
            "Dynamic shortcut ranges registered.".into()
        } else {
            format!(
                "Shortcut range partially registered: {} shortcut(s) unavailable. First error: {}",
                registration_errors.len(),
                registration_errors[0]
            )
        };
        self.persist();
        true
    }

    fn apply_actions(&mut self, actions: Vec<WindowAction>) {
        if actions.is_empty() {
            return;
        }
        match self.action_tx.try_send(actions) {
            Ok(()) => self.action_in_flight = true,
            Err(mpsc::TrySendError::Full(_)) => {
                self.state.status = "Window operation is already in progress.".into();
            }
            Err(mpsc::TrySendError::Disconnected(_)) => {
                self.state.status = "Window operation worker stopped.".into();
            }
        }
    }

    fn update_window_actions(&mut self) {
        while let Ok((applied, snapshot)) = self.action_rx.try_recv() {
            self.action_in_flight = false;
            match applied {
                Ok(()) => {}
                Err(error) => self.state.status = error,
            }
            match snapshot {
                Ok(windows) => {
                    if !windows.is_empty() {
                        retain_unrestored_targets(&mut self.dynamic_recovery, &windows);
                        self.windows = windows;
                    }
                }
                Err(error) => {
                    self.queued_toggles.clear();
                    if self.state.status.is_empty() {
                        self.state.status = error;
                    }
                }
            }
        }
        if !self.action_in_flight
            && !self.dynamic_toggle_in_flight
            && let Some(group_name) = self.queued_toggles.pop_front()
            && let Some(index) = self
                .state
                .groups
                .iter()
                .position(|group| group.name == group_name)
        {
            self.toggle_group_now(index);
        }
    }

    fn retry_hotkeys_if_needed(&mut self) {
        if !self.hotkey_retry_needed || self.last_hotkey_retry.elapsed() < Duration::from_secs(5) {
            return;
        }
        self.last_hotkey_retry = Instant::now();
        if self.hotkeys.is_none() {
            let Ok(manager) = GlobalHotKeyManager::new() else {
                return;
            };
            self.hotkeys = Some(manager);
            self.registered_hotkey_ids.clear();
        }
        let Some(manager) = self.hotkeys.as_ref() else {
            return;
        };
        let mut failed = Vec::new();
        for (name, hotkey) in pending_hotkeys(&self.state.groups, &self.registered_hotkey_ids) {
            if manager.register(hotkey).is_ok() {
                self.registered_hotkey_ids.insert(hotkey.id);
            } else {
                failed.push(name);
            }
        }
        failed.extend(
            self.state
                .groups
                .iter()
                .filter(|group| {
                    group
                        .hotkey
                        .as_deref()
                        .is_some_and(|value| HotKey::from_str(value).is_err())
                })
                .map(|group| group.name.clone()),
        );
        for role in self.dynamic_hotkey_roles.values() {
            if !self.registered_hotkey_ids.contains(&role.hotkey.id) {
                if manager.register(role.hotkey).is_ok() {
                    self.registered_hotkey_ids.insert(role.hotkey.id);
                } else {
                    failed.push(format!("Dynamic shortcut {}", role.hotkey));
                }
            }
        }
        self.hotkey_retry_needed = !failed.is_empty();
        if failed.is_empty() {
            self.state.status = "Shortcuts recovered.".into();
        }
        if self.state.bind_mode.is_dynamic()
            && self.dynamic_hotkey_roles.is_empty()
            && !self.register_dynamic_hotkeys()
        {
            self.hotkey_retry_needed = true;
        }
    }

    fn process_global_binding_capture(&mut self, english: bool) {
        let mut captured = None;
        while let Ok(event) = self.capture_rx.try_recv() {
            captured = Some(event);
        }
        let Some(captured) = captured else {
            return;
        };
        match captured {
            Err(error) => {
                self.state.status = error;
                // Ensure a quick fail/restart cycle still produces a visible
                // recovered status even if the UI missed the brief false state.
                self.capture_listener_was_alive = false;
            }
            Ok(chord) => {
                self.capture_armed.store(false, Ordering::Release);
                if let Some(group_name) = self.pending_bind_group.take() {
                    if let Some(index) = group_index_by_name(&self.state.groups, &group_name) {
                        self.register_group_hotkey(index, chord);
                    } else {
                        self.state.status = tr(
                            english,
                            "The target group was removed before the shortcut was recorded.",
                            "录制快捷键前，目标窗口组已被删除。",
                        )
                        .into();
                    }
                } else {
                    self.state.status = tr(
                        english,
                        "Checking the focused window…",
                        "正在识别当前前台窗口…",
                    )
                    .into();
                    self.focused_binding_in_flight = true;
                    let sender = self.focused_binding_tx.clone();
                    thread::spawn(move || {
                        let focused = NativeWindowController::foreground_window_id();
                        let windows = if matches!(focused, Ok(Some(_))) {
                            NativeWindowController::enumerate_fast()
                        } else {
                            Ok(Vec::new())
                        };
                        let _ = sender.send((chord, focused, windows));
                    });
                }
            }
        }
    }

    fn complete_focused_window_bindings(&mut self, english: bool) {
        while let Ok((chord, focused, windows)) = self.focused_binding_rx.try_recv() {
            self.focused_binding_in_flight = false;
            self.complete_focused_window_binding(chord, focused, windows, english);
        }
    }

    fn complete_focused_window_binding(
        &mut self,
        chord: String,
        focused: Result<Option<String>, String>,
        windows: Result<Vec<WindowInfo>, String>,
        english: bool,
    ) {
        let window_id = match focused {
            Ok(Some(window_id)) => window_id,
            Ok(None) => {
                self.state.status = tr(
                    english,
                    "No eligible foreground window found.",
                    "没有找到可绑定的前台窗口。",
                )
                .into();
                return;
            }
            Err(error) => {
                self.state.status = error;
                return;
            }
        };
        let fresh = match windows {
            Ok(windows) => windows,
            Err(error) => {
                self.state.status = error;
                return;
            }
        };
        let Some(window) = fresh
            .iter()
            .find(|window| window.id == window_id && window_is_displayable(window))
        else {
            self.state.status = tr(
                english,
                "No eligible foreground window found.",
                "没有找到可绑定的前台窗口。",
            )
            .into();
            return;
        };
        let mut name = window.title.clone();
        let mut suffix = 2;
        while self
            .state
            .groups
            .iter()
            .any(|group| group.name.eq_ignore_ascii_case(&name))
        {
            name = format!("{} {suffix}", window.title);
            suffix += 1;
        }
        self.state.selected_windows.insert(window.id.clone());
        if self.state.create_group(name) {
            self.windows = fresh;
            let index = self.state.groups.len() - 1;
            self.register_group_hotkey(index, chord);
        } else {
            self.state.status = tr(
                english,
                "Could not create a group for the focused window.",
                "无法为当前前台窗口创建分组。",
            )
            .into();
        }
    }

    fn process_dynamic_toggle_results(&mut self, english: bool) {
        while let Ok((mode, suffix, focused, windows)) = self.dynamic_toggle_rx.try_recv() {
            self.dynamic_toggle_in_flight = false;
            let windows = match windows {
                Ok(windows) => windows,
                Err(error) => {
                    self.state.status = error;
                    continue;
                }
            };
            self.windows = windows;
            let mut actions = Vec::new();
            let binding = should_bind_slot(
                self.state.dynamic_groups.get(&suffix),
                focused.as_ref().ok().and_then(|value| value.as_deref()),
                &self.windows,
            );
            if binding {
                let focused = match focused {
                    Ok(Some(id)) => id,
                    Ok(None) => {
                        self.state.status =
                            tr(english, "No foreground window.", "没有可用的前台窗口。").into();
                        continue;
                    }
                    Err(error) => {
                        self.state.status = error;
                        continue;
                    }
                };
                let Some(ids) = dynamic_target_ids(mode, &focused, &self.windows) else {
                    self.state.status = "无法识别当前窗口目标。".into();
                    continue;
                };
                if ids.iter().any(|id| {
                    self.windows
                        .iter()
                        .any(|w| &w.id == id && w.process_id == std::process::id())
                }) {
                    self.state.status = "请切换到目标应用后再绑定。".into();
                    continue;
                }
                // Restore the previous target before replacing this slot.
                if let Some(old) = self.state.dynamic_groups.get(&suffix) {
                    let old = old.clone();
                    if !old.is_visible {
                        actions.extend(restore_group_actions(&old, &self.windows));
                        self.dynamic_recovery.push(old);
                    }
                }
                // Transfer overlapping targets out of other slots so their shortcuts
                // cannot hide/show the same window with conflicting saved geometry.
                for group in self.state.dynamic_groups.values_mut() {
                    group
                        .window_ids
                        .retain(|id| !ids.iter().any(|next| veilkey::same_window_id(next, id)));
                }
                let title = self
                    .windows
                    .iter()
                    .find(|w| w.id == focused)
                    .map(|w| w.title.clone())
                    .unwrap_or_default();
                let mut group = Group {
                    name: title,
                    window_ids: ids.into_iter().collect(),
                    hotkey: Some(suffix.clone()),
                    hotkey_draft: String::new(),
                    is_visible: true,
                    force_popup: false,
                    macos_hide_application: cfg!(target_os = "macos")
                        && mode == BindMode::ActiveApplication,
                    restore_rects: Default::default(),
                };
                actions.extend(slot_actions(&mut group, &self.windows));
                self.state.dynamic_groups.insert(suffix, group);
            } else if let Some(group) = self.state.dynamic_groups.get_mut(&suffix) {
                actions.extend(slot_actions(group, &self.windows));
            } else {
                self.state.status = "此按键尚未绑定窗口。".into();
                continue;
            }
            if actions.is_empty() {
                self.state.status = tr(
                    english,
                    "No windows could be toggled.",
                    "没有可切换的窗口。",
                )
                .into();
            } else {
                self.state.status.clear();
                self.apply_actions(actions);
                self.persist();
            }
        }
    }

    fn activate_dynamic_hotkey(&mut self, role: DynamicHotkeyRole) {
        if self.dynamic_toggle_in_flight || self.action_in_flight {
            self.state.status = "A window operation is already in progress.".into();
            return;
        }
        self.dynamic_toggle_in_flight = true;
        let mode = self.state.bind_mode;
        let previous_window_ids: Vec<String> = self
            .state
            .dynamic_groups
            .get(&role.suffix)
            .map(|group| group.window_ids.iter().cloned().collect())
            .unwrap_or_default();
        let sender = self.dynamic_toggle_tx.clone();
        thread::spawn(move || {
            let (focused, windows) =
                match NativeWindowController::focused_window_snapshot(&previous_window_ids) {
                    Ok((focused, windows)) => (Ok(focused), Ok(windows)),
                    Err(error) => (Err(error.clone()), Err(error)),
                };
            let _ = sender.send((mode, role.suffix, focused, windows));
        });
    }

    fn capture_hotkey(&mut self, ctx: &egui::Context) {
        let Some(group_name) = self.recording_group.clone() else {
            return;
        };
        let captured = ctx.input(|input| {
            input.events.iter().find_map(|event| match event {
                egui::Event::Key {
                    key,
                    pressed: true,
                    repeat: false,
                    modifiers,
                    ..
                } => Some((*key, *modifiers)),
                _ => None,
            })
        });
        let Some((key, modifiers)) = captured else {
            return;
        };
        let debug_name = format!("{key:?}");
        if matches!(
            debug_name.as_str(),
            "Control" | "Shift" | "Alt" | "Super" | "Command" | "MacCmd"
        ) {
            return;
        }
        let key_name = if debug_name.len() == 1 && debug_name.as_bytes()[0].is_ascii_uppercase() {
            format!("Key{debug_name}")
        } else {
            debug_name
        };
        let mut parts = Vec::new();
        if modifiers.ctrl {
            parts.push("ctrl");
        }
        if modifiers.alt {
            parts.push("alt");
        }
        if modifiers.shift {
            parts.push("shift");
        }
        if modifiers.mac_cmd || (modifiers.command && !cfg!(target_os = "macos") && !modifiers.ctrl)
        {
            parts.push("super");
        }
        parts.push(key_name.as_str());
        let value = parts.join("+");
        self.recording_group = None;
        let Some(index) = group_index_by_name(&self.state.groups, &group_name) else {
            self.state.status = tr(
                self.state.english_ui,
                "The target group was removed before the shortcut was recorded.",
                "录制快捷键前，目标窗口组已被删除。",
            )
            .into();
            return;
        };
        if let Some(group) = self.state.groups.get_mut(index) {
            group.hotkey_draft = value.clone();
        }
        self.register_group_hotkey(index, value);
    }

    fn toggle_group(&mut self, index: usize) {
        if self.action_in_flight || self.dynamic_toggle_in_flight {
            // Preserve the order of groups that may overlap; only adjacent repeated
            // toggles cancel while a native operation is pending.
            let Some(group_name) = self.state.groups.get(index).map(|group| group.name.clone())
            else {
                return;
            };
            if !enqueue_toggle(&mut self.queued_toggles, group_name, 16) {
                self.state.status = "Too many window shortcuts are queued.".into();
            }
            return;
        }
        self.toggle_group_now(index);
    }

    fn toggle_group_now(&mut self, index: usize) {
        let actions = self.state.toggle_group(index, &self.windows);
        if actions.is_empty() {
            self.state.status =
                "No matching windows are available. Keep them open, then refresh the list.".into();
        } else {
            self.state.status.clear();
            self.apply_actions(actions);
            self.persist();
        }
    }

    fn handle_hotkey_events(&mut self) {
        let mut group_to_toggle = None;
        let mut dynamic_toggle = None;
        if let Some(receiver) = Some(GlobalHotKeyEvent::receiver()) {
            while let Ok(event) = receiver.try_recv() {
                if event.state() != HotKeyState::Pressed {
                    continue;
                }
                if let Some(role) = self.dynamic_hotkey_roles.get(&event.id()).cloned() {
                    dynamic_toggle = Some(role);
                    break;
                }
                group_to_toggle = self.state.groups.iter().position(|group| {
                    group
                        .hotkey
                        .as_deref()
                        .and_then(|s| HotKey::from_str(s).ok())
                        .is_some_and(|hotkey| hotkey.id == event.id())
                });
                if group_to_toggle.is_some() {
                    break;
                }
            }
        }
        if let Some(role) = dynamic_toggle {
            self.activate_dynamic_hotkey(role);
        } else if let Some(index) = group_to_toggle {
            self.toggle_group(index);
        }
    }

    fn update_window_snapshot(&mut self) {
        let mut latest = None;
        while let Ok(result) = self.window_rx.try_recv() {
            latest = Some(result);
        }
        if let Some(result) = latest {
            match result {
                Ok(windows) => {
                    self.windows = windows;
                }
                Err(error) => self.state.status = error,
            }
        }
    }

    fn update_capture_listener_status(&mut self) {
        let alive = self.capture_listener_alive.load(Ordering::Acquire);
        if alive
            && !self.capture_listener_was_alive
            && self
                .state
                .status
                .starts_with("Global key capture unavailable:")
        {
            self.state.status = "Keyboard listener recovered.".into();
        }
        self.capture_listener_was_alive = alive;
    }

    #[cfg(any(target_os = "macos", target_os = "windows"))]
    fn process_tray_commands(&mut self, ctx: &egui::Context) {
        while let Ok(command) = self.tray_events.try_recv() {
            match command {
                TrayCommand::Show => {
                    ctx.send_viewport_cmd(egui::ViewportCommand::Visible(true));
                    ctx.send_viewport_cmd(egui::ViewportCommand::Focus);
                }
                TrayCommand::Exit => {
                    self.exit_requested.store(true, Ordering::Release);
                }
            }
        }
        if self.on_exit_requested() && !self.action_in_flight && !self.dynamic_toggle_in_flight {
            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
        }
    }

    #[cfg(any(target_os = "macos", target_os = "windows"))]
    fn on_exit_requested(&self) -> bool {
        self.exit_requested.load(Ordering::Acquire)
    }

    fn dynamic_ui(&mut self, ui: &mut egui::Ui, english: bool, show_bindings: bool) {
        let mut apply = false;
        let mut reset = false;
        let mut cancel = false;
        let parsed = parse_suffix_input(&self.dynamic_suffix_input);
        let dirty = self.dynamic_range_draft.dynamic_toggle_modifiers
            != self.state.dynamic_toggle_modifiers
            || parsed.as_ref().ok() != Some(&self.state.dynamic_suffixes);
        ui.add_space(12.0);
        ui.horizontal(|ui| {
            ui.heading(tr(english, "Shortcuts", "快捷键设置"));
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                apply = ui
                    .add_enabled(
                        dirty && !self.action_in_flight && !self.dynamic_toggle_in_flight,
                        egui::Button::new(tr(english, "Apply configuration", "应用配置")),
                    )
                    .clicked();
                if dirty {
                    cancel = ui.button(tr(english, "Cancel", "取消修改")).clicked();
                    ui.weak(tr(english, "Unsaved changes", "修改未生效"));
                }
            });
        });
        ui.add_space(16.0);
        egui::Grid::new("dynamic_modifiers")
            .spacing([28.0, 16.0])
            .show(ui, |ui| {
                {
                    let label = tr(english, "Shortcut modifiers", "快捷键修饰键");
                    let modifiers = &mut self.dynamic_range_draft.dynamic_toggle_modifiers;
                    ui.label(label);
                    ui.horizontal(|ui| {
                        for (key, label) in [
                            (
                                "super",
                                if cfg!(target_os = "macos") {
                                    "Command"
                                } else {
                                    "Win"
                                },
                            ),
                            (
                                "ctrl",
                                if cfg!(target_os = "macos") {
                                    "Control"
                                } else {
                                    "Ctrl"
                                },
                            ),
                            (
                                "alt",
                                if cfg!(target_os = "macos") {
                                    "Option"
                                } else {
                                    "Alt"
                                },
                            ),
                            ("shift", "Shift"),
                        ] {
                            let mut selected = modifiers.iter().any(|value| value == key);
                            if ui.checkbox(&mut selected, label).changed() {
                                modifiers.retain(|value| value != key);
                                if selected {
                                    modifiers.push(key.into());
                                }
                            }
                        }
                    });
                    ui.end_row();
                }
                ui.label(tr(english, "Key range", "按键范围"));
                ui.add(
                    egui::TextEdit::singleline(&mut self.dynamic_suffix_input)
                        .desired_width(330.0)
                        .hint_text(tr(
                            english,
                            "u i o / F1 F2 / 9 0 - =",
                            "u i o / F1 F2 / 9 0 - =",
                        )),
                );
                ui.end_row();
            });
        ui.add_space(8.0);
        ui.weak(tr(
            english,
            "Each allowed key is registered with every pair of selected modifiers. Example: Shift+Ctrl+U, Shift+Alt+U, Ctrl+Alt+U.",
            "每个允许按键都会与已勾选修饰键中的任意两键组合绑定。例如：Shift+Ctrl+U、Shift+Alt+U、Ctrl+Alt+U。",
        ));
        ui.add_space(12.0);
        ui.horizontal(|ui| {
            reset = ui
                .button(tr(english, "Restore defaults", "恢复默认"))
                .clicked();
            ui.add_space(12.0);
            egui::ComboBox::from_id_salt("suffix_presets")
                .selected_text(tr(english, "Common key ranges", "常用按键"))
                .show_ui(ui, |ui| {
                    for (label, value) in [
                        ("a – z", "abcdefghijklmnopqrstuvwxyz"),
                        ("9 0 - =", "9 0 - ="),
                        ("- = ;", "- = ;"),
                        ("0 – 9", "0 1 2 3 4 5 6 7 8 9"),
                        ("F1 – F4", "F1 F2 F3 F4"),
                    ] {
                        if ui.selectable_label(false, label).clicked() {
                            self.dynamic_suffix_input = value.into();
                        }
                    }
                });
        });
        if reset {
            self.dynamic_range_draft = AppState::default();
            self.dynamic_suffix_input = suffix_input(&self.dynamic_range_draft.dynamic_suffixes);
            self.dynamic_config_error.clear();
        }
        if cancel {
            self.dynamic_range_draft = self.state.clone();
            self.dynamic_suffix_input = suffix_input(&self.state.dynamic_suffixes);
            self.dynamic_config_error.clear();
        }
        ui.add_space(16.0);
        if dirty && !reset && !cancel {
            match parse_suffix_input(&self.dynamic_suffix_input) {
                Ok(suffixes) => {
                    let mut proposed = self.state.clone();
                    proposed.dynamic_suffixes = suffixes;
                    proposed.dynamic_toggle_modifiers =
                        self.dynamic_range_draft.dynamic_toggle_modifiers.clone();
                    self.dynamic_config_error =
                        plan_dynamic_hotkeys(&proposed).err().unwrap_or_default();
                }
                Err(error) => self.dynamic_config_error = error,
            }
        }
        if !self.dynamic_config_error.is_empty() {
            ui.colored_label(
                egui::Color32::from_rgb(180, 64, 52),
                &self.dynamic_config_error,
            );
        }
        if apply && (self.action_in_flight || self.dynamic_toggle_in_flight) {
            self.state.status = "请等待当前窗口操作完成。".into();
        } else if apply && self.dynamic_config_error.is_empty() {
            let previous = (
                self.state.dynamic_toggle_modifiers.clone(),
                self.state.dynamic_suffixes.clone(),
            );
            self.state.dynamic_toggle_modifiers =
                self.dynamic_range_draft.dynamic_toggle_modifiers.clone();
            self.state.dynamic_suffixes =
                parse_suffix_input(&self.dynamic_suffix_input).unwrap_or_default();
            if !self.state.bind_mode.is_dynamic() {
                self.persist();
                self.state.status =
                    "Shortcut range saved. Select a dynamic mode to activate it.".into();
                self.dynamic_range_draft = self.state.clone();
                self.dynamic_suffix_input = suffix_input(&self.state.dynamic_suffixes);
                self.dynamic_config_error.clear();
            } else if !self.register_dynamic_hotkeys() {
                self.dynamic_config_error = localized_status(english, &self.state.status);
                (
                    self.state.dynamic_toggle_modifiers,
                    self.state.dynamic_suffixes,
                ) = previous;
            } else {
                self.dynamic_range_draft = self.state.clone();
                self.dynamic_suffix_input = suffix_input(&self.state.dynamic_suffixes);
                self.dynamic_config_error.clear();
                if self.state.status == "Dynamic shortcut ranges registered." {
                    self.state.status.clear();
                }
            }
        }
        if !show_bindings {
            return;
        }
        ui.add_space(8.0);
        ui.separator();
        ui.add_space(12.0);
        ui.heading(tr(english, "Current bindings", "当前绑定"));
        ui.add_space(12.0);
        let suffixes: Vec<_> = self
            .state
            .dynamic_groups
            .iter()
            .filter(|(_, group)| is_bound_in_session(group))
            .map(|(key, _)| key.clone())
            .collect();
        let mut toggle = None;
        if suffixes.is_empty() {
            ui.weak(tr(
                english,
                "No windows bound this session",
                "本次运行尚未绑定窗口",
            ));
        }
        egui::ScrollArea::vertical().show(ui, |ui| {
            egui::Grid::new("dynamic_slots")
                .num_columns(3)
                .striped(true)
                .min_row_height(44.0)
                .spacing([24.0, 10.0])
                .show(ui, |ui| {
                    ui.strong(tr(english, "Shortcut", "快捷键"));
                    ui.strong(tr(english, "Window / group", "窗口 / 窗口组"));
                    ui.label("");
                    ui.end_row();
                    for suffix in suffixes {
                        {
                            let role = self
                                .dynamic_hotkey_roles
                                .values()
                                .find(|role| role.suffix == suffix);
                            ui.label(
                                egui::RichText::new(if role.is_some() {
                                    readable_chord(&suffix, cfg!(target_os = "macos"))
                                } else {
                                    tr(english, "Disabled", "已停用").into()
                                })
                                .strong(),
                            );
                        }
                        if let Some(group) = self.state.dynamic_groups.get(&suffix) {
                            ui.vertical(|ui| {
                                ui.set_max_width(280.0);
                                ui.strong(&group.name);
                                for id in &group.window_ids {
                                    let window = self.windows.iter().find(|w| &w.id == id);
                                    ui.label(
                                        window
                                            .map(|w| format!("{} · {}", w.app, w.title))
                                            .unwrap_or_else(|| {
                                                tr(english, "Window closed", "窗口已关闭").into()
                                            }),
                                    );
                                }
                            });
                            if ui
                                .add_enabled(
                                    !self.action_in_flight && !self.dynamic_toggle_in_flight,
                                    egui::Button::new(if group.is_visible {
                                        tr(english, "Hide", "隐藏")
                                    } else {
                                        tr(english, "Show", "显示")
                                    }),
                                )
                                .clicked()
                            {
                                toggle = Some(suffix);
                            }
                        } else {
                            ui.weak(tr(english, "Unbound", "未绑定"));
                            ui.label("");
                        }
                        ui.end_row();
                    }
                });
        });
        if let Some(suffix) = toggle {
            // UI recovery remains available even for a key removed from the range.
            if let Some(group) = self.state.dynamic_groups.get_mut(&suffix) {
                let actions = slot_actions(group, &self.windows);
                self.apply_actions(actions);
                self.persist();
            }
        }
    }

    fn sync_preview_textures(&mut self, ctx: &egui::Context) {
        for window in &self.windows {
            let Some(preview) = &window.preview else {
                continue;
            };
            if self.preview_pixels.get(&window.id) == Some(preview) {
                continue;
            }
            let image = egui::ColorImage::from_rgba_premultiplied(
                [preview.width, preview.height],
                &preview.rgba,
            );
            let texture = ctx.load_texture(
                format!("window-preview-{}", window.id),
                image,
                egui::TextureOptions::LINEAR,
            );
            self.preview_pixels
                .insert(window.id.clone(), preview.clone());
            self.preview_textures.insert(window.id.clone(), texture);
        }
        let live_ids: HashSet<_> = self
            .windows
            .iter()
            .filter(|window| window.preview.is_some())
            .map(|window| window.id.as_str())
            .collect();
        self.preview_textures
            .retain(|window_id, _| live_ids.contains(window_id.as_str()));
        self.preview_pixels
            .retain(|window_id, _| live_ids.contains(window_id.as_str()));
    }
}

impl eframe::App for VeilKeyApp {
    fn on_exit(&mut self, _gl: Option<&eframe::glow::Context>) {
        if let Ok(windows) = NativeWindowController::enumerate_fast() {
            self.windows = windows;
        }
        let actions = restore_session_groups(
            self.state
                .groups
                .iter_mut()
                .chain(self.state.dynamic_groups.values_mut())
                .chain(self.dynamic_recovery.iter_mut()),
            &self.windows,
        );
        if let Err(error) = NativeWindowController::apply_actions(actions) {
            eprintln!("Could not restore all windows while exiting: {error}");
        }
        self.state.dynamic_groups.clear();
        self.persist();
    }

    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        #[cfg(any(target_os = "macos", target_os = "windows"))]
        self.process_tray_commands(ctx);
        #[cfg(any(target_os = "macos", target_os = "windows"))]
        if ctx.input(|input| input.viewport().close_requested())
            && self.tray_icon.is_some()
            && !self.on_exit_requested()
        {
            ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose);
            ctx.send_viewport_cmd(egui::ViewportCommand::Visible(false));
        }
        self.update_window_snapshot();
        self.update_window_actions();
        self.update_capture_listener_status();
        self.retry_hotkeys_if_needed();
        self.process_global_binding_capture(self.state.english_ui);
        self.complete_focused_window_bindings(self.state.english_ui);
        self.capture_hotkey(ctx);
        self.handle_hotkey_events();
        self.process_dynamic_toggle_results(self.state.english_ui);
        self.sync_preview_textures(ctx);
        ctx.request_repaint_after(Duration::from_millis(100));
        let english = self.state.english_ui;
        let displayable_count = self
            .windows
            .iter()
            .filter(|w| window_is_displayable(w))
            .count();

        egui::TopBottomPanel::top("header").show(ctx, |ui| {
            ui.add_space(8.0);
            ui.horizontal(|ui| {
                ui.heading("VeilKey");
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui.button(tr(english, "中文", "English")).clicked() {
                        self.state.english_ui = !self.state.english_ui;
                        self.persist();
                    }
                    if ui.button(tr(english, "Refresh", "刷新窗口")).clicked() {
                        request_window_refresh(&self.refresh_tx, true);
                    }
                    ui.label(if english {
                        format!("{displayable_count} windows")
                    } else {
                        format!("{displayable_count} 个窗口")
                    });
                });
            });
            ui.add_space(8.0);
        });

        egui::CentralPanel::default()
            .frame(egui::Frame::central_panel(&ctx.style()).inner_margin(24.0))
            .show(ctx, |ui| {
            egui::ScrollArea::vertical().show(ui, |ui| {
            let old_mode = self.state.bind_mode;
            ui.horizontal(|ui| {
                for (mode, en, zh) in [
                    (BindMode::OneWindowAtATime, "Single window", "单窗口"),
                    (BindMode::ActiveApplication, "Application windows", "同应用窗口"),
                    (BindMode::SelectFromList, "Window combination", "多窗口组合"),
                ] {
                    if ui.add_sized([144.0, 36.0], egui::Button::new(tr(english, en, zh))
                        .selected(self.state.bind_mode == mode)).clicked() {
                        self.state.bind_mode = mode;
                    }
                }
            });
            if self.state.bind_mode != old_mode {
                if self.dynamic_toggle_in_flight || self.action_in_flight {
                    self.state.bind_mode = old_mode;
                    self.state.status = "请等待当前窗口操作完成。".into();
                } else if self.register_dynamic_hotkeys() {
                    self.recording_group = None;
                    self.pending_bind_group = None;
                    self.capture_armed.store(false, Ordering::Release);
                    self.persist();
                } else {
                    self.state.bind_mode = old_mode;
                }
            }
            ui.add_space(12.0);
            ui.separator();
            if !self.state.status.is_empty() {
                ui.horizontal(|ui| {
                    ui.label(
                        egui::RichText::new(localized_status(english, &self.state.status))
                            .color(ui.visuals().text_color()),
                    );
                    if ui.small_button("×").clicked() {
                        self.state.status.clear();
                    }
                });
                ui.add_space(4.0);
            }
            self.dynamic_ui(ui, english, self.state.bind_mode.is_dynamic());
            if self.state.bind_mode.is_dynamic() {
                return;
            }
            ui.columns(2, |columns| {
                let ui = &mut columns[0];
                ui.heading(tr(english, "Window groups", "窗口组"));
                ui.add_space(8.0);
                if self.state.groups.is_empty() {
                    ui.label(
                        egui::RichText::new(tr(english, "No groups yet", "还没有窗口组")).strong(),
                    );
                    ui.label(tr(
                        english,
                        "Select windows on the right to get started.",
                        "先在右侧选择要绑定的窗口。",
                    ));
                }
                let mut toggle = None;
                let mut delete = None;
                let mut hotkey_save = None;
                let mut group_option_changed = false;
                for (index, group) in self.state.groups.iter_mut().enumerate() {
                    ui.separator();
                    ui.horizontal(|ui| {
                        ui.vertical(|ui| {
                            ui.label(egui::RichText::new(&group.name).strong());
                            ui.label(egui::RichText::new(group.hotkey.as_deref()
                                .unwrap_or(tr(english, "Unassigned", "未绑定快捷键"))).monospace());
                            for id in &group.window_ids {
                                ui.label(self.windows.iter().find(|w| &w.id == id)
                                    .map(|w| format!("{} · {}", w.app, w.title))
                                    .unwrap_or_else(|| id.split('\0').take(2)
                                        .collect::<Vec<_>>().join(" · ")));
                            }
                            ui.label(format!(
                                "{} {} · {}",
                                group.window_ids.len(),
                                tr(english, "windows", "个窗口"),
                                if english {
                                    if group.is_visible { "shown" } else { "hidden" }
                                } else if group.is_visible {
                                    "显示中"
                                } else {
                                    "已隐藏"
                                }
                            ));
                        });
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            if ui.small_button(tr(english, "Remove", "移除")).clicked() {
                                delete = Some(index);
                            }
                            if ui
                                .button(if group.is_visible {
                                    tr(english, "Hide", "隐藏")
                                } else {
                                    tr(english, "Show", "显示")
                                })
                                .clicked()
                            {
                                toggle = Some(index);
                            }
                        });
                    });
                    ui.horizontal(|ui| {
                        ui.label(tr(english, "Shortcut", "快捷键"));
                        ui.add(
                            egui::TextEdit::singleline(&mut group.hotkey_draft)
                                .hint_text(tr(
                                    english,
                                    "e.g. Ctrl + Alt + H",
                                    "例如 Ctrl + Alt + H",
                                ))
                                .desired_width(150.0),
                        );
                        if ui.small_button(tr(english, "Save", "保存")).clicked() {
                            hotkey_save = Some((index, group.hotkey_draft.clone()));
                        }
                        if ui
                            .small_button(if self.recording_group.as_deref() == Some(&group.name) {
                                tr(english, "Press keys…", "请按快捷键…")
                            } else {
                                tr(english, "Record", "录制")
                            })
                            .clicked()
                        {
                            self.recording_group = Some(group.name.clone());
                            self.state.status =
                                tr(english, "Press the shortcut now.", "请按下要绑定的快捷键。")
                                    .into();
                        }
                    });
                    let force_popup = ui.add_enabled(
                        cfg!(target_os = "windows"),
                        egui::Checkbox::new(
                            &mut group.force_popup,
                            tr(english, "Keep above other windows", "显示在其他窗口上方"),
                        ),
                    );
                    let force_popup_changed = force_popup.changed();
                    force_popup.on_hover_text(tr(
                        english,
                        "Windows only. Keeps the group above normal windows without focusing it.",
                        "仅 Windows 可用；窗口置顶但不抢占焦点。",
                    ));
                    if force_popup_changed {
                        group_option_changed = true;
                    }
                    let hide_application = ui.add_enabled(
                        cfg!(target_os = "macos") && group.is_visible,
                        egui::Checkbox::new(
                            &mut group.macos_hide_application,
                            tr(
                                english,
                            "Hide whole app instead of minimizing (Dock icon stays)",
                                "隐藏整个应用、不最小化窗口（保留程序坞图标）",
                            ),
                        ),
                    );
                    let hide_application_changed = hide_application.changed();
                    hide_application.on_hover_text(tr(
                        english,
                        "macOS only. Hides all windows of each application. App icons remain in the Dock. Change this option while the group is shown.",
                        "仅 macOS。隐藏每个应用的全部窗口；应用图标仍保留在程序坞。请在窗口组显示时调整。",
                    ));
                    if hide_application_changed {
                        group_option_changed = true;
                    }
                }
                if let Some(index) = delete {
                    if let Some(group) = self.state.groups.get(index)
                        && let Some(hotkey) = group
                            .hotkey
                            .as_deref()
                            .and_then(|s| HotKey::from_str(s).ok())
                        && let Some(manager) = &self.hotkeys
                    {
                        let _ = manager.unregister(hotkey);
                    }
                    self.state.groups.remove(index);
                    self.persist();
                }
                if let Some((index, value)) = hotkey_save {
                    self.register_group_hotkey(index, value);
                }
                if group_option_changed {
                    self.persist();
                }
                if let Some(index) = toggle {
                    self.toggle_group(index);
                }

                let ui = &mut columns[1];
                ui.heading(tr(english, "Available windows", "可绑定窗口"));
                ui.label(tr(
                    english,
                    "Only visible windows with a usable preview are listed.",
                    "这里只显示当前可见且有实际尺寸的窗口。",
                ));
                #[cfg(target_os = "macos")]
                if !NativeWindowController::screen_capture_authorized() {
                    ui.horizontal(|ui| {
                        ui.colored_label(
                            egui::Color32::from_rgb(210, 132, 66),
                            tr(
                                english,
                                "Window previews require Screen Recording permission.",
                                "窗口预览需要“屏幕录制”权限。",
                            ),
                        );
                        if ui
                            .button(tr(english, "Grant permission", "请求授权"))
                            .clicked()
                        {
                            let granted = NativeWindowController::request_screen_capture_access();
                            self.state.status = if granted {
                                tr(
                                    english,
                                    "Screen Recording permission granted. Restart VeilKey to load previews.",
                                    "已获得屏幕录制权限，请重启 VeilKey 以加载预览图。",
                                )
                            } else {
                                tr(
                                    english,
                                    "Allow VeilKey under Privacy & Security → Screen Recording, then restart it.",
                                    "请在“隐私与安全性 → 屏幕录制”中允许 VeilKey，然后重启应用。",
                                )
                            }
                            .into();
                        }
                    });
                }
                ui.add_space(8.0);
                ui.horizontal(|ui| {
                    ui.label(tr(english, "Name", "组名"));
                    ui.add(
                        egui::TextEdit::singleline(&mut self.state.new_group_name)
                            .hint_text(tr(english, "e.g. MuMu set", "例如：MuMu 多开"))
                            .desired_width(190.0),
                    );
                });
                ui.add_space(6.0);
                if self.state.bind_mode == BindMode::SelectFromList
                    && ui
                        .button(tr(english, "Create group", "创建窗口组"))
                        .clicked()
                {
                    if self
                        .state
                        .create_group(self.state.new_group_name.trim().to_owned())
                    {
                        self.state.status = tr(
                            english,
                            "Group created. Record a shortcut to use it anywhere.",
                            "窗口组已创建，录制快捷键即可随时切换。",
                        )
                        .into();
                        self.persist();
                    } else {
                        self.state.status = tr(
                            english,
                            "Add a name and select at least one window.",
                            "请填写组名并至少选择一个窗口。",
                        )
                        .into();
                    }
                }
                ui.separator();
                ui.horizontal(|ui| {
                    ui.label(tr(english, "Search", "搜索"));
                    ui.add(
                        egui::TextEdit::singleline(&mut self.filter)
                            .hint_text(tr(english, "App or window name", "应用或窗口名称"))
                            .desired_width(180.0),
                    );
                });
                let mut quick_bind = None;
                egui::ScrollArea::vertical().show(ui, |ui| {
                    for window in &self.windows {
                        if !window_is_displayable(window) {
                            continue;
                        }
                        let text = format!("{} {}", window.app, window.title);
                        if !self.filter.is_empty()
                            && !text.to_lowercase().contains(&self.filter.to_lowercase())
                        {
                            continue;
                        }
                        egui::Frame::group(ui.style()).show(ui, |ui| {
                            ui.horizontal(|ui| {
                                let mut selected = self.state.selected_windows.contains(&window.id);
                                if self.state.bind_mode == BindMode::SelectFromList
                                    && ui.checkbox(&mut selected, "").changed()
                                {
                                    if selected {
                                        self.state.selected_windows.insert(window.id.clone());
                                    } else {
                                        self.state.selected_windows.remove(&window.id);
                                    }
                                }
                                ui.vertical(|ui| {
                                    if let Some(texture) = self.preview_textures.get(&window.id) {
                                        ui.add(
                                            egui::Image::new(texture)
                                                .max_size(egui::vec2(144.0, 100.0)),
                                        );
                                    }
                                    ui.label(egui::RichText::new(&window.title).strong());
                                    ui.label(format!(
                                        "{} · {}×{}",
                                        window.app, window.rect.width, window.rect.height
                                    ));
                                });
                                if self.state.bind_mode == BindMode::OneWindowAtATime
                                    && ui
                                        .small_button(tr(english, "Bind this window", "绑定此窗口"))
                                        .clicked()
                                {
                                    quick_bind = Some((window.id.clone(), window.title.clone()));
                                }
                            });
                        });
                    }
                });
                if let Some((window_id, title)) = quick_bind {
                    let mut name = title.clone();
                    let mut suffix = 2;
                    while self
                        .state
                        .groups
                        .iter()
                        .any(|group| group.name.eq_ignore_ascii_case(&name))
                    {
                        name = format!("{title} {suffix}");
                        suffix += 1;
                    }
                    self.state.selected_windows.insert(window_id);
                    if self.state.create_group(name.clone()) {
                        self.recording_group = None;
                        self.pending_bind_group = Some(name);
                        self.capture_armed.store(true, Ordering::Release);
                        self.state.status = tr(
                            english,
                            "Focus that window and press its shortcut.",
                            "切到该窗口并按下要绑定的快捷键。",
                        )
                        .into();
                        self.persist();
                    }
                }
            });
            });
        });
    }
}

fn main() -> eframe::Result {
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([1000.0, 680.0])
            .with_min_inner_size([780.0, 520.0]),
        #[cfg(target_os = "macos")]
        event_loop_builder: Some(Box::new(|builder| {
            use winit::platform::macos::{ActivationPolicy, EventLoopBuilderExtMacOS};
            builder.with_activation_policy(ActivationPolicy::Accessory);
        })),
        ..Default::default()
    };
    let state_file = state_path();
    let lock_path = state_file.with_file_name("instance.lock");
    let instance_lock = match acquire_instance_lock(&lock_path) {
        Ok(lock) => lock,
        Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
            eprintln!("VeilKey is already running; using the existing instance.");
            return Ok(());
        }
        Err(error) => {
            eprintln!("Could not acquire the VeilKey instance lock: {error}");
            return Ok(());
        }
    };
    eframe::run_native(
        "VeilKey",
        options,
        Box::new(move |cc| {
            configure_ui(&cc.egui_ctx);
            #[cfg(target_os = "macos")]
            configure_macos_cjk_fonts(&cc.egui_ctx);
            #[cfg(target_os = "windows")]
            configure_windows_cjk_fonts(&cc.egui_ctx);
            Ok(Box::new(VeilKeyApp::new(instance_lock, &cc.egui_ctx)))
        }),
    )
}

fn configure_ui(ctx: &egui::Context) {
    let mut style = (*ctx.style()).clone();
    style.spacing.item_spacing = egui::vec2(12.0, 10.0);
    style.spacing.button_padding = egui::vec2(12.0, 7.0);
    style.spacing.interact_size.y = 32.0;
    for (text_style, size) in [
        (egui::TextStyle::Body, 16.0),
        (egui::TextStyle::Button, 15.0),
        (egui::TextStyle::Small, 13.0),
        (egui::TextStyle::Monospace, 15.0),
        (egui::TextStyle::Heading, 20.0),
    ] {
        style
            .text_styles
            .insert(text_style, egui::FontId::proportional(size));
    }
    ctx.set_style(style);
}

#[cfg(target_os = "macos")]
fn configure_macos_cjk_fonts(ctx: &egui::Context) {
    let candidates = [
        "/System/Library/Fonts/Hiragino Sans GB.ttc",
        "/System/Library/Fonts/PingFang.ttc",
        "/System/Library/Fonts/STHeiti Medium.ttc",
    ];
    let Some((path, bytes)) = candidates
        .iter()
        .find_map(|path| std::fs::read(path).ok().map(|bytes| (*path, bytes)))
    else {
        eprintln!("No supported macOS Chinese font was found; Chinese text may be missing.");
        return;
    };

    let mut fonts = egui::FontDefinitions::default();
    fonts.font_data.insert(
        "macos_cjk".into(),
        std::sync::Arc::new(egui::FontData::from_owned(bytes)),
    );
    for family in [egui::FontFamily::Proportional, egui::FontFamily::Monospace] {
        fonts
            .families
            .entry(family)
            .or_default()
            .push("macos_cjk".into());
    }
    eprintln!("Using macOS Chinese font: {path}");
    ctx.set_fonts(fonts);
}

#[cfg(target_os = "windows")]
fn configure_windows_cjk_fonts(ctx: &egui::Context) {
    let windows_dir = std::env::var_os("WINDIR")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| std::path::PathBuf::from(r"C:\Windows"));
    let candidates = [
        windows_dir.join(r"Fonts\msyh.ttc"),
        windows_dir.join(r"Fonts\msyh.ttf"),
        windows_dir.join(r"Fonts\simhei.ttf"),
    ];
    let Some((path, bytes)) = candidates
        .iter()
        .find_map(|path| std::fs::read(path).ok().map(|bytes| (path, bytes)))
    else {
        eprintln!("No supported Windows Chinese font was found; Chinese text may be missing.");
        return;
    };
    let mut fonts = egui::FontDefinitions::default();
    fonts.font_data.insert(
        "windows_cjk".into(),
        std::sync::Arc::new(egui::FontData::from_owned(bytes)),
    );
    for family in [egui::FontFamily::Proportional, egui::FontFamily::Monospace] {
        // Make sure Windows' CJK font is tried before egui's Latin-only default
        // fonts. Keeping the defaults after it still provides glyph fallback
        // for symbols that are not present in the selected system font.
        let family_fonts = fonts.families.entry(family).or_default();
        family_fonts.insert(0, "windows_cjk".into());
    }
    eprintln!("Using Windows Chinese font: {}", path.display());
    ctx.set_fonts(fonts);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn suffix_form_accepts_compact_reference_format_and_function_keys() {
        assert_eq!(
            parse_suffix_input("90-=").unwrap(),
            ["Digit9", "Digit0", "Minus", "Equal"]
        );
        assert_eq!(
            parse_suffix_input("- = ;").unwrap(),
            ["Minus", "Equal", "Semicolon"]
        );
        assert_eq!(
            parse_suffix_input("a B, F1 f12，0 a").unwrap(),
            ["KeyA", "KeyB", "F1", "F12", "Digit0"]
        );
        for invalid in ["", "   ", "中文", "F13", "9 /"] {
            assert!(parse_suffix_input(invalid).is_err(), "{invalid}");
        }
        assert_eq!(suffix_input(&["KeyU".into()]), "u");
    }

    #[test]
    fn only_bound_groups_appear_in_the_session_binding_list() {
        let mut bound = group("Bound", "super+KeyU");
        bound.window_ids.insert("window-1".into());
        let unbound = group("Empty slot", "super+KeyI");
        assert!(is_bound_in_session(&bound));
        assert!(!is_bound_in_session(&unbound));
    }

    #[test]
    fn shortcut_labels_use_platform_names_and_round_trip_suffixes() {
        let keys = vec![
            "Digit9".into(),
            "Digit0".into(),
            "Minus".into(),
            "F12".into(),
        ];
        assert_eq!(parse_suffix_input(&suffix_input(&keys)).unwrap(), keys);
        let modifiers = vec!["super".into(), "shift".into()];
        assert_eq!(
            readable_shortcut(&modifiers, "Digit9", true),
            "Command + Shift + 9"
        );
        assert_eq!(
            readable_shortcut(&modifiers, "Digit9", false),
            "Win + Shift + 9"
        );
        assert_eq!(
            readable_shortcut(&["super".into()], "KeyU", true),
            "Command + u"
        );
        let modifiers = vec!["ctrl".into(), "alt".into()];
        assert_eq!(
            readable_shortcut(&modifiers, "F12", true),
            "Control + Option + F12"
        );
        assert_eq!(
            readable_shortcut(&modifiers, "F12", false),
            "Ctrl + Alt + F12"
        );
    }

    fn group(name: &str, hotkey: &str) -> Group {
        Group {
            name: name.into(),
            window_ids: Default::default(),
            hotkey: Some(hotkey.into()),
            hotkey_draft: String::new(),
            is_visible: true,
            force_popup: false,
            macos_hide_application: false,
            restore_rects: Default::default(),
        }
    }

    #[test]
    fn shortcut_recovery_only_retries_unregistered_bindings() {
        let active = HotKey::from_str("ctrl+alt+KeyH").unwrap();
        let groups = [
            group("Already active", "ctrl+alt+KeyH"),
            group("Needs retry", "ctrl+alt+KeyJ"),
        ];
        let pending = pending_hotkeys(&groups, &HashSet::from([active.id]));
        assert_eq!(pending.len(), 1);
        assert_eq!(pending[0].0, "Needs retry");
    }

    #[test]
    fn capture_listener_retry_uses_capped_exponential_backoff() {
        assert_eq!(capture_retry_delay(0), Duration::from_secs(2));
        assert_eq!(capture_retry_delay(1), Duration::from_secs(2));
        assert_eq!(capture_retry_delay(2), Duration::from_secs(4));
        assert_eq!(capture_retry_delay(3), Duration::from_secs(8));
        assert_eq!(capture_retry_delay(6), Duration::from_secs(60));
        assert_eq!(capture_retry_delay(20), Duration::from_secs(60));
        let mut failures = 8;
        assert_eq!(
            capture_retry_delay_after_failure(&mut failures, Duration::from_secs(299)),
            Duration::from_secs(60)
        );
        assert_eq!(failures, 9);
        assert_eq!(
            capture_retry_delay_after_failure(&mut failures, Duration::from_secs(300)),
            Duration::from_secs(2)
        );
        assert_eq!(failures, 1);
    }

    #[test]
    fn window_refresh_requests_are_coalesced_when_the_worker_is_busy() {
        let (sender, receiver) = mpsc::sync_channel(1);
        request_window_refresh(&sender, false);
        request_window_refresh(&sender, true);
        request_window_refresh(&sender, true);
        assert!(receiver.try_recv().is_ok());
        assert!(receiver.try_recv().is_err());
    }

    #[test]
    fn adjacent_duplicate_toggles_cancel_without_reordering_overlapping_groups() {
        let mut queue = VecDeque::new();
        assert!(enqueue_toggle(&mut queue, "A", 3));
        assert!(enqueue_toggle(&mut queue, "B", 3));
        assert!(enqueue_toggle(&mut queue, "A", 3));
        assert_eq!(queue, VecDeque::from(["A", "B", "A"]));
        assert!(!enqueue_toggle(&mut queue, "C", 3));
        assert!(enqueue_toggle(&mut queue, "A", 3));
        assert_eq!(queue, VecDeque::from(["A", "B"]));
    }

    #[test]
    fn pending_shortcut_recording_tracks_group_identity_after_deletion() {
        let groups = [
            group("First", "ctrl+alt+KeyA"),
            group("Target", "ctrl+alt+KeyB"),
        ];
        assert_eq!(group_index_by_name(&groups, "Target"), Some(1));
        let groups_after_delete = [group("Target", "ctrl+alt+KeyB")];
        assert_eq!(group_index_by_name(&groups_after_delete, "Target"), Some(0));
        assert_eq!(group_index_by_name(&groups_after_delete, "Deleted"), None);
    }

    #[test]
    fn selected_modifiers_form_every_pair_per_suffix() {
        let state = AppState {
            bind_mode: BindMode::OneWindowAtATime,
            ..Default::default()
        };
        let roles = plan_dynamic_hotkeys(&state).unwrap();
        assert_eq!(roles.len(), 4);
        for suffix in &state.dynamic_suffixes {
            let roles: Vec<_> = roles.iter().filter(|role| &role.suffix == suffix).collect();
            assert_eq!(roles.len(), 1);
        }
        let shift_combo = AppState {
            dynamic_toggle_modifiers: vec!["super".into(), "shift".into()],
            dynamic_suffixes: vec!["KeyU".into()],
            bind_mode: BindMode::OneWindowAtATime,
            ..Default::default()
        };
        let roles = plan_dynamic_hotkeys(&shift_combo).unwrap();
        assert_eq!(roles.len(), 1);
        assert_eq!(
            roles[0].hotkey,
            HotKey::from_str("super+shift+KeyU").unwrap()
        );
        let all_modifiers = AppState {
            dynamic_toggle_modifiers: vec![
                "super".into(),
                "ctrl".into(),
                "alt".into(),
                "shift".into(),
            ],
            dynamic_suffixes: vec!["KeyU".into(), "KeyI".into(), "KeyO".into()],
            bind_mode: BindMode::OneWindowAtATime,
            ..Default::default()
        };
        let roles = plan_dynamic_hotkeys(&all_modifiers).unwrap();
        assert_eq!(roles.len(), 18);
        assert_eq!(roles.iter().filter(|role| role.suffix == "KeyU").count(), 6);
    }

    #[test]
    fn dynamic_hotkey_uses_each_selected_modifier_pair() {
        let state = AppState {
            bind_mode: BindMode::OneWindowAtATime,
            dynamic_toggle_modifiers: vec!["super".into(), "shift".into()],
            dynamic_suffixes: vec!["KeyU".into()],
            ..Default::default()
        };
        let roles = plan_dynamic_hotkeys(&state).unwrap();
        assert_eq!(roles.len(), 1);
        assert_eq!(
            roles[0].hotkey,
            HotKey::from_str("super+shift+KeyU").unwrap()
        );
    }

    #[test]
    fn shortcut_range_rejects_overlap_duplicates_empty_keys_and_manual_conflicts() {
        let mut state = AppState {
            bind_mode: BindMode::ActiveApplication,
            ..Default::default()
        };
        assert!(plan_dynamic_hotkeys(&state).is_ok());
        state.dynamic_toggle_modifiers.push("shift".into());
        let combined = plan_dynamic_hotkeys(&state).unwrap();
        assert_eq!(combined.len(), state.dynamic_suffixes.len() * 3);
        assert!(combined.iter().any(|role| {
            role.hotkey
                .mods
                .contains(global_hotkey::hotkey::Modifiers::SHIFT)
        }));
        state.dynamic_suffixes = vec![];
        assert!(plan_dynamic_hotkeys(&state).is_err());
        state.dynamic_suffixes = vec!["Digit9".into()];
        state.groups.push(group("Conflicting", "ctrl+shift+Digit9"));
        assert!(plan_dynamic_hotkeys(&state).is_err());
        state.groups.clear();
        state.dynamic_toggle_modifiers.clear();
        assert!(plan_dynamic_hotkeys(&state).is_err());
    }

    #[test]
    fn stored_slot_toggles_without_any_foreground_target() {
        let mut first = group("First", "super+Digit9");
        first.window_ids.insert("first".into());
        first.is_visible = false;
        let windows = [WindowInfo {
            id: "first".into(),
            title: "First".into(),
            app: "Example".into(),
            rect: Default::default(),
            visible: false,
            native_handle: 42,
            process_id: 10,
            preview: None,
        }];
        assert!(!should_bind_slot(Some(&first), Some("other-app"), &windows));
        let actions = slot_actions(&mut first, &windows);
        assert!(matches!(
            actions.as_slice(),
            [WindowAction::SetVisible {
                visible: true,
                handle: 42,
                ..
            }]
        ));
        assert!(first.is_visible);
        let visible_windows = [WindowInfo {
            visible: true,
            ..windows[0].clone()
        }];
        assert!(!should_bind_slot(
            Some(&first),
            Some("first"),
            &visible_windows
        ));
        assert!(should_bind_slot(
            Some(&first),
            Some("other-app"),
            &visible_windows
        ));
        assert!(should_bind_slot(None, Some("first"), &visible_windows));
    }

    #[test]
    fn replacing_slot_only_restores_hidden_old_windows() {
        let mut old = group("Old target", "super+Digit9");
        old.window_ids.extend(["hidden".into(), "visible".into()]);
        let windows = [false, true].map(|visible| WindowInfo {
            id: if visible { "visible" } else { "hidden" }.into(),
            title: String::new(),
            app: String::new(),
            rect: Default::default(),
            visible,
            native_handle: if visible { 2 } else { 1 },
            process_id: 10,
            preview: None,
        });
        assert!(matches!(
            restore_group_actions(&old, &windows).as_slice(),
            [WindowAction::SetVisible {
                visible: true,
                handle: 1,
                ..
            }]
        ));
    }

    #[test]
    fn session_exit_restores_hidden_windows_and_clears_group_hidden_state() {
        let mut hidden = group("Hidden target", "super+Digit9");
        hidden.window_ids.insert("hidden-window".into());
        hidden.is_visible = false;
        let windows = [WindowInfo {
            id: "hidden-window".into(),
            title: "Hidden target".into(),
            app: "Example".into(),
            rect: Default::default(),
            visible: false,
            native_handle: 77,
            process_id: 10,
            preview: None,
        }];

        let actions = restore_session_groups([&mut hidden].into_iter(), &windows);

        assert!(matches!(
            actions.as_slice(),
            [WindowAction::SetVisible {
                visible: true,
                handle: 77,
                ..
            }]
        ));
        assert!(hidden.is_visible);
    }

    #[test]
    fn successful_unrelated_action_keeps_failed_rebind_recovery() {
        let mut old = group("Failed restoration", "super+Digit9");
        old.window_ids.insert("old".into());
        old.is_visible = false;
        let mut recovery = vec![old];
        let mut windows = vec![WindowInfo {
            id: "old".into(),
            title: String::new(),
            app: String::new(),
            rect: Default::default(),
            visible: false,
            native_handle: 1,
            process_id: 1,
            preview: None,
        }];
        retain_unrestored_targets(&mut recovery, &windows);
        assert_eq!(recovery.len(), 1);
        windows[0].visible = true;
        retain_unrestored_targets(&mut recovery, &windows);
        assert!(recovery.is_empty());
    }
}
