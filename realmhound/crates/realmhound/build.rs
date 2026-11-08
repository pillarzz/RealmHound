use std::collections::BTreeMap;
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
    validate_sets();
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

fn validate_sets() {
    let root = Path::new("../../../data");
    for file in [
        "rotmg-sets.json",
        "rotmg-sets-sources.json",
        "rotmg-item-sources.json",
        "rotmg-set-item-ids.json",
    ] {
        println!("cargo:rerun-if-changed={}", root.join(file).display());
    }
    let read = |file: &str| -> serde_json::Value {
        let path = root.join(file);
        serde_json::from_slice(
            &std::fs::read(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display())),
        )
        .unwrap_or_else(|e| panic!("{}: invalid JSON: {e}", path.display()))
    };
    let sets = read("rotmg-sets.json");
    let meta = read("rotmg-sets-sources.json");
    let sources = read("rotmg-item-sources.json");
    let ids = read("rotmg-set-item-ids.json");
    let mut issues = Vec::new();
    let mut names = BTreeMap::new();
    for (set, pieces) in sets.as_object().expect("sets must be an object") {
        let Some(pieces) = pieces.as_array() else {
            issues.push(format!("{set}: items are not an array"));
            continue;
        };
        if pieces.len() != 4 {
            issues.push(format!(
                "{set}: expected four slots, found {}",
                pieces.len()
            ));
        }
        for (slot, piece) in pieces.iter().enumerate() {
            let Some(name) = piece.as_str() else {
                issues.push(format!("{set} slot {slot}: item name is not text"));
                continue;
            };
            names.insert(name, (set, slot));
            if ids[name]
                .as_i64()
                .is_none_or(|id| id <= 0 || id > i32::MAX as i64)
            {
                issues.push(format!(
                    "{set} slot {slot}: {name}: unresolved canonical item ID"
                ));
            }
            if sources["items"][name].is_null() {
                issues.push(format!("{set} slot {slot}: {name}: missing source entry"));
            }
        }
    }
    for (variant, base) in meta["alternate_ring_entries"]
        .as_object()
        .expect("alternate_ring_entries must be an object")
    {
        let Some(base) = base.as_str() else {
            issues.push(format!("{variant}: invalid base set"));
            continue;
        };
        if sets[variant].is_null() || sets[base].is_null() {
            issues.push(format!("{variant}: base or variant entry missing"));
            continue;
        }
        if sets[variant].as_array().unwrap()[..3] != sets[base].as_array().unwrap()[..3] {
            issues.push(format!("{variant}: differs outside jewelry slot"));
        }
    }
    if !issues.is_empty() {
        panic!("set data validation failed:\n{}", issues.join("\n"));
    }
    println!(
        "cargo:warning=Validated {} set item names against reviewed IDs and source records",
        names.len()
    );
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
