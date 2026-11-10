use std::path::{Path, PathBuf};

/// Static libraries the parakeet.cpp CMake build produces and the app links.
const NATIVE_LIBS: [&str; 4] = ["parakeet", "ggml", "ggml-base", "ggml-cpu"];

fn main() {
    println!("cargo:rerun-if-changed=resources/skrivli.ico");
    println!("cargo:rerun-if-changed=resources/skrivli.manifest");
    println!("cargo:rerun-if-changed=third_party/parakeet.cpp/CMakeLists.txt");
    println!("cargo:rerun-if-changed=third_party/parakeet.cpp/include/parakeet_capi.h");
    println!("cargo:rerun-if-changed=third_party/parakeet.cpp/src/parakeet_capi.cpp");
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("windows") {
        return;
    }

    build_parakeet();

    // PerMonitorV2 has to be in the process manifest before GPUI creates a
    // window. Calling SetProcessDpiAwareness afterwards is ignored.
    // gpui-pre already embeds that manifest (RT_MANIFEST id 1). A second
    // manifest from this crate makes the MSVC linker fail with CVT1100, so
    // resources/skrivli.manifest is the checked-in contract and is not embedded.
    let mut resources = winresource::WindowsResource::new();
    resources.set_icon("resources/skrivli.ico");
    resources.set("FileDescription", "Local Windows meeting recorder");
    resources.set("ProductName", "Skrivli");
    resources.set("OriginalFilename", "skrivli.exe");
    resources.set("InternalName", "skrivli");
    if let Err(error) = resources.compile() {
        eprintln!("failed to embed Windows resources: {error}");
        std::process::exit(1);
    }
}

/// Builds parakeet.cpp (pinned submodule, see .specs/decisions.md D-33) as static,
/// CPU-only libraries that use the static MSVC runtime, matching `+crt-static`.
fn build_parakeet() {
    let mut config = cmake::Config::new("third_party/parakeet.cpp");
    config
        // Rust always links the release CRT, so a Debug build must not use /MTd.
        .profile("Release")
        .static_crt(true)
        // The cmake crate derives the per-config flags from the cargo profile, so a dev
        // build would compile the engine without optimisation (4x slower than realtime).
        .define("CMAKE_C_FLAGS_RELEASE", "/O2 /Ob2 /DNDEBUG")
        .define("CMAKE_CXX_FLAGS_RELEASE", "/O2 /Ob2 /DNDEBUG")
        .define("CMAKE_MSVC_RUNTIME_LIBRARY", "MultiThreaded")
        // parakeet.cpp and ggml declare an old cmake_minimum_required.
        .define("CMAKE_POLICY_DEFAULT_CMP0091", "NEW")
        .define("CMAKE_POLICY_VERSION_MINIMUM", "3.5")
        .define("BUILD_SHARED_LIBS", "OFF")
        .define("PARAKEET_SHARED", "OFF")
        .define("PARAKEET_BUILD_CLI", "OFF")
        .define("PARAKEET_BUILD_SERVER", "OFF")
        .define("PARAKEET_BUILD_TESTS", "OFF")
        .define("PARAKEET_WITH_CED", "OFF")
        .define("PARAKEET_WITH_VOICEDETECT", "OFF")
        .define("GGML_NATIVE", "OFF")
        // OpenMP would import vcomp140.dll; ggml's own thread pool is enough.
        .define("GGML_OPENMP", "OFF")
        .define("GGML_BUILD_TESTS", "OFF")
        .define("GGML_BUILD_EXAMPLES", "OFF");
    let out = config.build();

    let mut dirs = Vec::new();
    collect_lib_dirs(&out.join("build"), &mut dirs);
    for dir in &dirs {
        println!("cargo:rustc-link-search=native={}", dir.display());
    }
    for lib in NATIVE_LIBS {
        println!("cargo:rustc-link-lib=static={lib}");
    }
}

/// Directories under the CMake build tree that hold `.lib` files.
fn collect_lib_dirs(dir: &Path, found: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    let mut has_lib = false;
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            collect_lib_dirs(&path, found);
        } else if path.extension().is_some_and(|ext| ext == "lib") {
            has_lib = true;
        }
    }
    if has_lib {
        found.push(dir.to_path_buf());
    }
}
