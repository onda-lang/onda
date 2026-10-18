use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

/// Metadata shortcuts are only used for fallback polling. Native invalidations
/// always validate contents, including edits that preserve timestamps and size.
#[derive(Default)]
pub(super) struct FileValidationCache {
    validated: HashMap<PathBuf, Option<FileMetadata>>,
}

impl FileValidationCache {
    pub(super) fn clear(&mut self) {
        self.validated.clear();
    }

    pub(super) fn matches(
        &mut self,
        paths: &[PathBuf],
        validate_contents: impl FnOnce(&[PathBuf]) -> bool,
    ) -> bool {
        let changed = paths
            .iter()
            .filter_map(|path| {
                let metadata = FileMetadata::read(path);
                (self.validated.get(path) != Some(&metadata)).then_some((path, metadata))
            })
            .collect::<Vec<_>>();
        if changed.is_empty() {
            return true;
        }
        let paths = changed
            .iter()
            .map(|(path, _)| (*path).clone())
            .collect::<Vec<_>>();
        if !validate_contents(&paths) {
            return false;
        }
        for (path, metadata) in changed {
            // Never associate an older content check with newer metadata.
            if FileMetadata::read(path) != metadata {
                return false;
            }
            self.validated.insert(path.clone(), metadata);
        }
        true
    }
}

#[derive(Debug, PartialEq, Eq)]
struct FileMetadata {
    length: u64,
    modified: Option<SystemTime>,
    created: Option<SystemTime>,
    #[cfg(unix)]
    identity: (u64, u64, i64, i64),
}

impl FileMetadata {
    fn read(path: &Path) -> Option<Self> {
        onda_frontend::ensure_no_symlink_components(path).ok()?;
        let metadata = fs::metadata(path).ok()?;
        Some(Self {
            length: metadata.len(),
            modified: metadata.modified().ok(),
            created: metadata.created().ok(),
            #[cfg(unix)]
            identity: {
                use std::os::unix::fs::MetadataExt;
                (
                    metadata.dev(),
                    metadata.ino(),
                    metadata.ctime(),
                    metadata.ctime_nsec(),
                )
            },
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::UNIX_EPOCH;

    #[test]
    fn fallback_reads_contents_once_until_metadata_changes() {
        let root = std::env::temp_dir().join(format!(
            "onda_fallback_{}_{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir_all(&root).unwrap();
        let root = fs::canonicalize(root).unwrap();
        let path = root.join("asset.wav");
        fs::write(&path, b"original").unwrap();
        let paths = vec![path.clone()];
        let mut cache = FileValidationCache::default();
        assert!(cache.matches(&paths, |_| fs::read(&path).unwrap() == b"original"));
        assert!(cache.matches(&paths, |_| panic!("unchanged files must not be read again")));
        fs::write(&path, b"changed").unwrap();
        assert!(!cache.matches(&paths, |_| fs::read(&path).unwrap() == b"original"));
        cache.clear();
        assert!(cache.matches(&paths, |_| fs::read(&path).unwrap() == b"changed"));
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn content_validation_cannot_acknowledge_a_later_edit() {
        let path = std::env::temp_dir().canonicalize().unwrap().join(format!(
            "onda_fallback_race_{}_{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::write(&path, b"before").unwrap();
        let mut cache = FileValidationCache::default();
        assert!(!cache.matches(std::slice::from_ref(&path), |_| {
            fs::write(&path, b"after").unwrap();
            true
        }));
        fs::remove_file(path).unwrap();
    }
}
