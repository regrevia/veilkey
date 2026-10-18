use eframe::egui;
use global_hotkey::hotkey::HotKey;
use global_hotkey::{GlobalHotKeyEvent, GlobalHotKeyManager, HotKeyState};
use std::str::FromStr;
use std::sync::mpsc::{self, Receiver};
use std::thread;
use std::time::Duration;
use veilkey::platform::NativeWindowController;
use veilkey::{AppState, WindowAction, WindowInfo, load_state, save_state, state_path};

struct VeilKeyApp {
    state: AppState,
    windows: Vec<WindowInfo>,
    last_refresh: std::time::Instant,
    window_rx: Receiver<Result<Vec<WindowInfo>, String>>,
    hotkeys: Option<GlobalHotKeyManager>,
    state_file: std::path::PathBuf,
    filter: String,
}

impl VeilKeyApp {
    fn new() -> Self {
        let state_file = state_path();
        let state = load_state(&state_file);
        // A slow Accessibility query must never let stale snapshots accumulate.
        let (window_tx, window_rx) = mpsc::sync_channel(1);
        thread::spawn(move || {
            loop {
                match window_tx.try_send(NativeWindowController::enumerate()) {
                    Ok(()) | Err(mpsc::TrySendError::Full(_)) => {}
                    Err(mpsc::TrySendError::Disconnected(_)) => break,
                }
                thread::sleep(Duration::from_secs(2));
            }
        });
        let hotkeys = GlobalHotKeyManager::new().ok();
        let mut app = Self {
            state,
            windows: Vec::new(),
            last_refresh: std::time::Instant::now(),
            window_rx,
            hotkeys,
            state_file,
            filter: String::new(),
        };
        app.register_saved_hotkeys();
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
            self.state.status = "Global shortcuts are unavailable on this system.".into();
            return;
        };
        let mut failed = Vec::new();
        for group in &self.state.groups {
            if let Some(value) = group.hotkey.as_deref() {
                match HotKey::from_str(value) {
                    Ok(hotkey) if manager.register(hotkey).is_ok() => {}
                    Ok(_) => failed.push(format!("{} ({value})", group.name)),
                    Err(_) => failed.push(format!("{} (invalid shortcut)", group.name)),
                }
            }
        }
        if !failed.is_empty() {
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
        {
            let _ = manager.unregister(old);
        }
        match manager.register(hotkey) {
            Ok(()) => {
                if let Some(group) = self.state.groups.get_mut(index) {
                    group.hotkey = Some(value);
                }
                self.state.status = "Shortcut registered.".into();
                self.persist();
            }
            Err(error) => {
                if let Some(old) = old_value
                    .as_deref()
                    .and_then(|old| HotKey::from_str(old).ok())
                {
                    let _ = manager.register(old);
                }
                self.state.status = format!("Could not register shortcut: {error}");
            }
        }
    }

    fn apply_actions(&mut self, actions: Vec<WindowAction>) {
        if let Err(error) = NativeWindowController::apply_actions(actions) {
            self.state.status = error;
        }
    }

    fn toggle_group(&mut self, index: usize) {
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
        while let Ok(result) = self.window_rx.try_recv() {
            latest = Some(result);
        }
        if let Some(result) = latest {
            match result {
                Ok(windows) => {
                    self.windows = windows;
                    self.last_refresh = std::time::Instant::now();
                }
                Err(error) => self.state.status = error,
            }
        }
    }
}

impl eframe::App for VeilKeyApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        self.update_window_snapshot();
        self.handle_hotkey_events();
        ctx.request_repaint_after(Duration::from_millis(100));

        egui::TopBottomPanel::top("header").show(ctx, |ui| {
            ui.horizontal(|ui| {
                ui.heading("VeilKey");
                ui.add_space(8.0);
                ui.label(
                    egui::RichText::new("WINDOW GROUPS")
                        .small()
                        .color(egui::Color32::from_rgb(120, 165, 185)),
                );
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui.button("Refresh windows").clicked() {
                        self.last_refresh = std::time::Instant::now() - Duration::from_secs(3);
                    }
                    ui.label(format!("{} windows", self.windows.len()));
                });
            });
        });

        egui::CentralPanel::default().show(ctx, |ui| {
            if !self.state.status.is_empty() {
                ui.horizontal(|ui| {
                    ui.label(
                        egui::RichText::new(&self.state.status)
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
                ui.heading("Groups");
                ui.label("Bind windows once, then toggle a whole group together.");
                ui.add_space(8.0);
                if self.state.groups.is_empty() {
                    ui.label(egui::RichText::new("No groups yet").strong());
                    ui.label("Select windows from the list and create your first group.");
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
                                "{} windows · {}",
                                group.window_ids.len(),
                                if group.is_visible { "shown" } else { "hidden" }
                            ));
                        });
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            if ui.small_button("Remove").clicked() {
                                delete = Some(index);
                            }
                            if ui
                                .button(if group.is_visible {
                                    "Hide group"
                                } else {
                                    "Show group"
                                })
                                .clicked()
                            {
                                toggle = Some(index);
                            }
                        });
                    });
                    ui.horizontal(|ui| {
                        ui.label("Shortcut");
                        let mut value = group.hotkey.clone().unwrap_or_default();
                        ui.add(
                            egui::TextEdit::singleline(&mut value)
                                .hint_text("ctrl+alt+KeyH")
                                .desired_width(150.0),
                        );
                        if ui.small_button("Bind").clicked() {
                            hotkey_save = Some((index, value));
                        }
                    });
                    if ui
                        .checkbox(
                            &mut group.force_popup,
                            "Force popup (keep above, don't focus)",
                        )
                        .changed()
                    {
                        group_option_changed = true;
                    }
                }
                if let Some(index) = delete {
                    if let Some(group) = self.state.groups.get(index) {
                        if let Some(hotkey) = group
                            .hotkey
                            .as_deref()
                            .and_then(|s| HotKey::from_str(s).ok())
                        {
                            if let Some(manager) = &self.hotkeys {
                                let _ = manager.unregister(hotkey);
                            }
                        }
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
                ui.heading("Open windows");
                ui.label("Choose the windows that should belong to the new group.");
                ui.add_space(8.0);
                ui.horizontal(|ui| {
                    ui.label("Group name");
                    ui.add(
                        egui::TextEdit::singleline(&mut self.state.new_group_name)
                            .hint_text("e.g. MuMu set")
                            .desired_width(190.0),
                    );
                });
                ui.add_space(6.0);
                if ui.button("Create group from selected").clicked() {
                    if self
                        .state
                        .create_group(self.state.new_group_name.trim().to_owned())
                    {
                        self.state.status =
                            "Group created. Set a shortcut to toggle it from anywhere.".into();
                        self.persist();
                    } else {
                        self.state.status =
                            "Enter a group name and select at least one window.".into();
                    }
                }
                ui.separator();
                ui.horizontal(|ui| {
                    ui.label("Filter");
                    ui.add(
                        egui::TextEdit::singleline(&mut self.filter)
                            .hint_text("app or title")
                            .desired_width(180.0),
                    );
                });
                egui::ScrollArea::vertical().show(ui, |ui| {
                    for window in &self.windows {
                        let text = format!("{} {}", window.app, window.title);
                        if !self.filter.is_empty()
                            && !text.to_lowercase().contains(&self.filter.to_lowercase())
                        {
                            continue;
                        }
                        ui.horizontal(|ui| {
                            let mut selected = self.state.selected_windows.contains(&window.id);
                            if ui.checkbox(&mut selected, "").changed() {
                                if selected {
                                    self.state.selected_windows.insert(window.id.clone());
                                } else {
                                    self.state.selected_windows.remove(&window.id);
                                }
                            }
                            ui.vertical(|ui| {
                                ui.label(egui::RichText::new(&window.title).strong());
                                ui.label(format!(
                                    "{} · {} · {}×{}",
                                    window.app,
                                    if window.visible { "visible" } else { "hidden" },
                                    window.rect.width,
                                    window.rect.height
                                ));
                            });
                        });
                    }
                });
            });
        });
    }
}

fn main() -> eframe::Result {
    let mut options = eframe::NativeOptions::default();
    options.viewport = egui::ViewportBuilder::default()
        .with_inner_size([1000.0, 680.0])
        .with_min_inner_size([780.0, 520.0]);
    eframe::run_native(
        "VeilKey",
        options,
        Box::new(|_cc| Ok(Box::new(VeilKeyApp::new()))),
    )
}
