//! Steam library and Proton prefix discovery for the installed RotMG client.

use std::collections::HashSet;
use std::path::{Path, PathBuf};

fn vdf_value(text: &str, key: &str) -> Option<String> {
    for line in text.lines() {
        let fields = quoted_fields(line);
        if fields.len() >= 2 && fields[0].eq_ignore_ascii_case(key) {
            return Some(fields[1].clone());
        }
    }
    None
}

fn quoted_fields(line: &str) -> Vec<String> {
    let mut fields = Vec::new();
    let mut chars = line.chars();
    while let Some(ch) = chars.next() {
        if ch != '"' {
            continue;
        }
        let mut field = String::new();
        while let Some(ch) = chars.next() {
            match ch {
                '"' => break,
                '\\' => {
                    if let Some(next) = chars.next() {
                        field.push(next);
                    }
                }
                _ => field.push(ch),
            }
        }
        fields.push(field);
    }
    fields
}

fn resource_files(root: &Path, depth: usize, results: &mut Vec<PathBuf>) {
    if depth == 0 {
        return;
    }
    let Ok(entries) = std::fs::read_dir(root) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_file()
            && path
                .file_name()
                .is_some_and(|name| name == "resources.assets")
        {
            if path.parent().is_some_and(|parent| {
                parent
                    .file_name()
                    .is_some_and(|name| name.to_string_lossy().ends_with("_Data") || name == "Data")
            }) {
                results.push(path);
            }
        } else if path.is_dir() && !path.is_symlink() {
            resource_files(&path, depth - 1, results);
        }
    }
}

fn discover_under(home: &Path, override_dir: Option<&Path>) -> Vec<PathBuf> {
    let mut results = Vec::new();
    if let Some(dir) = override_dir {
        resource_files(dir, 7, &mut results);
        if !results.is_empty() {
            results.sort();
            results.dedup();
            tracing::debug!("[ASSETS] REALMHOUND_GAME_DIR matched {:?}", dir);
            return results;
        }
        tracing::debug!(
            "[ASSETS] REALMHOUND_GAME_DIR={:?} matched nothing, falling back to Steam discovery",
            dir
        );
    }
    let roots = [
        home.join(".steam/steam"),
        home.join(".local/share/Steam"),
        home.join(".var/app/com.valvesoftware.Steam/.local/share/Steam"),
    ];
    let mut libraries = HashSet::new();
    for root in roots {
        if !root.exists() {
            continue;
        }
        libraries.insert(root.clone());
        let vdf = root.join("steamapps/libraryfolders.vdf");
        match std::fs::read_to_string(&vdf) {
            Ok(text) => {
                for line in text.lines() {
                    let fields = quoted_fields(line);
                    if fields.len() >= 2 && fields[0] == "path" {
                        libraries.insert(PathBuf::from(&fields[1]));
                    }
                }
            }
            Err(error) => tracing::debug!("[ASSETS] Skipping unreadable {:?}: {error}", vdf),
        }
    }
    for library in libraries {
        let steamapps = library.join("steamapps");
        let entries = match std::fs::read_dir(&steamapps) {
            Ok(entries) => entries,
            Err(error) => {
                tracing::debug!("[ASSETS] Skipping unreadable {:?}: {error}", steamapps);
                continue;
            }
        };
        for entry in entries.flatten() {
            let path = entry.path();
            let Some(name) = path.file_name().and_then(|n| n.to_str()) else {
                continue;
            };
            if !name.starts_with("appmanifest_") || !name.ends_with(".acf") {
                continue;
            }
            let text = match std::fs::read_to_string(&path) {
                Ok(text) => text,
                Err(error) => {
                    tracing::debug!("[ASSETS] Skipping unreadable {:?}: {error}", path);
                    continue;
                }
            };
            let Some(game_name) = vdf_value(&text, "name") else {
                continue;
            };
            if !game_name
                .to_ascii_lowercase()
                .contains("realm of the mad god")
            {
                continue;
            }
            let Some(install_dir) = vdf_value(&text, "installdir") else {
                continue;
            };
            let Some(appid) = vdf_value(&text, "appid") else {
                continue;
            };
            if install_dir.contains('/') || install_dir.contains('\\') || install_dir == ".." {
                continue;
            }
            resource_files(&steamapps.join("common").join(install_dir), 7, &mut results);
            // The launcher may download Unity assets into Proton's LocalAppData.
            if appid.chars().all(|c| c.is_ascii_digit()) {
                let users = steamapps
                    .join("compatdata")
                    .join(appid)
                    .join("pfx/drive_c/users");
                match std::fs::read_dir(&users) {
                    Ok(entries) => {
                        for user in entries.flatten() {
                            let prefix = user
                                .path()
                                .join("AppData/Local/RealmOfTheMadGod/Production");
                            resource_files(&prefix, 5, &mut results);
                        }
                    }
                    Err(error) => {
                        tracing::debug!("[ASSETS] Skipping unreadable {:?}: {error}", users)
                    }
                }
            }
        }
    }
    results.sort();
    results.dedup();
    results
}

pub(super) fn discover_resources_assets() -> Vec<PathBuf> {
    let Some(home) = dirs::home_dir() else {
        return Vec::new();
    };
    let override_dir = std::env::var_os("REALMHOUND_GAME_DIR").map(PathBuf::from);
    discover_under(&home, override_dir.as_deref())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn finds_multiple_steam_libraries_and_proton_assets() {
        let temp = tempfile::tempdir().unwrap();
        let home = temp.path().join("home");
        let root = home.join(".local/share/Steam");
        let extra = temp.path().join("disk/SteamLibrary");
        std::fs::create_dir_all(root.join("steamapps")).unwrap();
        std::fs::create_dir_all(extra.join("steamapps")).unwrap();
        std::fs::write(
            root.join("steamapps/libraryfolders.vdf"),
            format!(
                "\"libraryfolders\"\n{{\n\"1\"\n{{\n\"path\" \"{}\"\n}}\n}}",
                extra.display()
            ),
        )
        .unwrap();
        std::fs::write(extra.join("steamapps/appmanifest_2000.acf"), "\"AppState\"\n{\n\"appid\" \"2000\"\n\"name\" \"Realm of the Mad God Exalt\"\n\"installdir\" \"RealmGame\"\n}").unwrap();
        let asset = extra.join("steamapps/compatdata/2000/pfx/drive_c/users/steamuser/AppData/Local/RealmOfTheMadGod/Production/RotMG Exalt_Data/resources.assets");
        std::fs::create_dir_all(asset.parent().unwrap()).unwrap();
        std::fs::write(&asset, b"test").unwrap();
        let installed = extra.join("steamapps/common/RealmGame/RotMG Exalt_Data/resources.assets");
        std::fs::create_dir_all(installed.parent().unwrap()).unwrap();
        std::fs::write(&installed, b"test").unwrap();
        let found = discover_under(&home, None);
        assert!(found.contains(&asset));
        assert!(found.contains(&installed));
        let manual = temp.path().join("manual/RotMG Exalt_Data/resources.assets");
        std::fs::create_dir_all(manual.parent().unwrap()).unwrap();
        std::fs::write(&manual, b"test").unwrap();
        assert_eq!(
            discover_under(&home, Some(manual.parent().unwrap().parent().unwrap())),
            vec![manual]
        );
    }
}
