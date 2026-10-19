use std::sync::Arc;
use std::sync::Mutex;
use std::sync::OnceLock;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::SyncSender;
use windows::Win32::Foundation::{HINSTANCE, LPARAM, LRESULT, WPARAM};
use windows::Win32::UI::Input::KeyboardAndMouse::{
    GetAsyncKeyState, VK_CONTROL, VK_LWIN, VK_MENU, VK_RWIN, VK_SHIFT,
};
use windows::Win32::UI::WindowsAndMessaging::{
    CallNextHookEx, GetMessageW, HC_ACTION, KBDLLHOOKSTRUCT, MSG, SetWindowsHookExW,
    UnhookWindowsHookEx, WH_KEYBOARD_LL, WM_KEYDOWN, WM_KEYUP, WM_SYSKEYDOWN, WM_SYSKEYUP,
};

struct HookState {
    armed: Arc<AtomicBool>,
    sender: SyncSender<Result<String, String>>,
}

static HOOK_STATE: OnceLock<Mutex<Option<HookState>>> = OnceLock::new();

unsafe extern "system" fn keyboard_hook(code: i32, message: WPARAM, data: LPARAM) -> LRESULT {
    if code == HC_ACTION as i32
        && matches!(
            message.0 as u32,
            WM_KEYDOWN | WM_SYSKEYDOWN | WM_KEYUP | WM_SYSKEYUP
        )
    {
        let state = HOOK_STATE.get_or_init(|| Mutex::new(None));
        let Ok(context) = state.lock() else {
            return unsafe { CallNextHookEx(None, code, message, data) };
        };
        let Some(context) = context.as_ref() else {
            return unsafe { CallNextHookEx(None, code, message, data) };
        };
        let key = unsafe { (*(data.0 as *const KBDLLHOOKSTRUCT)).vkCode as u16 };
        if matches!(message.0 as u32, WM_KEYDOWN | WM_SYSKEYDOWN)
            && context.armed.load(Ordering::Relaxed)
            && let Some(chord) = capture_chord(key)
            && context.armed.swap(false, Ordering::AcqRel)
            && context.sender.try_send(Ok(chord)).is_err()
        {
            context.armed.store(true, Ordering::Release);
        }
    }
    unsafe { CallNextHookEx(None, code, message, data) }
}

pub fn listen(
    armed: Arc<AtomicBool>,
    alive: Arc<AtomicBool>,
    sender: SyncSender<Result<String, String>>,
) -> Result<(), String> {
    let state = HOOK_STATE.get_or_init(|| Mutex::new(None));
    *state
        .lock()
        .map_err(|_| "Windows keyboard listener state is poisoned".to_owned())? =
        Some(HookState { armed, sender });
    let hook = match unsafe {
        SetWindowsHookExW(WH_KEYBOARD_LL, Some(keyboard_hook), HINSTANCE::default(), 0)
    } {
        Ok(hook) => hook,
        Err(error) => {
            if let Ok(mut state) = state.lock() {
                *state = None;
            }
            alive.store(false, Ordering::Release);
            return Err(format!("Windows keyboard hook could not start: {error}"));
        }
    };
    alive.store(true, Ordering::Release);

    let mut message = MSG::default();
    let result = loop {
        match unsafe { GetMessageW(&mut message, None, 0, 0).0 } {
            value if value > 0 => continue,
            0 => break Err("Windows keyboard message loop stopped".to_owned()),
            _ => {
                break Err(format!(
                    "Windows keyboard message loop failed: {}",
                    std::io::Error::last_os_error()
                ));
            }
        }
    };

    unsafe {
        let _ = UnhookWindowsHookEx(hook);
    }
    if let Ok(mut state) = state.lock() {
        *state = None;
    }
    alive.store(false, Ordering::Release);
    result
}

fn capture_chord(key: u16) -> Option<String> {
    let control = key_down(VK_CONTROL.0);
    let alt = key_down(VK_MENU.0);
    let shift = key_down(VK_SHIFT.0);
    let super_key = key_down(VK_LWIN.0) || key_down(VK_RWIN.0);
    chord_from_modifiers(key, control, alt, shift, super_key)
}

fn chord_from_modifiers(
    key: u16,
    control: bool,
    alt: bool,
    shift: bool,
    super_key: bool,
) -> Option<String> {
    if !(control || alt || shift || super_key) {
        return None;
    }
    let key_name = virtual_key_name(key)?;
    if matches!(key_name, "Control" | "Alt" | "Shift" | "Super") {
        return None;
    }
    let mut parts = Vec::with_capacity(5);
    if control {
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
    Some(parts.join("+"))
}

fn key_down(key: u16) -> bool {
    unsafe { GetAsyncKeyState(key as i32) < 0 }
}

fn virtual_key_name(key: u16) -> Option<&'static str> {
    Some(match key {
        0x30..=0x39 => match key {
            0x30 => "Digit0",
            0x31 => "Digit1",
            0x32 => "Digit2",
            0x33 => "Digit3",
            0x34 => "Digit4",
            0x35 => "Digit5",
            0x36 => "Digit6",
            0x37 => "Digit7",
            0x38 => "Digit8",
            0x39 => "Digit9",
            _ => unreachable!(),
        },
        0x41..=0x5a => match key {
            0x41 => "KeyA",
            0x42 => "KeyB",
            0x43 => "KeyC",
            0x44 => "KeyD",
            0x45 => "KeyE",
            0x46 => "KeyF",
            0x47 => "KeyG",
            0x48 => "KeyH",
            0x49 => "KeyI",
            0x4a => "KeyJ",
            0x4b => "KeyK",
            0x4c => "KeyL",
            0x4d => "KeyM",
            0x4e => "KeyN",
            0x4f => "KeyO",
            0x50 => "KeyP",
            0x51 => "KeyQ",
            0x52 => "KeyR",
            0x53 => "KeyS",
            0x54 => "KeyT",
            0x55 => "KeyU",
            0x56 => "KeyV",
            0x57 => "KeyW",
            0x58 => "KeyX",
            0x59 => "KeyY",
            0x5a => "KeyZ",
            _ => unreachable!(),
        },
        0x70 => "F1",
        0x71 => "F2",
        0x72 => "F3",
        0x73 => "F4",
        0x74 => "F5",
        0x75 => "F6",
        0x76 => "F7",
        0x77 => "F8",
        0x78 => "F9",
        0x79 => "F10",
        0x7a => "F11",
        0x7b => "F12",
        0x08 => "Backspace",
        0x09 => "Tab",
        0x0d => "Enter",
        0x1b => "Escape",
        0x20 => "Space",
        0x10 | 0xa0 | 0xa1 => "Shift",
        0x11 | 0xa2 | 0xa3 => "Control",
        0x12 | 0xa4 | 0xa5 => "Alt",
        0x5b | 0x5c => "Super",
        0x25 => "ArrowLeft",
        0x26 => "ArrowUp",
        0x27 => "ArrowRight",
        0x28 => "ArrowDown",
        0xbd => "Minus",
        0xbb => "Equal",
        0xba => "Semicolon",
        0xbc => "Comma",
        0xbe => "Period",
        0xbf => "Slash",
        0xc0 => "Backquote",
        0xdb => "BracketLeft",
        0xdd => "BracketRight",
        0xdc => "Backslash",
        0xde => "Quote",
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::{chord_from_modifiers, virtual_key_name};

    #[test]
    fn windows_virtual_keys_map_to_persisted_hotkey_names() {
        assert_eq!(virtual_key_name(0x48), Some("KeyH"));
        assert_eq!(virtual_key_name(0x31), Some("Digit1"));
        assert_eq!(virtual_key_name(0x7b), Some("F12"));
        assert_eq!(virtual_key_name(0x100), None);
    }

    #[test]
    fn windows_hotkey_capture_requires_a_modifier() {
        assert_eq!(chord_from_modifiers(0x48, false, false, false, false), None);
        assert_eq!(
            chord_from_modifiers(0x48, true, true, false, false),
            Some("ctrl+alt+KeyH".into())
        );
    }
}
