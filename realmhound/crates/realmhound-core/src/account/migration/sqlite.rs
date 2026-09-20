//! SQLite database migration: checkpoint, backup, and validate.
//!
//! This is the only permitted way to migrate a live loot/combat database. It
//! never ordinary-copies a `.db` file, never runs the application writer or
//! initializer against the source or destination, and only produces the final
//! destination file after an independent read-only validation of a
//! migration-owned temporary backup. Because originals are never moved here, an
//! interruption can discard the temp output and retry safely.

use std::path::{Path, PathBuf};

use rusqlite::{Connection, OpenFlags};

use super::MigrationError;

/// Supported version and required tables for a migrated database kind.
#[derive(Debug, Clone, Copy)]
pub struct DbSpec {
    /// Human label used in diagnostics (e.g. `loot_history`).
    pub label: &'static str,
    /// Highest `user_version` this build accepts.
    pub supported_version: i32,
    /// Core tables the database must contain.
    pub required_tables: &'static [&'static str],
}

impl DbSpec {
    /// Spec for the loot-history database.
    pub fn loot() -> Self {
        Self {
            label: "loot_history",
            supported_version: crate::loot::SUPPORTED_SCHEMA_VERSION,
            required_tables: crate::loot::REQUIRED_TABLES,
        }
    }

    /// Spec for the combat-history database.
    pub fn combat() -> Self {
        Self {
            label: "combat_history",
            supported_version: crate::combat::SUPPORTED_SCHEMA_VERSION,
            required_tables: crate::combat::REQUIRED_TABLES,
        }
    }
}

fn wal_sidecar(path: &Path) -> PathBuf {
    let mut name = path.as_os_str().to_os_string();
    name.push("-wal");
    PathBuf::from(name)
}

/// Migrate a SQLite database from `source` to `destination`.
///
/// The caller must already hold the exclusive migration lock and must have
/// closed every application reader/writer. `destination` must not already exist;
/// its parent directory is created. Returns `Ok(())` only after the validated
/// backup has been renamed into place.
pub fn migrate_database(
    source: &Path,
    destination: &Path,
    spec: &DbSpec,
) -> Result<(), MigrationError> {
    if destination.exists() {
        return Err(MigrationError::DatabaseDestinationExists {
            path: destination.to_path_buf(),
        });
    }
    let parent =
        destination
            .parent()
            .ok_or_else(|| MigrationError::DatabaseInvalidDestination {
                path: destination.to_path_buf(),
            })?;
    std::fs::create_dir_all(parent).map_err(|source_err| MigrationError::CopyIo {
        path: parent.to_path_buf(),
        source: source_err,
    })?;

    let temp = unique_temp_path(parent);

    // Scope the source connection so it is fully closed before validation.
    {
        let conn = Connection::open(source).map_err(|source_err| MigrationError::Sqlite {
            label: spec.label,
            detail: format!("open source: {source_err}"),
        })?;
        conn.busy_timeout(std::time::Duration::from_secs(5))
            .map_err(|e| sqlite_error(spec, "set busy_timeout", e))?;

        checkpoint_truncate(&conn, spec)?;
        confirm_wal_consolidated(source, spec)?;
        run_quick_check(&conn, spec)?;
        validate_version(&conn, spec)?;
        validate_required_tables(&conn, spec)?;

        // VACUUM INTO writes a fresh, single-file, fully-consolidated copy.
        let escaped = temp.to_string_lossy().replace('\'', "''");
        conn.execute_batch(&format!("VACUUM INTO '{escaped}'"))
            .map_err(|e| sqlite_error(spec, "vacuum into temporary backup", e))?;
    }

    // Independently validate the temporary backup read-only before installing.
    let validation = validate_backup(&temp, spec);
    if let Err(error) = validation {
        let _ = std::fs::remove_file(&temp);
        return Err(error);
    }

    std::fs::rename(&temp, destination).map_err(|source_err| {
        let _ = std::fs::remove_file(&temp);
        MigrationError::CopyIo {
            path: destination.to_path_buf(),
            source: source_err,
        }
    })?;
    Ok(())
}

fn checkpoint_truncate(conn: &Connection, spec: &DbSpec) -> Result<(), MigrationError> {
    let (busy, log, checkpointed): (i64, i64, i64) = conn
        .query_row("PRAGMA wal_checkpoint(TRUNCATE)", [], |row| {
            Ok((row.get(0)?, row.get(1)?, row.get(2)?))
        })
        .map_err(|e| sqlite_error(spec, "wal_checkpoint(TRUNCATE)", e))?;
    if busy != 0 {
        return Err(MigrationError::DatabaseCheckpointBusy { label: spec.label });
    }
    if log != checkpointed {
        return Err(MigrationError::DatabaseCheckpointIncomplete {
            label: spec.label,
            log,
            checkpointed,
        });
    }
    Ok(())
}

fn confirm_wal_consolidated(source: &Path, spec: &DbSpec) -> Result<(), MigrationError> {
    let wal = wal_sidecar(source);
    match std::fs::metadata(&wal) {
        Ok(metadata) if metadata.len() == 0 => Ok(()),
        Ok(_) => Err(MigrationError::DatabaseWalRemains { label: spec.label }),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(MigrationError::CopyIo {
            path: wal,
            source: error,
        }),
    }
}

fn run_quick_check(conn: &Connection, spec: &DbSpec) -> Result<(), MigrationError> {
    let result: String = conn
        .query_row("PRAGMA quick_check", [], |row| row.get(0))
        .map_err(|e| sqlite_error(spec, "quick_check", e))?;
    if result != "ok" {
        return Err(MigrationError::DatabaseIntegrity {
            label: spec.label,
            detail: result,
        });
    }
    Ok(())
}

fn validate_version(conn: &Connection, spec: &DbSpec) -> Result<(), MigrationError> {
    let version: i32 = conn
        .query_row("PRAGMA user_version", [], |row| row.get(0))
        .map_err(|e| sqlite_error(spec, "user_version", e))?;
    if version > spec.supported_version {
        return Err(MigrationError::DatabaseVersionUnsupported {
            label: spec.label,
            found: version,
            supported: spec.supported_version,
        });
    }
    Ok(())
}

fn validate_required_tables(conn: &Connection, spec: &DbSpec) -> Result<(), MigrationError> {
    for table in spec.required_tables {
        let present: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master WHERE type = 'table' AND name = ?1",
                [table],
                |row| row.get(0),
            )
            .map_err(|e| sqlite_error(spec, "inspect schema", e))?;
        if present == 0 {
            return Err(MigrationError::DatabaseMissingTable {
                label: spec.label,
                table: (*table).to_string(),
            });
        }
    }
    Ok(())
}

fn validate_backup(temp: &Path, spec: &DbSpec) -> Result<(), MigrationError> {
    let conn = Connection::open_with_flags(
        temp,
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )
    .map_err(|e| sqlite_error(spec, "open backup read-only", e))?;
    run_quick_check(&conn, spec)?;
    validate_version(&conn, spec)?;
    validate_required_tables(&conn, spec)?;
    Ok(())
}

/// Read-only validation of a migrated/backup SQLite file against a [`DbSpec`].
///
/// Opens the file read-only and runs `quick_check`, the supported-version gate,
/// and the required-table check. Used to revalidate a completed destination or a
/// backup copy without trusting a journal `validated` flag.
pub fn validate_database_file(path: &Path, spec: &DbSpec) -> Result<(), MigrationError> {
    if !path.exists() {
        return Err(MigrationError::DatabaseInvalidDestination {
            path: path.to_path_buf(),
        });
    }
    validate_backup(path, spec)
}

/// Return the [`DbSpec`] for a database inventory id, if it is a known database.
pub fn spec_for(item_id: &str) -> Option<DbSpec> {
    match item_id {
        "loot_history.db" => Some(DbSpec::loot()),
        "combat_history.db" => Some(DbSpec::combat()),
        _ => None,
    }
}

fn unique_temp_path(parent: &Path) -> PathBuf {
    let name = format!(
        ".migration-{}-{:016x}.db",
        std::process::id(),
        rand::random::<u64>()
    );
    parent.join(name)
}

fn sqlite_error(spec: &DbSpec, operation: &str, error: rusqlite::Error) -> MigrationError {
    MigrationError::Sqlite {
        label: spec.label,
        detail: format!("{operation}: {error}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn seed_loot_like(path: &Path, version: i32) {
        let conn = Connection::open(path).unwrap();
        conn.execute_batch(
            "PRAGMA journal_mode=WAL;
             CREATE TABLE loot_drops (id INTEGER PRIMARY KEY);
             CREATE TABLE loot_items (id INTEGER PRIMARY KEY);
             INSERT INTO loot_drops (id) VALUES (1);",
        )
        .unwrap();
        conn.execute_batch(&format!("PRAGMA user_version = {version}"))
            .unwrap();
    }

    #[test]
    fn migrates_valid_database_into_destination() {
        let temp = tempfile::tempdir().unwrap();
        let source = temp.path().join("loot_history.db");
        seed_loot_like(&source, 5);
        let dest = temp.path().join("out").join("loot_history.db");

        migrate_database(&source, &dest, &DbSpec::loot()).unwrap();

        assert!(dest.exists());
        let conn = Connection::open(&dest).unwrap();
        let count: i64 = conn
            .query_row("SELECT COUNT(*) FROM loot_drops", [], |r| r.get(0))
            .unwrap();
        assert_eq!(count, 1);
    }

    #[test]
    fn rejects_future_version_database() {
        let temp = tempfile::tempdir().unwrap();
        let source = temp.path().join("loot_history.db");
        seed_loot_like(&source, crate::loot::SUPPORTED_SCHEMA_VERSION + 1);
        let dest = temp.path().join("out.db");

        let error = migrate_database(&source, &dest, &DbSpec::loot()).unwrap_err();
        assert!(matches!(
            error,
            MigrationError::DatabaseVersionUnsupported { .. }
        ));
        assert!(!dest.exists());
    }

    #[test]
    fn rejects_missing_required_table() {
        let temp = tempfile::tempdir().unwrap();
        let source = temp.path().join("loot_history.db");
        let conn = Connection::open(&source).unwrap();
        conn.execute_batch("CREATE TABLE loot_drops (id INTEGER PRIMARY KEY);")
            .unwrap();
        drop(conn);
        let dest = temp.path().join("out.db");

        let error = migrate_database(&source, &dest, &DbSpec::loot()).unwrap_err();
        assert!(matches!(error, MigrationError::DatabaseMissingTable { .. }));
    }

    #[test]
    fn commits_rows_that_live_only_in_the_wal() {
        let temp = tempfile::tempdir().unwrap();
        let source = temp.path().join("loot_history.db");
        seed_loot_like(&source, 5);
        // Add a row on a second connection and leave it in the WAL by not
        // checkpointing before the migration runs.
        {
            let conn = Connection::open(&source).unwrap();
            conn.execute_batch("PRAGMA wal_autocheckpoint=0;").unwrap();
            conn.execute("INSERT INTO loot_drops (id) VALUES (2)", [])
                .unwrap();
            // Connection dropped without an explicit checkpoint.
        }
        let dest = temp.path().join("out.db");
        migrate_database(&source, &dest, &DbSpec::loot()).unwrap();

        let conn = Connection::open(&dest).unwrap();
        let count: i64 = conn
            .query_row("SELECT COUNT(*) FROM loot_drops", [], |r| r.get(0))
            .unwrap();
        assert_eq!(count, 2);
    }

    #[test]
    fn refuses_existing_destination() {
        let temp = tempfile::tempdir().unwrap();
        let source = temp.path().join("loot_history.db");
        seed_loot_like(&source, 5);
        let dest = temp.path().join("out.db");
        std::fs::write(&dest, b"existing").unwrap();

        let error = migrate_database(&source, &dest, &DbSpec::loot()).unwrap_err();
        assert!(matches!(
            error,
            MigrationError::DatabaseDestinationExists { .. }
        ));
    }
}
