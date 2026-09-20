use std::env;
use std::path::Path;

const OFFICIAL_SOUND_PATHS: [&str; 9] = [
    "assets/sounds/bad_mod_warning.mp3",
    "assets/sounds/dimitus.mp3",
    "assets/sounds/enchants/money.mp3",
    "assets/sounds/events/adept.mp3",
    "assets/sounds/events/alien_wave.mp3",
    "assets/sounds/events/calbrik.mp3",
    "assets/sounds/events/seasonal.mp3",
    "assets/sounds/events/ufo_.mp3",
    "assets/sounds/events/veteran.mp3",
];

fn main() {
    println!("cargo:rustc-check-cfg=cfg(realmhound_official_sounds)");
    println!("cargo:rerun-if-env-changed=REALMHOUND_OFFICIAL_SOUNDS");
    println!("cargo:rerun-if-changed=assets/sounds");
    println!("cargo:rerun-if-changed=assets/icon.ico");

    match env::var("REALMHOUND_OFFICIAL_SOUNDS") {
        Err(env::VarError::NotPresent) => reject_private_sounds_in_public_build(),
        Ok(value) if value == "1" => enable_official_sounds(),
        Ok(value) => {
            panic!("REALMHOUND_OFFICIAL_SOUNDS must be unset or equal to 1, got {value:?}")
        }
        Err(env::VarError::NotUnicode(_)) => {
            panic!("REALMHOUND_OFFICIAL_SOUNDS must contain valid Unicode")
        }
    }

    #[cfg(windows)]
    compile_windows_resources();
}

fn enable_official_sounds() {
    let missing: Vec<_> = OFFICIAL_SOUND_PATHS
        .iter()
        .filter(|path| !Path::new(path).is_file())
        .collect();

    if !missing.is_empty() {
        panic!(
            "official sound build is missing required files:\n{}",
            missing
                .iter()
                .map(|path| format!("  {path}"))
                .collect::<Vec<_>>()
                .join("\n")
        );
    }

    println!("cargo:rustc-cfg=realmhound_official_sounds");
}

fn reject_private_sounds_in_public_build() {
    let present: Vec<_> = OFFICIAL_SOUND_PATHS
        .iter()
        .filter(|path| Path::new(path).exists())
        .collect();

    if !present.is_empty() {
        panic!(
            "private official sound files are present in a public build:\n{}\n\
             set REALMHOUND_OFFICIAL_SOUNDS=1 only when building an official release",
            present
                .iter()
                .map(|path| format!("  {path}"))
                .collect::<Vec<_>>()
                .join("\n")
        );
    }
}

#[cfg(windows)]
fn compile_windows_resources() {
    let mut res = winres::WindowsResource::new();
    res.set_icon("assets/icon.ico");

    if let Err(error) = res.compile() {
        eprintln!("Warning: Failed to compile Windows resources: {error}");
    }
}
