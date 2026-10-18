# VeilKey

VeilKey is a cross-platform desktop utility for grouping windows and showing or hiding a group with one keyboard shortcut. A group keeps each window's original position and size so it can return to the same workspace arrangement.

This is a clean-room project. It is independently designed and implemented; the earlier project is used only as a product concept reference.

## Project status

Early development. The repository currently contains the project scaffold. Windows and macOS window discovery, grouping, shortcut capture, persistence, and recovery behavior are planned as platform adapters behind a shared core.

## Design goals

- Explicitly bind selected windows into named groups; keep the focused-window binding as a quick path.
- Toggle the entire group atomically and restore each window's prior visibility and geometry.
- Persist group state safely and reconcile stale window handles after restart.
- Keep global shortcut registration observable and recover from registration loss.
- Use native platform APIs through isolated Windows and macOS adapters.

## Build

Requires Rust stable.

```sh
cargo build --release
```

## License

MIT. See [LICENSE](LICENSE).
