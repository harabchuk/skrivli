# Skrivli — agent guide

Windows desktop app (Win10/11, `x86_64-pc-windows-msvc`). Rust stable, one Cargo package (`src/lib.rs` testable library, `src/main.rs` thin entry). UI is **GPUI Kit** (`gpui-kit`); do not mix its re-exported GPUI crates at other versions. Windows APIs via the `windows` crate. Audio is WASAPI on dedicated threads. Settings and sidecars are `serde_json`. Transcription is local whisper.cpp. Logs use `tracing`. Commit `Cargo.lock`.

`domain/` is the core. It has no GPUI, Windows, COM, file I/O, or AI types. `services/` own use cases and depend on ports. `infrastructure/` implements adapters. `ui/` is only Kit overrides and custom controls. `views/` compose screens. Rendering never owns a WASAPI handle or calls Whisper. The GPUI thread does no blocking audio, disk, or inference work.

## Layout

Add a file only when implementing it.

```text
src/
├── main.rs
├── lib.rs
├── app/                  # bootstrap, wiring, actions, presentation state
├── domain/               # core: formats, timeline, recording, transcript
├── services/             # recording, level monitor, transcription, ports
├── infrastructure/
│   ├── audio/            # WASAPI capture, loopback, mixer, resample
│   ├── files/            # WAV, optional M4A, reader, player
│   ├── ai/               # whisper.cpp, model files
│   ├── storage/          # settings, library, sidecars, watcher
│   └── windows/          # tray, hotkey, dialogs, autostart
├── ui/
│   └── components/       # Kit overrides and custom controls (e.g. level meter)
├── views/                # main window, record, recordings, settings, model prompt
└── common/               # errors, paths, logging
tests/                    # integration tests against public lib APIs
```

## Tests first (red, green, refactor)

Write the failing unit test before production code.

1. **Red.** Add the test. Run it. It must fail for the missing behavior, not for a typo or a compile error you intend to leave.
2. **Green.** Write the smallest implementation that passes.
3. **Refactor.** Clean up with the test still green.

Colocate unit tests as `#[cfg(test)] mod tests`. Put multi-module flows in `tests/`. Use injected clocks, temp dirs, and fake ports. Default `cargo test` must not need hardware.

Mark Arrange, Act, and Assert in every test:

```rust
#[test]
fn formats_timestamp_past_one_hour() {
    // Arrange
    let elapsed = Duration::from_secs(3661);

    // Act
    let text = format_timestamp(elapsed);

    // Assert
    assert_eq!(text, "[01:01:01]");
}
```

## Rust style

- `cargo fmt` and `cargo clippy --all-targets -- -D warnings` stay clean.
- `snake_case` modules and functions, `PascalCase` types, `SCREAMING_SNAKE` constants. Group by responsibility, not one type per file. Keep helpers private.
- Return `Result` with a typed error at boundaries. Propagate with `?`. Do not `unwrap` or `expect` outside tests and documented invariants.
- Own data at boundaries; borrow inside. Do not `clone` to dodge a lifetime.
- Newtypes for IDs. Enums for state. Match domain enums exhaustively.
- Constructors validate. Fields stay private.
- Release COM, threads, files, and handles with `Drop`. Do not claim `Send` or `Sync` for COM wrappers.
- Prefer iterators and `From` / `TryFrom`. Prefer `pub(crate)` until a type is a real public API.
- Log with `tracing`. Never log audio samples or transcript text.
