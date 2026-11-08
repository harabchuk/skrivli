fn main() {
    println!("cargo:rerun-if-changed=resources/skrivli.ico");
    println!("cargo:rerun-if-changed=resources/skrivli.manifest");
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("windows") {
        return;
    }

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
