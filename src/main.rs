#![windows_subsystem = "windows"]

fn main() {
    let code = skrivli::run();
    std::process::exit(code);
}
