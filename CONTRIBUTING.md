# Contributing to Nib

Thanks for considering a contribution. Nib is a small project with two codebases in one repo —
a Rust/GTK4 host app and a Flutter client — so a few conventions keep them consistent.

## Getting set up

Follow [Building & Running](README.md#building--running) in the README for prerequisites and how
to run the host app (`cargo run`) and client app (`flutter run`).

## Where things live

- `src/daemon/` — the streaming pipeline: portal/D-Bus session setup (`virtual_monitor.rs`),
  GStreamer pipeline construction (`screencast.rs`), the binary input protocol
  (`input_injector.rs`), ADB transport (`adb_transport.rs`), and the cursor-keepalive overlay
  workaround (`cursor_keepalive.rs`). `daemon/mod.rs` orchestrates all of these via `NibDaemon`.
- `src/ui/` — GTK4/Libadwaita widgets (device list, gesture help, display settings).
- `src/i18n.rs` — the app's translation lookup (`tr(key)`); see its doc comment for why this
  exists instead of gettext.
- `nib_client/` — the Flutter/Dart companion app; see its own `README.md` if present, and
  `nib_client/android/.../MainActivity.kt` for the Android `MediaCodec` decoder that the iOS side
  still needs an equivalent for.

## Conventions

- **Errors**: daemon-side fallible functions return `Result<_, daemon::DaemonError>` (see
  `src/daemon/error.rs`), not `String`. Prefer `?` with `DaemonError`'s `From` impls
  (`ashpd::Error`, `zbus::Error`, `std::io::Error`) over hand-rolled `.map_err(|e| e.to_string())`.
  If you need to detect a specific failure (e.g. a cancelled portal request), add a method like
  `DaemonError::is_cancelled()` rather than matching on the message string.
- **Panics**: avoid `.unwrap()`/`.expect()`/`panic!()` on anything reachable at runtime (portal
  calls, D-Bus replies, GStreamer state changes, mutex locks). It's fine in `main()`-time setup
  that truly can't recover, and in `#[cfg(test)]` code.
- **Docs**: every `pub` item should have a `///` doc comment explaining *why*, not just *what* —
  run `RUSTFLAGS="-W missing_docs" cargo check` to catch gaps. Comments on tricky bits (portal
  quirks, sandbox workarounds, GStreamer caps negotiation) should explain the constraint that
  forced the code to look the way it does, since that context isn't visible from the diff alone.
- **Sandboxing quirks**: this app has been bitten repeatedly by differences between running under
  Flatpak confinement and running natively (`cargo run`). If you touch sandbox detection
  (`ScreencastPipeline::is_running_confined`) or anything encoder/PipeWire-related, test both
  paths if you can, and leave a comment explaining what broke and why if you're fixing a
  regression — see the git history around `screencast.rs` for the level of detail expected.

## Before opening a PR

CI runs `cargo fmt --check`, `cargo clippy -- -D warnings`, `cargo build`, and lints the Flatpak
manifest. Run these locally first:

```bash
cargo fmt
cargo clippy --all-targets -- -D warnings
cargo test
```

If you change dependencies in `Cargo.toml`, `cargo-sources.json` (used by the Flatpak build) needs
to be regenerated to match the new `Cargo.lock`. There's no committed generator script yet; the
standard tool is [`flatpak-cargo-generator.py`](https://github.com/flatpak/flatpak-builder-tools/blob/master/cargo/flatpak-cargo-generator.py).

## Code of Conduct

This project follows the [GNOME Code of Conduct](https://conduct.gnome.org/).
