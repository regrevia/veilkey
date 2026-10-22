use global_hotkey::hotkey::HotKey;
use global_hotkey::{GlobalHotKeyEvent, GlobalHotKeyManager};
use std::sync::mpsc::{self, Receiver, Sender, SyncSender};
use std::thread;
use std::time::Duration;
use windows::Win32::UI::WindowsAndMessaging::GetForegroundWindow;
use windows::Win32::UI::WindowsAndMessaging::{
    DispatchMessageW, MSG, PM_REMOVE, PeekMessageW, TranslateMessage,
};

#[derive(Clone, Copy, Debug)]
pub struct HotkeyEvent {
    pub event: global_hotkey::GlobalHotKeyEvent,
    pub foreground_handle: isize,
    pub woke_hidden_viewport: bool,
}

enum Command {
    Register(HotKey, SyncSender<Result<(), String>>),
    Unregister(HotKey, SyncSender<Result<(), String>>),
}

pub struct HotkeyManager {
    commands: SyncSender<Command>,
}

impl HotkeyManager {
    pub fn new(events: Sender<HotkeyEvent>) -> Result<Self, String> {
        let (commands, requests) = mpsc::sync_channel(128);
        let (ready_tx, ready_rx) = mpsc::sync_channel(1);
        thread::Builder::new()
            .name("veilkey-hotkeys".into())
            .spawn(move || run_message_thread(requests, ready_tx, events))
            .map_err(|error| error.to_string())?;
        ready_rx
            .recv()
            .map_err(|_| "Windows hotkey thread stopped during startup.".to_owned())??;
        Ok(Self { commands })
    }

    pub fn register(&self, hotkey: HotKey) -> Result<(), String> {
        self.call(CommandKind::Register, hotkey)
    }

    pub fn unregister(&self, hotkey: HotKey) -> Result<(), String> {
        self.call(CommandKind::Unregister, hotkey)
    }

    fn call(&self, kind: CommandKind, hotkey: HotKey) -> Result<(), String> {
        let (reply, response) = mpsc::sync_channel(1);
        let command = match kind {
            CommandKind::Register => Command::Register(hotkey, reply),
            CommandKind::Unregister => Command::Unregister(hotkey, reply),
        };
        self.commands
            .send(command)
            .map_err(|_| "Windows hotkey thread stopped.".to_owned())?;
        response
            .recv()
            .map_err(|_| "Windows hotkey thread stopped before replying.".to_owned())?
    }
}

enum CommandKind {
    Register,
    Unregister,
}

fn run_message_thread(
    requests: Receiver<Command>,
    ready: SyncSender<Result<(), String>>,
    events: Sender<HotkeyEvent>,
) {
    let manager = match GlobalHotKeyManager::new() {
        Ok(manager) => {
            veilkey::platform::debug_log("dedicated hotkey message thread started");
            let _ = ready.send(Ok(()));
            manager
        }
        Err(error) => {
            let _ = ready.send(Err(error.to_string()));
            return;
        }
    };
    let receiver = GlobalHotKeyEvent::receiver();
    loop {
        while let Ok(command) = requests.try_recv() {
            match command {
                Command::Register(hotkey, reply) => {
                    let result = manager.register(hotkey).map_err(|error| error.to_string());
                    veilkey::platform::debug_log(format!(
                        "register hotkey id={} result={:?}",
                        hotkey.id, result
                    ));
                    let _ = reply.send(result);
                }
                Command::Unregister(hotkey, reply) => {
                    let result = manager
                        .unregister(hotkey)
                        .map_err(|error| error.to_string());
                    veilkey::platform::debug_log(format!(
                        "unregister hotkey id={} result={:?}",
                        hotkey.id, result
                    ));
                    let _ = reply.send(result);
                }
            }
        }
        while let Ok(event) = receiver.try_recv() {
            let foreground_handle = unsafe { GetForegroundWindow().0 as isize };
            veilkey::platform::debug_log(format!(
                "message thread forwarded hotkey id={} state={:?} foreground=0x{:x}",
                event.id(),
                event.state(),
                foreground_handle
            ));
            if events
                .send(HotkeyEvent {
                    event,
                    foreground_handle,
                    woke_hidden_viewport: false,
                })
                .is_err()
            {
                return;
            }
        }

        let mut message = MSG::default();
        unsafe {
            while PeekMessageW(&mut message, None, 0, 0, PM_REMOVE).as_bool() {
                let _ = TranslateMessage(&message);
                DispatchMessageW(&message);
            }
        }
        thread::sleep(Duration::from_millis(2));
    }
}
