//! Content validators and durable, verified copy operations for migration.
//!
//! Every copied source is proven durable before its original is moved:
//! structured files must parse, opaque files must hash-match, and opaque
//! directories must have an equal recursive tree manifest. The RealmShark
//! validator is intentionally syntactic and independent of the asset globals so
//! migration never depends on loaded game assets.

use std::collections::BTreeMap;
use std::fs::{self, File};
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};

use sha2::{Digest, Sha256};

use super::inventory::Validator;
use super::MigrationError;

/// A 32-byte content digest.
pub type Digest32 = [u8; 32];

fn hash_bytes(bytes: &[u8]) -> Digest32 {
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    hasher.finalize().into()
}

fn hash_file(path: &Path) -> io::Result<Digest32> {
    let mut file = File::open(path)?;
    let mut hasher = Sha256::new();
    let mut buffer = [0u8; 64 * 1024];
    loop {
        let read = file.read(&mut buffer)?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
    }
    Ok(hasher.finalize().into())
}

/// Hex content digest of a single regular file.
pub fn hash_file_hex(path: &Path) -> Result<String, MigrationError> {
    hash_file(path)
        .map(hex::encode)
        .map_err(|source| MigrationError::CopyIo {
            path: path.to_path_buf(),
            source,
        })
}

/// Safely remove a directory tree, refusing any symlink or reparse point.
///
/// Used to discard an unjournaled partial destination tree before recopying so
/// an interrupted opaque-tree copy is recoverable.
pub fn remove_tree_no_links(path: &Path) -> Result<(), MigrationError> {
    let metadata = match fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(source) => {
            return Err(MigrationError::CopyIo {
                path: path.to_path_buf(),
                source,
            })
        }
    };
    if is_link_or_reparse(&metadata) {
        return Err(MigrationError::LinkNotAllowed {
            path: path.to_path_buf(),
        });
    }
    if metadata.is_dir() {
        for entry in fs::read_dir(path).map_err(|source| MigrationError::CopyIo {
            path: path.to_path_buf(),
            source,
        })? {
            let entry = entry.map_err(|source| MigrationError::CopyIo {
                path: path.to_path_buf(),
                source,
            })?;
            remove_tree_no_links(&entry.path())?;
        }
        fs::remove_dir(path).map_err(|source| MigrationError::CopyIo {
            path: path.to_path_buf(),
            source,
        })
    } else {
        fs::remove_file(path).map_err(|source| MigrationError::CopyIo {
            path: path.to_path_buf(),
            source,
        })
    }
}

/// Validate the syntactic form of a document per its [`Validator`].
///
/// `OpaqueFile`, `OpaqueTree`, `Sqlite`, and `None` are not content-parsed here.
pub fn validate_document(validator: Validator, bytes: &[u8]) -> Result<(), MigrationError> {
    match validator {
        Validator::Json => serde_json::from_slice::<serde_json::Value>(bytes)
            .map(|_| ())
            .map_err(|source| MigrationError::JsonInvalid { source }),
        Validator::Xml => validate_char_list_xml(bytes),
        Validator::RealmShark => validate_realmshark(bytes),
        Validator::OpaqueFile | Validator::OpaqueTree | Validator::Sqlite | Validator::None => {
            Ok(())
        }
    }
}

/// Syntactic well-formedness check for `char_list.xml`, independent of assets.
fn validate_char_list_xml(bytes: &[u8]) -> Result<(), MigrationError> {
    use quick_xml::events::Event;
    use quick_xml::Reader;

    let text = std::str::from_utf8(bytes).map_err(|_| MigrationError::XmlInvalid {
        detail: "char_list.xml is not valid UTF-8".to_string(),
    })?;
    let mut reader = Reader::from_str(text);
    reader.config_mut().check_end_names = true;
    let mut buf_events = 0u64;
    loop {
        match reader.read_event() {
            Ok(Event::Eof) => break,
            Ok(_) => {
                buf_events += 1;
            }
            Err(error) => {
                return Err(MigrationError::XmlInvalid {
                    detail: error.to_string(),
                })
            }
        }
    }
    if buf_events == 0 {
        return Err(MigrationError::XmlInvalid {
            detail: "char_list.xml contained no XML events".to_string(),
        });
    }
    Ok(())
}

/// Syntactic validator for a RealmShark `dungeon.stats` document.
///
/// Only checks the document shape (`data` map of dungeon entries with the
/// expected scalar fields); it never resolves object or dungeon assets.
fn validate_realmshark(bytes: &[u8]) -> Result<(), MigrationError> {
    #[derive(serde::Deserialize)]
    struct Shape {
        #[allow(dead_code)]
        data: std::collections::HashMap<String, Entry>,
    }
    #[derive(serde::Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct Entry {
        #[allow(dead_code)]
        name: String,
        #[allow(dead_code)]
        entered_dungeon: i64,
        #[allow(dead_code)]
        total_time: i64,
    }

    serde_json::from_slice::<Shape>(bytes)
        .map(|_| ())
        .map_err(|source| MigrationError::RealmSharkInvalid { source })
}

/// Copy one regular file to a new destination and prove the copy is durable.
///
/// The destination must not already exist. After copying, the file is flushed,
/// its declared document form is validated, and its content hash is compared to
/// the source. Returns the shared content digest on success.
pub fn copy_and_validate_file(
    source: &Path,
    destination: &Path,
    validator: Validator,
) -> Result<Digest32, MigrationError> {
    let bytes = fs::read(source).map_err(|source_err| MigrationError::CopyIo {
        path: source.to_path_buf(),
        source: source_err,
    })?;
    let source_digest = hash_bytes(&bytes);

    validate_document(validator, &bytes)?;

    if let Some(parent) = destination.parent() {
        fs::create_dir_all(parent).map_err(|source_err| MigrationError::CopyIo {
            path: parent.to_path_buf(),
            source: source_err,
        })?;
    }
    let mut file = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(destination)
        .map_err(|source_err| MigrationError::CopyIo {
            path: destination.to_path_buf(),
            source: source_err,
        })?;
    file.write_all(&bytes)
        .and_then(|()| file.sync_all())
        .map_err(|source_err| MigrationError::CopyIo {
            path: destination.to_path_buf(),
            source: source_err,
        })?;
    drop(file);

    let destination_digest =
        hash_file(destination).map_err(|source_err| MigrationError::CopyIo {
            path: destination.to_path_buf(),
            source: source_err,
        })?;
    if destination_digest != source_digest {
        return Err(MigrationError::CopyMismatch {
            path: destination.to_path_buf(),
        });
    }
    Ok(source_digest)
}

/// Recursive relative-path -> content-digest manifest for a directory tree.
pub type TreeManifest = BTreeMap<String, Digest32>;

/// Stable hex digest over a whole [`TreeManifest`], binding names and content.
pub fn tree_manifest_digest(manifest: &TreeManifest) -> String {
    let mut hasher = Sha256::new();
    for (name, digest) in manifest {
        hasher.update(name.as_bytes());
        hasher.update([0u8]);
        hasher.update(digest);
    }
    hex::encode(hasher.finalize())
}

/// Build the recursive manifest of a directory tree, refusing links/reparse.
pub fn tree_manifest(root: &Path) -> Result<TreeManifest, MigrationError> {
    build_tree_manifest(root)
}

fn build_tree_manifest(root: &Path) -> Result<TreeManifest, MigrationError> {
    let mut manifest = TreeManifest::new();
    let mut stack = vec![PathBuf::new()];
    while let Some(relative) = stack.pop() {
        let absolute = root.join(&relative);
        let metadata =
            fs::symlink_metadata(&absolute).map_err(|source| MigrationError::CopyIo {
                path: absolute.clone(),
                source,
            })?;
        if is_link_or_reparse(&metadata) {
            return Err(MigrationError::LinkNotAllowed {
                path: absolute.clone(),
            });
        }
        if metadata.is_dir() {
            for entry in fs::read_dir(&absolute).map_err(|source| MigrationError::CopyIo {
                path: absolute.clone(),
                source,
            })? {
                let entry = entry.map_err(|source| MigrationError::CopyIo {
                    path: absolute.clone(),
                    source,
                })?;
                stack.push(relative.join(entry.file_name()));
            }
        } else if metadata.is_file() {
            let digest = hash_file(&absolute).map_err(|source| MigrationError::CopyIo {
                path: absolute.clone(),
                source,
            })?;
            manifest.insert(relative.to_string_lossy().replace('\\', "/"), digest);
        }
    }
    Ok(manifest)
}

/// Copy an opaque directory tree and prove tree-manifest equality.
///
/// Never follows a symlink or Windows reparse point in either tree.
pub fn copy_and_validate_tree(
    source: &Path,
    destination: &Path,
) -> Result<TreeManifest, MigrationError> {
    let source_manifest = build_tree_manifest(source)?;

    fs::create_dir_all(destination).map_err(|src| MigrationError::CopyIo {
        path: destination.to_path_buf(),
        source: src,
    })?;
    for relative in source_manifest.keys() {
        let relative_path = PathBuf::from(relative);
        let from = source.join(&relative_path);
        let to = destination.join(&relative_path);
        if let Some(parent) = to.parent() {
            fs::create_dir_all(parent).map_err(|src| MigrationError::CopyIo {
                path: parent.to_path_buf(),
                source: src,
            })?;
        }
        let bytes = fs::read(&from).map_err(|src| MigrationError::CopyIo {
            path: from.clone(),
            source: src,
        })?;
        let mut file = fs::OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .open(&to)
            .map_err(|src| MigrationError::CopyIo {
                path: to.clone(),
                source: src,
            })?;
        file.write_all(&bytes)
            .and_then(|()| file.sync_all())
            .map_err(|src| MigrationError::CopyIo {
                path: to.clone(),
                source: src,
            })?;
    }

    let destination_manifest = build_tree_manifest(destination)?;
    if destination_manifest != source_manifest {
        return Err(MigrationError::CopyMismatch {
            path: destination.to_path_buf(),
        });
    }
    Ok(source_manifest)
}

#[cfg(windows)]
fn is_link_or_reparse(metadata: &fs::Metadata) -> bool {
    use std::os::windows::fs::MetadataExt;
    const FILE_ATTRIBUTE_REPARSE_POINT: u32 = 0x400;
    metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0
}

#[cfg(not(windows))]
fn is_link_or_reparse(metadata: &fs::Metadata) -> bool {
    metadata.file_type().is_symlink()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn json_validator_accepts_valid_and_rejects_invalid() {
        assert!(validate_document(Validator::Json, br#"{"a":1}"#).is_ok());
        assert!(validate_document(Validator::Json, b"{ not json").is_err());
    }

    #[test]
    fn xml_validator_accepts_wellformed_char_list() {
        let xml = br#"<Chars nextCharId="2"><Char id="1"></Char></Chars>"#;
        assert!(validate_document(Validator::Xml, xml).is_ok());
    }

    #[test]
    fn xml_validator_rejects_malformed_char_list() {
        let xml = br#"<Chars><Char></Chars>"#;
        assert!(validate_document(Validator::Xml, xml).is_err());
    }

    #[test]
    fn realmshark_validator_accepts_shape_without_assets() {
        let doc = br#"{"data":{"0":{"name":"Shatters","enteredDungeon":3,"totalTime":100}}}"#;
        assert!(validate_document(Validator::RealmShark, doc).is_ok());
    }

    #[test]
    fn realmshark_validator_rejects_wrong_shape() {
        assert!(validate_document(Validator::RealmShark, br#"{"nope":true}"#).is_err());
    }

    #[test]
    fn copy_and_validate_file_hashes_match() {
        let temp = tempfile::tempdir().unwrap();
        let source = temp.path().join("src.json");
        fs::write(&source, br#"{"ok":true}"#).unwrap();
        let dest = temp.path().join("out").join("dst.json");

        copy_and_validate_file(&source, &dest, Validator::Json).unwrap();
        assert_eq!(fs::read(&dest).unwrap(), fs::read(&source).unwrap());
    }

    #[test]
    fn copy_and_validate_file_rejects_invalid_json_before_writing() {
        let temp = tempfile::tempdir().unwrap();
        let source = temp.path().join("src.json");
        fs::write(&source, b"{ broken").unwrap();
        let dest = temp.path().join("dst.json");

        assert!(copy_and_validate_file(&source, &dest, Validator::Json).is_err());
        assert!(!dest.exists());
    }

    #[test]
    fn copy_and_validate_tree_matches_manifest() {
        let temp = tempfile::tempdir().unwrap();
        let source = temp.path().join("chat");
        fs::create_dir_all(source.join("nested")).unwrap();
        fs::write(source.join("a.log"), b"alpha").unwrap();
        fs::write(source.join("nested").join("b.log"), b"beta").unwrap();
        let dest = temp.path().join("out").join("chat");

        let manifest = copy_and_validate_tree(&source, &dest).unwrap();
        assert_eq!(manifest.len(), 2);
        assert_eq!(
            fs::read(dest.join("nested").join("b.log")).unwrap(),
            b"beta"
        );
    }
}
