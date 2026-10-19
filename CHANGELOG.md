## Unreleased

- Keep processing Windows window groups and restoring the previously focused app when one forced z-order operation fails.
- Exclude minimized Windows from the visible-window count and read their normal placement bounds for reliable group restoration.
- Convert minimized-window workspace coordinates to screen coordinates so restoration remains aligned with the correct monitor work area.
- Preserve screen-coordinate behavior for tool windows and cover negative multi-monitor coordinates in regression tests.

## 0.2.25 - 2026-10-19

- Recover mixed-visibility groups toward the last requested state after partial native window failures, and avoid moving windows that are already visible when restoring a group.

## 0.2.24 - 2026-10-19

- Add an optional macOS group mode that hides the owning apps instead of minimizing their windows, avoiding minimized-window tiles in the Dock. The mode explicitly affects every window belonging to each app and leaves its Dock icon visible.

## 0.2.23 - 2026-10-19

- Prevent multiple VeilKey instances from competing for global shortcuts or overwriting each other's saved settings; the lock is released automatically when the app exits.

## 0.2.22 - 2026-10-19

- Preserve restore positions for windows that already hid successfully when another window in the same group needs a retry.

## 0.2.21 - 2026-10-19

- Wait for Windows visibility changes to reach the native window state before refreshing snapshots and processing queued shortcuts; report bounded timeouts for unresponsive windows.

## 0.2.20 - 2026-10-19

- Track shortcut recording targets by group name so deleting an earlier group cannot redirect a pending binding to a different group.

## 0.2.19 - 2026-10-19

- Move focused-window discovery after shortcut capture to a background task so slow macOS Accessibility queries do not freeze the interface.
- Bound concurrent focused-window lookups and apply a timeout to the macOS system-wide focused-window query.

## 0.2.18 - 2026-10-19

- Report every global key listener failure instead of suppressing all failures after the first incident.
- Reset the listener retry backoff after five minutes of stable operation so a later isolated failure recovers quickly.

## 0.2.17 - 2026-10-19

- Run native window group operations off the UI thread and serialize rapid shortcut toggles against fresh window snapshots.
- Keep macOS window bindings stable when same-titled windows move by matching Accessibility element identity and process ID.
- Apply Accessibility messaging timeouts to individual macOS windows to limit stalls from unresponsive applications.

## 0.2.16 - 2026-10-19

- Run unit tests on Windows and macOS for every CI check.
- Gate tagged Windows and macOS release packaging on passing tests and Clippy checks on both operating systems.

## 0.2.15 - 2026-10-19

- Bound and coalesce window refresh requests so slow Accessibility enumeration cannot accumulate an unbounded queue of redundant desktop scans.

## 0.2.14 - 2026-10-19

- Enumerate macOS Accessibility windows once per group operation, avoiding repeated desktop-wide queries for each window and reducing toggle latency.

## 0.2.13 - 2026-10-19

- Back off global key capture retries exponentially up to 60 seconds instead of retrying every 2 seconds during prolonged permission or system failures.
- Clear stale listener errors and report when global key capture recovers.

## 0.2.12 - 2026-10-19

- Restart the global key capture listener with backoff after transient system or permission errors, keeping pending shortcut binding requests armed.
- Show a localized recovery message when global keyboard capture is unavailable.

## 0.2.11 - 2026-10-19

- Protect Windows window bindings against recycled native handles by including the owning process ID in new window identities.
- Retain compatibility with existing saved window bindings.

## 0.2.10 - 2026-10-19

- Preserve working global shortcuts while retrying only registrations that failed, preventing recovery attempts from disabling active bindings.

## 0.2.9 - 2026-10-19

- Bind the focused foreground window directly by pressing a global shortcut combination.
- Allow one-window-at-a-time binding to capture its shortcut while that target window is focused.
- Document global key capture permissions and clarify that window thumbnails are not yet available.

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







