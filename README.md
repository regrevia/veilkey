# VeilKey

VeilKey is a small desktop utility for binding several application windows into a named group and showing or hiding that group with one global keyboard shortcut. It aims to restore each window to its saved position and size when shown again.

This is an independent clean-room project. The earlier project informed only the general product idea; no source code, artwork, or documentation was carried over.

## Current status

VeilKey is in early development. The current build includes a basic group manager, window discovery, per-group shortcuts, and persistent settings on Windows and macOS. Select the windows in the list, enter a group name, and create the group. Enter a shortcut such as `ctrl+alt+KeyH` or `super+shift+KeyH` in that group's Shortcut field, then choose **Bind**. A shortcut already registered by another application may be unavailable.

On macOS, grant VeilKey permission under **System Settings → Privacy & Security → Accessibility**. Window discovery and control depend on that permission. On Windows, windows must expose a title and a usable top-level window handle to be listed and controlled.

On Windows, groups show windows without requesting foreground activation. The optional **Force popup (keep above, don't focus)** setting uses a non-activating topmost placement when showing the group; it is intended for ordinary and borderless-fullscreen desktops. The default path does not change z-order, and group actions are applied as one batch; if hiding the group unexpectedly changes foreground focus, VeilKey attempts to restore the app that was already focused (such as a game). It does not change display resolution or DPI scaling. Leave force popup off if any overlay could cover game content. Exclusive-fullscreen games control the display directly, so Windows cannot guarantee that another app will appear above them; VeilKey also cannot guarantee zero presentation stutter on every GPU/driver/game combination. Prefer the game's **Borderless fullscreen / Windowed fullscreen / 全屏窗口** mode when you need other windows to appear over it. This is usually a borderless window at the monitor's desktop resolution, unlike exclusive fullscreen which can switch display mode.

The UI and native window adapters are still evolving. In particular, shortcut capture, automatic recovery after another application takes a shortcut, signed/notarized distribution, and long-duration testing on real desktops remain future work. Please report platform-specific issues with OS version and steps to reproduce.

## Build

Install the stable Rust toolchain, then run:

```sh
cargo build --release
```

Run the test suite and compile checks with:

```sh
cargo test --all-targets
cargo check --all-targets
```

## License

MIT. See [LICENSE](LICENSE).
