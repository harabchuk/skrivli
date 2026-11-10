# Skrivli

[![CI](https://github.com/harabchuk/skrivli/actions/workflows/ci.yml/badge.svg)](https://github.com/harabchuk/skrivli/actions/workflows/ci.yml)
[![Release](https://img.shields.io/github/v/release/harabchuk/skrivli)](https://github.com/harabchuk/skrivli/releases/latest)
[![Downloads](https://img.shields.io/github/downloads/harabchuk/skrivli/total)](https://github.com/harabchuk/skrivli/releases)
[![License](https://img.shields.io/github/license/harabchuk/skrivli?color=green)](LICENSE)
[![Platform](https://img.shields.io/badge/platform-Windows%2010%20%7C%2011-0078D6?logo=windows)](https://github.com/harabchuk/skrivli/releases/latest)
[![Rust](https://img.shields.io/badge/rust-stable-green?logo=rust)](rust-toolchain.toml)
[![Last commit](https://img.shields.io/github/last-commit/harabchuk/skrivli)](https://github.com/harabchuk/skrivli/commits/main)
[![Issues](https://img.shields.io/github/issues/harabchuk/skrivli)](https://github.com/harabchuk/skrivli/issues)
[![PRs welcome](https://img.shields.io/badge/PRs-welcome-brightgreen)](https://github.com/harabchuk/skrivli/pulls)

Skrivli is a Windows app that records and transcribes meetings entirely on your computer, keeping your audio and text private.

## Download and installation

Skrivli is distributed as a portable Windows application in a ZIP archive.

**Install**
1. Download from the [Releases](https://github.com/harabchuk/skrivli/releases) page.
1. Unzip the whole folder anywhere you like.
1. Run skrivli.exe. There is no installer.

## Features

- **Records both sides of a call.** Your microphone and system audio go into one continuous FLAC file (lossless, 16-bit stereo 48 kHz). Silence is kept, so the timing stays correct.
- **Echo removal.** When system audio is captured, speaker echo is removed from the microphone automatically.
- **Live level meters** for each source, with peak hold and a manual microphone gain slider.
- **Device selection.** Pick any Windows microphone or output device. Devices are locked while recording.
- **Start and stop anywhere:** from the window, the system tray, or a global hotkey (default `Ctrl+Alt+R`).
- **Tray mode.** Hide the window to the tray and the recording keeps running.
- **Reliable saving.** If something fails during a recording, the audio captured so far is saved. You get a warning when disk space runs low, and a recording is still saved if one device drops.
- **Recordings library** grouped by Today, Yesterday and Earlier. It updates live when files change, and has playback with a seek bar, rename, delete to the Recycle Bin, and Show in folder.
- **Offline transcription** with parakeet.cpp and your choice of two NVIDIA models: Nemotron 3.5 (40 languages) or Parakeet TDT v3 (25 European languages), both including English and Swedish, with automatic language detection. A model is a one-time download of about 940 to 984 MB, only when you ask. Transcripts have `[HH:MM:SS]` timestamps, are saved as `.txt` files next to the audio, can be copied with one click, and highlight the current paragraph during playback.
- **Light, dark and system themes**, keyboard navigation, single instance, and the window remembers its size and position.

## Screenshots

### Record

![Record view](.assets/record.png)

### Recordings

![Recordings view](.assets/recordings.png)

## For developers

### Contribution

PRs are welcome. Each PR should fix only one issue and contain human readable explanation of the issue and the way it has been fixed.

For new features PRs please explain the problem and the goal from user perspective.

### Local build

Target: `x86_64-pc-windows-msvc` on Windows 10 or 11.

1. Install [rustup](https://rustup.rs/) and the stable toolchain (`rust-toolchain.toml` selects `stable`). If `cargo` is not on `PATH`, prepend the rustup bin directory:
   ```powershell
    $env:Path = "$env:USERPROFILE\.cargo\bin;" + $env:Path
   ```
2. Install the MSVC Build Tools (Visual Studio Build Tools workload "Desktop development with C++") and a Windows 10/11 SDK. The linker is `link.exe` from that toolchain.
3. Install [CMake](https://cmake.org/) (3.15 or newer) and Git. `build.rs` builds the speech engine from the `third_party/parakeet.cpp` submodule, so clone with `--recurse-submodules` or run `git submodule update --init --recursive`. The first build compiles parakeet.cpp and ggml and takes a few minutes. No libclang is needed.

### Commands

```powershell
cargo fmt --all -- --check
cargo clippy --all-targets -- -D warnings
cargo test
cargo build --release
cargo run
```

`cargo test` does not use a microphone, a GPU device check, or an interactive window. `cargo run` opens the Skrivli window.

### Release process

Releases are cut with [`cargo-release`](https://github.com/crate-ci/cargo-release) (configured in `[package.metadata.release]` in `Cargo.toml`) and built and published by GitHub Actions.

#### Before you start

- Be on a clean, up-to-date `main`. `cargo-release` refuses to run from any other branch.
- Install the tool once: `cargo install cargo-release --locked --version 1.1.7`.

#### Commands

```powershell
# Preview: shows what would change, does not modify anything
cargo release patch

# Do it: bump, commit, tag and push
cargo release patch --execute
```

Use `minor` or `major` instead of `patch` to bump a different part of the version.

#### What `cargo release` does

1. Runs `cargo clippy --all-targets --locked -- -D warnings` as a pre-release hook. A warning stops the release.
2. Bumps `version` in `Cargo.toml` (and `Cargo.lock`).
3. Commits the bump as `Release vX.Y.Z`.
4. Creates the annotated tag `vX.Y.Z` with the message `Skrivli X.Y.Z`.
5. Pushes the commit and the tag. It does not publish to crates.io.

#### What happens after the tag is pushed

Pushing a tag like `vX.Y.Z` starts the `release` workflow (`.github/workflows/release.yml`) on a Windows runner:

1. **CI.** Runs the full CI workflow first (format, clippy, tests, release build). If it fails, nothing is published.
2. **Version check.** Fails if the tag does not match the `Cargo.toml` version.
3. **Build.** `cargo build --release --locked`, then checks that the exe's `ProductVersion` and `FileVersionRaw` equal the version.
4. **Licenses.** Generates `releases/licenses/THIRD-PARTY.txt` with `cargo about`.
5. **Package.** `scripts/package.ps1` builds `target/dist/skrivli-X.Y.Z-windows-x64.zip` and its `.sha256` file. The ZIP holds `skrivli.exe`, `README.txt`, `PRIVACY.txt`, the licenses and the third-party notices. It must not contain `.bin`, `.gguf` or `.pdb` files. If `SKRIVLI_SIGN_CMD` is set, the exe is signed first (not used yet).
6. **Release notes.** Takes `releases/release-notes-header.md` (install steps, SmartScreen note, SHA-256) and adds the commit subjects since the previous tag, with merge commits left out.
7. **Publish.** Creates the GitHub release `Skrivli X.Y.Z` immediately (not a draft), marked as latest, with the ZIP and checksum attached. If the release already exists, the assets are replaced instead.
8. **Artifact.** Also uploads the ZIP and checksum as a workflow artifact, kept for 7 days.

To test packaging locally, run `cargo build --release` and then `./releases/package.ps1 -Version X.Y.Z -AllowMissingThirdParty`. The result contains a placeholder third-party file and must not be released.

## License

Skrivli is an open-source meeting recording and transcription application.

Copyright © 2026 Alexis Harabchuk.

Skrivli is licensed under the **GNU General Public License, version 3.0 only (GPL-3.0-only)**.

You may use, modify, and redistribute this software under the terms of the GNU GPLv3.

This software is provided without warranty, as described in the license.

See [LICENSE](LICENSE) for the complete license terms.