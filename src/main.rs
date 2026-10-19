use eframe::egui;
use global_hotkey::hotkey::HotKey;
use global_hotkey::{GlobalHotKeyEvent, GlobalHotKeyManager, HotKeyState};
use std::collections::HashSet;
use std::collections::VecDeque;
use std::fs::File;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::str::FromStr;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::{self, Receiver};
use std::thread;
use std::time::{Duration, Instant};
use veilkey::platform::NativeWindowController;
use veilkey::{
    AppState, BindMode, Group, WindowAction, WindowInfo, acquire_instance_lock, load_state,
    save_state, state_path, window_is_displayable,
};

fn tr<'a>(english: bool, en: &'a str, zh: &'a str) -> &'a str {
    if english { en } else { zh }
}

fn localized_status(english: bool, status: &str) -> String {
    if english {
        return status.to_owned();
    }
    match status {
        "Shortcut registered." => "快捷键已绑定。".into(),
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
        _ if status.starts_with("Some group window actions failed:") => {
            format!(
                "部分窗口操作失败：{}",
                status.trim_start_matches("Some group window actions failed:")
            )
        }
        _ if status.starts_with("Window operation failed unexpectedly:") => {
            format!(
                "窗口操作遇到意外错误，服务已恢复：{}",
                status.trim_start_matches("Window operation failed unexpectedly:")
            )
        }
        _ => status.to_owned(),
    }
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

fn request_window_refresh(sender: &mpsc::SyncSender<()>) {
    let _ = sender.try_send(());
}

fn snapshot_is_current(snapshot_generation: u64, current_generation: u64) -> bool {
    snapshot_generation == current_generation && current_generation.is_multiple_of(2)
}

struct OperationGenerationGuard<'a> {
    generation: &'a AtomicU64,
    active: bool,
}

impl<'a> OperationGenerationGuard<'a> {
    fn begin(generation: &'a AtomicU64) -> Self {
        generation.fetch_add(1, Ordering::AcqRel);
        Self {
            generation,
            active: true,
        }
    }

    fn finish(mut self) -> u64 {
        self.active = false;
        self.generation.fetch_add(1, Ordering::AcqRel) + 1
    }
}

impl Drop for OperationGenerationGuard<'_> {
    fn drop(&mut self) {
        if self.active {
            self.generation.fetch_add(1, Ordering::AcqRel);
        }
    }
}

fn run_protected_window_operation<F>(generation: &AtomicU64, operation: F) -> ActionCompletion
where
    F: FnOnce() -> (Result<(), String>, Result<Vec<WindowInfo>, String>),
{
    let generation_guard = OperationGenerationGuard::begin(generation);
    let (applied, snapshot) = match catch_unwind(AssertUnwindSafe(operation)) {
        Ok(result) => result,
        Err(payload) => {
            let detail = payload
                .downcast_ref::<String>()
                .map(String::as_str)
                .or_else(|| payload.downcast_ref::<&str>().copied())
                .unwrap_or("unexpected panic");
            (
                Err(format!("Window operation failed unexpectedly: {detail}")),
                Err("Window snapshot was skipped after an unexpected operation failure.".into()),
            )
        }
    };
    (generation_guard.finish(), applied, snapshot)
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
    window_rx: Receiver<(u64, Result<Vec<WindowInfo>, String>)>,
    snapshot_generation: Arc<AtomicU64>,
    refresh_tx: mpsc::SyncSender<()>,
    hotkeys: Option<GlobalHotKeyManager>,
    state_file: std::path::PathBuf,
    _instance_lock: File,
    filter: String,
    hotkey_retry_needed: bool,
    last_hotkey_retry: Instant,
    registered_hotkey_ids: HashSet<u32>,
    recording_group: Option<String>,
    settings_open: bool,
    capture_armed: Arc<AtomicBool>,
    capture_listener_alive: Arc<AtomicBool>,
    capture_listener_was_alive: bool,
    capture_rx: Receiver<Result<String, String>>,
    focused_binding_rx: Receiver<FocusedBindingLookup>,
    focused_binding_tx: mpsc::Sender<FocusedBindingLookup>,
    focused_binding_in_flight: bool,
    pending_bind_group: Option<String>,
    action_tx: mpsc::SyncSender<Vec<WindowAction>>,
    action_rx: Receiver<ActionCompletion>,
    action_in_flight: bool,
    queued_toggles: VecDeque<String>,
}

type ActionCompletion = (u64, Result<(), String>, Result<Vec<WindowInfo>, String>);
type FocusedBindingLookup = (
    String,
    Result<Option<String>, String>,
    Result<Vec<WindowInfo>, String>,
);

impl VeilKeyApp {
    fn new(instance_lock: File) -> Self {
        let state_file = state_path();
        let mut state = load_state(&state_file);
        for group in &mut state.groups {
            group.hotkey_draft = group.hotkey.clone().unwrap_or_default();
        }
        // A slow Accessibility query must never let stale snapshots accumulate.
        let (window_tx, window_rx) = mpsc::sync_channel(1);
        let (refresh_tx, refresh_rx) = mpsc::sync_channel(1);
        let snapshot_generation = Arc::new(AtomicU64::new(0));
        let refresh_generation = Arc::clone(&snapshot_generation);
        thread::spawn(move || {
            while let Ok(()) | Err(mpsc::RecvTimeoutError::Timeout) =
                refresh_rx.recv_timeout(Duration::from_secs(2))
            {
                while refresh_rx.try_recv().is_ok() {}
                let generation = refresh_generation.load(Ordering::Acquire);
                let snapshot = NativeWindowController::enumerate();
                if window_tx.send((generation, snapshot)).is_err() {
                    break;
                }
            }
        });
        let (action_tx, action_requests) = mpsc::sync_channel::<Vec<WindowAction>>(1);
        let (action_results, action_rx) = mpsc::channel();
        let action_generation = Arc::clone(&snapshot_generation);
        thread::spawn(move || {
            while let Ok(actions) = action_requests.recv() {
                // Odd generations mean a native operation is in progress. Any
                // concurrent enumeration is discarded, even if it returns late.
                let completion = run_protected_window_operation(&action_generation, || {
                    let retry = actions.clone();
                    let applied = match NativeWindowController::apply_actions(actions) {
                        Ok(()) => Ok(()),
                        Err(first) => match NativeWindowController::apply_actions(retry) {
                            Ok(()) => Ok(()),
                            Err(second) => Err(format!("{first}; retry failed: {second}")),
                        },
                    };
                    let snapshot = NativeWindowController::enumerate();
                    (applied, snapshot)
                });
                if action_results.send(completion).is_err() {
                    break;
                }
            }
        });
        let capture_armed = Arc::new(AtomicBool::new(false));
        let listener_capture_armed = Arc::clone(&capture_armed);
        let capture_listener_alive = Arc::new(AtomicBool::new(false));
        let listener_alive = Arc::clone(&capture_listener_alive);
        let (capture_tx, capture_rx) = mpsc::channel();
        let (focused_binding_tx, focused_binding_rx) = mpsc::channel();
        thread::spawn(move || {
            let mut failures = 0_u32;
            loop {
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
                            let alt =
                                down.contains(&rdev::Key::Alt) || down.contains(&rdev::Key::AltGr);
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
                let attempt_started = Instant::now();
                let error = rdev::listen(callback)
                    .err()
                    .map(|error| format!("{error:?}"))
                    .unwrap_or_else(|| "listener stopped unexpectedly".into());
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
            snapshot_generation,
            refresh_tx,
            hotkeys,
            state_file,
            _instance_lock: instance_lock,
            filter: String::new(),
            hotkey_retry_needed: false,
            last_hotkey_retry: Instant::now(),
            registered_hotkey_ids: HashSet::new(),
            recording_group: None,
            settings_open: false,
            capture_armed,
            capture_listener_alive,
            capture_listener_was_alive: false,
            capture_rx,
            focused_binding_rx,
            focused_binding_tx,
            focused_binding_in_flight: false,
            pending_bind_group: None,
            action_tx,
            action_rx,
            action_in_flight: false,
            queued_toggles: VecDeque::new(),
        };
        app.register_saved_hotkeys();
        request_window_refresh(&app.refresh_tx);
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
        while let Ok((generation, applied, snapshot)) = self.action_rx.try_recv() {
            self.action_in_flight = false;
            match applied {
                Ok(()) => {}
                Err(error) => self.state.status = error,
            }
            if snapshot_is_current(generation, self.snapshot_generation.load(Ordering::Acquire)) {
                match snapshot {
                    Ok(windows) => self.windows = windows,
                    Err(error) => {
                        self.queued_toggles.clear();
                        if self.state.status.is_empty() {
                            self.state.status = error;
                        }
                    }
                }
            }
        }
        if !self.action_in_flight
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
        self.hotkey_retry_needed = !failed.is_empty();
        if failed.is_empty() {
            self.state.status = "Shortcuts recovered.".into();
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
                            NativeWindowController::enumerate()
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

    fn arm_focused_window_binding(&mut self, english: bool) {
        if self.focused_binding_in_flight {
            self.state.status = tr(
                english,
                "The previous focused-window lookup is still running.",
                "上一次前台窗口识别仍在进行，请稍候。",
            )
            .into();
            return;
        }
        self.pending_bind_group = None;
        self.capture_armed.store(true, Ordering::Release);
        self.state.status = if self.capture_listener_alive.load(Ordering::Acquire) {
            tr(
                english,
                "Focus the target window and press Ctrl/Alt/Shift/Win + a key.",
                "切到目标窗口，按 Ctrl/Alt/Shift/Win + 任意键完成绑定。",
            )
            .into()
        } else {
            tr(
                english,
                "Keyboard listener is restarting; your binding will stay armed.",
                "键盘监听正在恢复，绑定请求会保留，请稍后按下快捷键。",
            )
            .into()
        };
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
        if modifiers.ctrl || modifiers.mac_cmd {
            parts.push("ctrl");
        }
        if modifiers.alt {
            parts.push("alt");
        }
        if modifiers.shift {
            parts.push("shift");
        }
        if modifiers.command && !modifiers.mac_cmd {
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
        if self.action_in_flight {
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
        if let Some(receiver) = Some(GlobalHotKeyEvent::receiver()) {
            while let Ok(event) = receiver.try_recv() {
                if event.state() != HotKeyState::Pressed {
                    continue;
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
        if let Some(index) = group_to_toggle {
            self.toggle_group(index);
        }
    }

    fn update_window_snapshot(&mut self) {
        let mut latest = None;
        while let Ok((generation, result)) = self.window_rx.try_recv() {
            latest = Some((generation, result));
        }
        if let Some((generation, result)) = latest
            && snapshot_is_current(generation, self.snapshot_generation.load(Ordering::Acquire))
        {
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
}

impl eframe::App for VeilKeyApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        self.update_window_snapshot();
        self.update_window_actions();
        self.update_capture_listener_status();
        self.retry_hotkeys_if_needed();
        self.process_global_binding_capture(self.state.english_ui);
        self.complete_focused_window_bindings(self.state.english_ui);
        self.capture_hotkey(ctx);
        self.handle_hotkey_events();
        ctx.request_repaint_after(Duration::from_millis(100));
        let english = self.state.english_ui;
        let displayable_count = self
            .windows
            .iter()
            .filter(|w| window_is_displayable(w))
            .count();

        egui::TopBottomPanel::top("header").show(ctx, |ui| {
            ui.horizontal(|ui| {
                ui.heading("VeilKey");
                ui.add_space(8.0);
                ui.label(
                    egui::RichText::new(tr(english, "WINDOW GROUPS", "窗口快捷管理"))
                        .small()
                        .color(egui::Color32::from_rgb(120, 165, 185)),
                );
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui.button(tr(english, "中文", "English")).clicked() {
                        self.state.english_ui = !self.state.english_ui;
                        self.persist();
                    }
                    if ui.button(tr(english, "Settings", "设置")).clicked() {
                        self.settings_open = !self.settings_open;
                    }
                    if ui.button(tr(english, "Refresh", "刷新窗口")).clicked() {
                        request_window_refresh(&self.refresh_tx);
                    }
                    ui.label(if english {
                        format!("{displayable_count} windows")
                    } else {
                        format!("{displayable_count} 个窗口")
                    });
                });
            });
        });

        egui::CentralPanel::default().show(ctx, |ui| {
            if self.settings_open {
                ui.horizontal(|ui| {
                    ui.heading(tr(english, "Settings", "设置"));
                    ui.label(tr(english, "Binding mode", "绑定方式"));
                    let list_mode = self.state.bind_mode == BindMode::SelectFromList;
                    if ui
                        .selectable_label(list_mode, tr(english, "Select", "列表多选"))
                        .clicked()
                    {
                        self.state.bind_mode = BindMode::SelectFromList;
                        self.persist();
                    }
                    if ui
                        .selectable_label(
                            !list_mode,
                            tr(english, "One window at a time", "逐个绑定"),
                        )
                        .clicked()
                    {
                        self.state.bind_mode = BindMode::OneWindowAtATime;
                        self.persist();
                    }
                    if ui
                        .button(tr(english, "Bind focused window…", "绑定当前前台窗口…"))
                        .clicked()
                    {
                        self.arm_focused_window_binding(english);
                    }
                    ui.label(tr(
                        english,
                        "Language is saved automatically.",
                        "语言会自动保存。",
                    ));
                });
                ui.separator();
            }
            if !self.state.status.is_empty() {
                ui.horizontal(|ui| {
                    ui.label(
                        egui::RichText::new(localized_status(english, &self.state.status))
                            .color(egui::Color32::from_rgb(235, 182, 95)),
                    );
                    if ui.small_button("×").clicked() {
                        self.state.status.clear();
                    }
                });
                ui.add_space(4.0);
            }
            ui.columns(2, |columns| {
                let ui = &mut columns[0];
                ui.heading(tr(english, "Window groups", "窗口组"));
                ui.label(tr(
                    english,
                    "Create a group, then use one shortcut to show or hide it.",
                    "把窗口加入组，再用一个快捷键统一显示或隐藏。",
                ));
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
                                "Hide whole app instead of minimizing",
                                "隐藏整个应用，不最小化窗口",
                            ),
                        ),
                    );
                    let hide_application_changed = hide_application.changed();
                    hide_application.on_hover_text(tr(
                        english,
                        "macOS only. Hides every window of each app in this group. The app icon remains in the Dock; this avoids minimized-window tiles. Change this option while the group is shown.",
                        "仅 macOS。会隐藏组内每个应用的全部窗口，应用图标仍保留在程序坞；可避免出现最小化窗口缩略项。请在窗口组显示时调整。",
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
    }
}

fn main() -> eframe::Result {
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([1000.0, 680.0])
            .with_min_inner_size([780.0, 520.0]),
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
        Box::new(move |_cc| Ok(Box::new(VeilKeyApp::new(instance_lock)))),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

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
        request_window_refresh(&sender);
        request_window_refresh(&sender);
        request_window_refresh(&sender);
        assert!(receiver.try_recv().is_ok());
        assert!(receiver.try_recv().is_err());
    }

    #[test]
    fn snapshots_from_during_or_before_window_operations_are_discarded() {
        assert!(snapshot_is_current(4, 4));
        assert!(!snapshot_is_current(3, 3));
        assert!(!snapshot_is_current(2, 4));
        assert!(!snapshot_is_current(4, 5));
    }

    #[test]
    fn panicking_window_operation_recovers_snapshot_generation_and_returns_error() {
        let generation = AtomicU64::new(0);
        let (generation, applied, snapshot) =
            run_protected_window_operation(&generation, || panic!("simulated worker fault"));

        assert_eq!(generation, 2);
        assert!(snapshot_is_current(generation, generation));
        assert!(applied.unwrap_err().contains("simulated worker fault"));
        assert!(snapshot.is_err());
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
}
