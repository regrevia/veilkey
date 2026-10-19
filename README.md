# VeilKey

VeilKey is a small desktop utility for binding several application windows into a named group and showing or hiding that group with one global keyboard shortcut. It aims to restore each window to its saved position and size when shown again.

This is an independent clean-room project. The earlier project informed only the general product idea; no source code, artwork, or documentation was carried over.

## Current status

VeilKey is in active early access. The current release includes a group manager, visible window discovery, per-group global shortcuts, atomic persistent settings, Windows window control, and macOS Accessibility-based window control. The interface defaults to Chinese and can be switched to English from the top bar. Choose list selection or individual binding in Settings. Select visible windows, create a named group, then record or enter its shortcut. Windows without a visible, non-zero-size top-level surface are omitted. A shortcut already registered by another application may be unavailable.

Download the Windows setup program or portable executable, or the macOS archive, from [GitHub Releases](https://github.com/regrevia/veilkey/releases). Windows setup is per-user and does not require administrator access. macOS builds are not currently signed or notarized; Gatekeeper may require the user to approve the app, and Accessibility permission must be granted before window discovery works. Save the macOS archive, extract `VeilKey.app` to Applications, then enable it in **System Settings → Privacy & Security → Accessibility**.

On macOS, grant VeilKey permission under **System Settings → Privacy & Security → Accessibility**. Window discovery and control depend on that permission. On Windows, windows must expose a title and a usable top-level window handle to be listed and controlled.

On macOS, each group can optionally **hide whole apps instead of minimizing windows**. This avoids putting minimized-window tiles in the Dock, but hides every window owned by each app in the group, including windows not selected into the group. The app's Dock icon remains visible; macOS does not let VeilKey safely remove another app's Dock icon.

On Windows, groups show windows without requesting foreground activation. The optional **Force popup (keep above, don't focus)** setting uses a non-activating topmost placement when showing the group; it is intended for ordinary and borderless-fullscreen desktops. The default path does not change z-order, and group actions are applied as one batch; if hiding the group unexpectedly changes foreground focus, VeilKey attempts to restore the app that was already focused (such as a game). It does not change display resolution or DPI scaling. Leave force popup off if any overlay could cover game content. Exclusive-fullscreen games control the display directly, so Windows cannot guarantee that another app will appear above them; VeilKey also cannot guarantee zero presentation stutter on every GPU/driver/game combination. Prefer the game's **Borderless fullscreen / Windowed fullscreen / 全屏窗口** mode when you need other windows to appear over it. This is usually a borderless window at the monitor's desktop resolution, unlike exclusive fullscreen which can switch display mode.

The window list shows windows with a visible, non-zero-size surface; it does not yet render live image thumbnails. Bind a focused window by choosing **Bind focused window**, switching to the target window, then pressing a shortcut with Ctrl, Alt, Shift, or Win. In **One window at a time** mode, choose **Bind this window** first, then focus that window and press its shortcut. macOS may require VeilKey to be enabled under **System Settings → Privacy & Security → Accessibility** for global key capture as well as window discovery. Automatic recovery after another application takes a shortcut, signed/notarized macOS distribution, and long-duration testing on real desktops remain future work. Please report platform-specific issues with OS version and steps to reproduce.

## Build

Install the stable Rust toolchain, then run:

```sh
cargo build --release
```

Run the test suite and compile checks with:

```sh
cargo test --all-targets
cargo check --all-targets
cargo clippy --all-targets -- -D warnings
```

Tagged releases are built by GitHub Actions. The private CNB repository mirrors source and runs the macOS ARM64 release build check.

## License

MIT. See [LICENSE](LICENSE).


