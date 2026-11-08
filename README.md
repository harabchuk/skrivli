# Skrivli

Skrivli is a local Windows meeting recorder. It captures meeting audio on the machine and keeps recordings and transcripts as ordinary files. Nothing in the foundation build sends audio or transcripts off the computer.

## Setup

Target: `x86_64-pc-windows-msvc` on Windows 10 or 11.

1. Install [rustup](https://rustup.rs/) and the stable toolchain (`rust-toolchain.toml` selects `stable`). If `cargo` is not on `PATH`, prepend the rustup bin directory:
   ```powershell
    $env:Path = "$env:USERPROFILE\.cargo\bin;" + $env:Path
   ```
2. Install the MSVC Build Tools (Visual Studio Build Tools workload "Desktop development with C++") and a Windows 10/11 SDK. The linker is `link.exe` from that toolchain.
3. DPI awareness is Per-Monitor V2. `gpui-pre` 0.3.11 already embeds that manifest, and the MSVC linker rejects a second one, so `build.rs` embeds the icon and version resource only. `resources/skrivli.manifest` is the same DPI contract, kept in the repo and not embedded. 100% and 150% scaling were not exercised on hardware for this milestone.

## Commands

```powershell
cargo fmt --all -- --check
cargo clippy --all-targets -- -D warnings
cargo test
cargo build --release
cargo run
```

`cargo test` does not use a microphone, a GPU device check, or an interactive window. `cargo run` opens the Skrivli window.

## License

Skrivli is an open-source meeting recording and transcription application.

Copyright © 2026 Alexis Harabchuk.

Skrivli is licensed under the **GNU General Public License, version 3.0 only (GPL-3.0-only)**.

You may use, modify, and redistribute this software under the terms of the GNU GPLv3.

This software is provided without warranty, as described in the license.

See [LICENSE](LICENSE) for the complete license terms.