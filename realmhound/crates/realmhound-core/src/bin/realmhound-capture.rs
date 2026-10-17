#[cfg(target_os = "linux")]
fn main() {
    if let Err(error) = realmhound_core::capture::linux_helper::run() {
        eprintln!("Capture helper: {error}");
        std::process::exit(1);
    }
}

#[cfg(not(target_os = "linux"))]
fn main() {
    eprintln!("realmhound-capture is supported only on Linux");
    std::process::exit(1);
}
