## 0.2.8 - 2026-10-19

- Complete Chinese/English UI text and localized status messages.
- Clarify individual window binding and settings labels.

## 0.2.7 - 2026-10-19

- Default the interface to Chinese and add a persistent English switch.
- Filter hidden and zero-size windows from the available-window list.
- Add list-selection and individual-window binding modes.

## 0.2.6 - 2026-10-19

- Retry transient native window operations once before reporting failure.
- Preserve a backup when the settings file is corrupt, then recover with defaults.

## 0.2.5 - 2026-10-19

- Simplify the group and shortcut binding interface.
- Ignore modifier-only key presses while recording shortcuts.

## 0.2.4 - 2026-10-19

- Add in-app shortcut recording so groups can be bound by pressing a key combination.
- Keep shortcut capture and registration errors visible in the status bar.

## 0.2.3 - 2026-10-19

- Match moved Windows windows by stable HWND identity even when their sort order changes.
- Include the macOS process ID in window bindings to avoid same-name collisions.

## 0.2.2 - 2026-10-19

- Keep Windows bindings stable when windows move and retry failed global shortcuts.
- Refresh native window state after group operations.

# Changelog

## 0.2.1 - 2026-10-19

- Keep shortcut text editable until the user binds the new value.
- Reconcile group toggles from current native window visibility instead of stale saved state.
- Build Intel macOS artifacts on the available macOS runner architecture instead of queueing an unavailable legacy Intel runner.

## 0.2.0 - 2026-10-19

- Added grouped window show/hide with persistent geometry restoration.
- Added non-activating Windows show behavior for games and full-screen apps.
- Added optional non-activating topmost group display.
- Added resilient batch window operations and foreground restoration on Windows.
- Added reproducible Windows installer and macOS app packaging for tagged releases.

## 0.1.0

- Initial clean-room VeilKey project.







