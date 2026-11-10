# Building locally

Target: `x86_64-pc-windows-msvc` on Windows 10 or 11.

1. Install [rustup](https://rustup.rs/) and the stable toolchain (`rust-toolchain.toml` selects `stable`). If `cargo` is not on `PATH`, prepend the rustup bin directory:
   ```powershell
    $env:Path = "$env:USERPROFILE\.cargo\bin;" + $env:Path
   ```
2. Install the MSVC Build Tools (Visual Studio Build Tools workload "Desktop development with C++") and a Windows 10/11 SDK. The linker is `link.exe` from that toolchain.
3. Install [CMake](https://cmake.org/) (3.15 or newer) and Git. `build.rs` builds the speech engine from the `third_party/parakeet.cpp` submodule, so clone with `--recurse-submodules` or run `git submodule update --init --recursive`. The first build compiles parakeet.cpp and ggml and takes a few minutes. No libclang is needed.

## Commands

```powershell
cargo fmt --all -- --check
cargo clippy --all-targets -- -D warnings
cargo test
cargo build --release
cargo run
```

`cargo test` does not use a microphone, a GPU device check, or an interactive window. `cargo run` opens the Skrivli window.

`scripts\build_run.bat` stops a running `skrivli.exe`, builds a debug binary and launches it.
