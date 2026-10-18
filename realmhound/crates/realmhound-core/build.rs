fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-changed=wpcap.def");

    let target = std::env::var("TARGET").expect("Cargo must set TARGET");
    if target.ends_with("-windows-msvc") {
        #[cfg(windows)]
        generate_wpcap_import_library(&target);
        #[cfg(not(windows))]
        panic!("Generating the wpcap import library requires a Windows host with MSVC build tools");
    }
}

#[cfg(windows)]
fn generate_wpcap_import_library(target: &str) {
    use std::path::PathBuf;

    let machine = match std::env::var("CARGO_CFG_TARGET_ARCH").as_deref() {
        Ok("x86_64") => "X64",
        Ok("aarch64") => "ARM64",
        Ok("x86") => "X86",
        arch => panic!("Unsupported Windows architecture for wpcap: {arch:?}"),
    };
    let out_dir = PathBuf::from(std::env::var_os("OUT_DIR").expect("Cargo must set OUT_DIR"));
    let mut librarian = find_msvc_tools::find(target, "lib.exe").expect(
        "lib.exe was not found; install Visual Studio Build Tools with \
         Desktop development with C++ and a Windows SDK",
    );
    let output = librarian
        .arg("/nologo")
        .arg(format!("/machine:{machine}"))
        .arg("/def:wpcap.def")
        .arg(format!("/out:{}", out_dir.join("wpcap.lib").display()))
        .output()
        .expect("Failed to run lib.exe to generate the wpcap import library");

    assert!(
        output.status.success(),
        "lib.exe failed to generate the wpcap import library ({}):\n{}{}",
        output.status,
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    println!("cargo:rustc-link-search=native={}", out_dir.display());
}
