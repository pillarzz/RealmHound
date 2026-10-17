use std::path::Path;

fn game_process_present_in(proc_root: &Path) -> bool {
    let Ok(entries) = std::fs::read_dir(proc_root) else {
        return false;
    };
    entries.flatten().any(|entry| {
        if !entry
            .file_name()
            .to_string_lossy()
            .chars()
            .all(|c| c.is_ascii_digit())
        {
            return false;
        }
        let Ok(command) = std::fs::read(entry.path().join("cmdline")) else {
            return false;
        };
        let command = String::from_utf8_lossy(&command).to_ascii_lowercase();
        command.contains("rotmg exalt")
            || command.contains("rotmgexalt")
            || command.contains("realmofthemadgod")
    })
}

pub fn game_process_present() -> bool {
    use std::sync::{Mutex, OnceLock};
    use std::time::{Duration, Instant};
    static CACHE: OnceLock<Mutex<(Instant, bool)>> = OnceLock::new();
    let cache = CACHE.get_or_init(|| Mutex::new((Instant::now() - Duration::from_secs(2), false)));
    let mut state = cache.lock().unwrap();
    if state.0.elapsed() >= Duration::from_secs(1) {
        *state = (Instant::now(), game_process_present_in(Path::new("/proc")));
    }
    state.1
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn absent_game_is_reported_without_error() {
        let root = tempfile::tempdir().unwrap();
        assert!(!game_process_present_in(root.path()));
        std::fs::create_dir(root.path().join("123")).unwrap();
        std::fs::write(root.path().join("123/cmdline"), b"wine\0RotMG Exalt.exe\0").unwrap();
        assert!(game_process_present_in(root.path()));
    }
}
