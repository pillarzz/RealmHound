//! Linux installation layout and install-kind detection.
//!
//! The official Linux installer keeps the GUI in a user-owned directory and
//! the capability-bearing capture helper in a root-owned directory. This
//! module names both layouts and identifies which kind of installation is
//! running, so the updater can refuse to replace package-managed or unmanaged
//! files. It never grants privilege: the marker only identifies an install.

#![cfg_attr(not(target_os = "linux"), allow(dead_code))]

use std::path::{Path, PathBuf};

/// User-owned GUI directory relative to `$HOME`.
pub const GUI_DIR_FROM_HOME: &str = ".local/opt/realmhound";
/// GUI executable name inside that directory.
pub const GUI_BINARY_NAME: &str = "RealmHound";
/// Stable launcher name installed in `~/.local/bin`.
pub const LAUNCHER_NAME: &str = "realmhound";
/// Marker created by the official installer.
pub const INSTALL_MARKER_NAME: &str = "install.json";
/// Marker value identifying an official user-owned install.
pub const OFFICIAL_LAYOUT: &str = "official-user";

/// Protected directory containing the capture helper.
pub const HELPER_DIR: &str = "/usr/local/libexec/realmhound";
/// Capture helper file name.
pub const HELPER_BINARY_NAME: &str = "realmhound-capture";
/// Canonical protected helper path.
pub const HELPER_CANONICAL_PATH: &str = "/usr/local/libexec/realmhound/realmhound-capture";
/// Previous official helper location, retained for migration and old installs.
pub const HELPER_LEGACY_PATH: &str = "/usr/local/bin/realmhound-capture";
/// Explicit override for unusual layouts; must be an absolute path.
pub const HELPER_ENV_OVERRIDE: &str = "REALMHOUND_CAPTURE_HELPER";

/// How this copy of RealmHound was installed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InstallKind {
    /// The official user-owned installation created by the Linux installer.
    OfficialUser,
    /// Owned by a distribution package manager. Never replace it upstream.
    PackageManaged,
    /// A source build or an unrecognized layout. No in-app replacement.
    Unmanaged,
}

impl InstallKind {
    /// A short user-facing explanation when this install cannot self-update.
    pub fn update_hint(self) -> Option<&'static str> {
        match self {
            Self::OfficialUser => None,
            Self::PackageManaged => {
                Some("RealmHound is managed by your package manager. Update it there.")
            }
            Self::Unmanaged => Some(
                "This RealmHound was not installed by the official Linux installer. \
                 Reinstall it with the official release installer to enable in-app updates.",
            ),
        }
    }
}

/// The user-owned GUI directory for this account, if `$HOME` is known.
pub fn official_gui_dir() -> Option<PathBuf> {
    dirs::home_dir().map(|home| home.join(GUI_DIR_FROM_HOME))
}

/// Detect how the running copy of RealmHound was installed.
pub fn detect_install_kind() -> InstallKind {
    #[cfg(target_os = "linux")]
    {
        if cfg!(feature = "package-build") {
            return InstallKind::PackageManaged;
        }
        let Ok(exe) = std::env::current_exe() else {
            return InstallKind::Unmanaged;
        };
        detect_install_kind_at(&exe, official_gui_dir().as_deref(), package_owned(&exe))
    }
    #[cfg(not(target_os = "linux"))]
    {
        InstallKind::Unmanaged
    }
}

/// Classify an installation from its inputs. Kept pure so every branch can be
/// tested without touching the real installation.
fn detect_install_kind_at(
    exe: &Path,
    official_dir: Option<&Path>,
    package_owned: bool,
) -> InstallKind {
    let in_official_layout = official_dir
        .map(|dir| same_file(exe, &dir.join(GUI_BINARY_NAME)) && marker_is_official(dir))
        .unwrap_or(false);
    classify(package_owned, in_official_layout)
}

fn classify(package_owned: bool, in_official_layout: bool) -> InstallKind {
    if package_owned {
        return InstallKind::PackageManaged;
    }
    if in_official_layout {
        return InstallKind::OfficialUser;
    }
    InstallKind::Unmanaged
}

/// Whether `dir` carries the official installer marker.
pub fn marker_is_official(dir: &Path) -> bool {
    #[derive(serde::Deserialize)]
    struct Marker {
        layout: String,
    }

    std::fs::read(dir.join(INSTALL_MARKER_NAME))
        .ok()
        .and_then(|bytes| serde_json::from_slice::<Marker>(&bytes).ok())
        .map(|marker| marker.layout == OFFICIAL_LAYOUT)
        .unwrap_or(false)
}

fn same_file(left: &Path, right: &Path) -> bool {
    let left = std::fs::canonicalize(left).unwrap_or_else(|_| left.to_path_buf());
    let right = std::fs::canonicalize(right).unwrap_or_else(|_| right.to_path_buf());
    left == right
}

/// Whether a distribution package manager owns `path`.
///
/// Directory writability is not an ownership signal: a package-installed file
/// can live in a writable location and a user-owned build can live in a
/// protected one. Ask the package manager instead.
fn package_owned(path: &Path) -> bool {
    let path = path.to_string_lossy().into_owned();
    command_succeeds("dpkg-query", &["-S", &path])
        || command_succeeds("rpm", &["-qf", &path])
        || command_succeeds("pacman", &["-Qo", &path])
}

fn command_succeeds(program: &str, args: &[&str]) -> bool {
    std::process::Command::new(program)
        .args(args)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .map(|status| status.success())
        .unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write_marker(dir: &Path, layout: &str) {
        std::fs::create_dir_all(dir).unwrap();
        std::fs::write(
            dir.join(INSTALL_MARKER_NAME),
            format!(r#"{{"layout":"{layout}"}}"#),
        )
        .unwrap();
    }

    #[test]
    fn package_ownership_wins_over_a_marker() {
        assert_eq!(classify(true, false), InstallKind::PackageManaged);
        assert_eq!(classify(true, true), InstallKind::PackageManaged);
    }

    #[test]
    fn only_marked_official_layouts_self_update() {
        assert_eq!(classify(false, true), InstallKind::OfficialUser);
        assert_eq!(classify(false, false), InstallKind::Unmanaged);
    }

    #[test]
    fn a_source_build_is_not_an_official_install() {
        let temp = tempfile::tempdir().unwrap();
        let dir = temp.path().join("realmhound");
        write_marker(&dir, OFFICIAL_LAYOUT);
        let exe = temp.path().join("target/release").join(GUI_BINARY_NAME);
        std::fs::create_dir_all(exe.parent().unwrap()).unwrap();
        std::fs::write(&exe, b"gui").unwrap();

        assert_eq!(
            detect_install_kind_at(&exe, Some(&dir), false),
            InstallKind::Unmanaged
        );
    }

    #[test]
    fn a_marked_official_binary_is_recognised() {
        let temp = tempfile::tempdir().unwrap();
        let dir = temp.path().join("realmhound");
        write_marker(&dir, OFFICIAL_LAYOUT);
        let exe = dir.join(GUI_BINARY_NAME);
        std::fs::write(&exe, b"gui").unwrap();

        assert_eq!(
            detect_install_kind_at(&exe, Some(&dir), false),
            InstallKind::OfficialUser
        );
    }

    #[test]
    fn an_unknown_marker_layout_is_not_official() {
        let temp = tempfile::tempdir().unwrap();
        let dir = temp.path().join("realmhound");
        write_marker(&dir, "something-else");
        let exe = dir.join(GUI_BINARY_NAME);
        std::fs::write(&exe, b"gui").unwrap();

        assert_eq!(
            detect_install_kind_at(&exe, Some(&dir), false),
            InstallKind::Unmanaged
        );
    }

    #[test]
    fn unmanaged_installs_explain_how_to_enable_updates() {
        assert!(InstallKind::OfficialUser.update_hint().is_none());
        assert!(InstallKind::PackageManaged
            .update_hint()
            .unwrap()
            .contains("package manager"));
        assert!(InstallKind::Unmanaged
            .update_hint()
            .unwrap()
            .contains("official release installer"));
    }
}
