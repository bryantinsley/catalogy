use catalogy_core::{CatalogyError, Result};
use std::path::{Path, PathBuf};

/// Result of applying the original file policy.
#[derive(Clone, Debug)]
pub struct PolicyResult {
    pub policy: String,
    pub original_path: PathBuf,
    pub transcoded_path: PathBuf,
    pub archive_path: Option<PathBuf>,
    pub original_deleted: bool,
}

/// Apply the configured policy to the original file after successful transcode.
///
/// Policies:
/// - "keep": Both files coexist, no action taken
/// - "archive": Move original to archive_dir preserving directory structure
/// - "replace": Delete original, move transcoded to original location
pub fn apply_policy(
    original_path: &Path,
    transcoded_path: &Path,
    policy: &str,
    archive_dir: Option<&str>,
) -> Result<PolicyResult> {
    match policy {
        "keep" => Ok(PolicyResult {
            policy: "keep".to_string(),
            original_path: original_path.to_path_buf(),
            transcoded_path: transcoded_path.to_path_buf(),
            archive_path: None,
            original_deleted: false,
        }),

        "archive" => {
            let archive_base = archive_dir.ok_or_else(|| {
                CatalogyError::Transcode(
                    "archive_dir must be set when original_policy is 'archive'".to_string(),
                )
            })?;

            let archive_path = build_archive_path(original_path, archive_base);

            // Create archive directory structure
            if let Some(parent) = archive_path.parent() {
                std::fs::create_dir_all(parent).map_err(|e| {
                    CatalogyError::Transcode(format!("cannot create archive directory: {e}"))
                })?;
            }

            // Move original to archive
            std::fs::rename(original_path, &archive_path).map_err(|e| {
                CatalogyError::Transcode(format!("failed to move original to archive: {e}"))
            })?;

            Ok(PolicyResult {
                policy: "archive".to_string(),
                original_path: original_path.to_path_buf(),
                transcoded_path: transcoded_path.to_path_buf(),
                archive_path: Some(archive_path),
                original_deleted: false,
            })
        }

        "replace" => {
            // Move transcoded to original location (replacing it)
            // First remove the original
            std::fs::remove_file(original_path)
                .map_err(|e| CatalogyError::Transcode(format!("failed to remove original: {e}")))?;

            // Move transcoded to original's location
            std::fs::rename(transcoded_path, original_path).or_else(|_| {
                // rename fails across filesystems, fall back to copy+delete
                std::fs::copy(transcoded_path, original_path)
                    .and_then(|_| std::fs::remove_file(transcoded_path))
                    .map_err(|e| {
                        CatalogyError::Transcode(format!(
                            "failed to move transcoded to original location: {e}"
                        ))
                    })
            })?;

            Ok(PolicyResult {
                policy: "replace".to_string(),
                original_path: original_path.to_path_buf(),
                transcoded_path: original_path.to_path_buf(), // now at original location
                archive_path: None,
                original_deleted: true,
            })
        }

        other => Err(CatalogyError::Transcode(format!(
            "unknown original_policy: '{other}'. Expected 'keep', 'archive', or 'replace'"
        ))),
    }
}

/// Build the archive path preserving the directory structure.
/// e.g., original: /nas/media/videos/clip.mp4, archive_dir: /nas/archive
/// → /nas/archive/videos/clip.mp4
fn build_archive_path(original_path: &Path, archive_base: &str) -> PathBuf {
    let file_name = original_path.file_name().unwrap_or_default();
    PathBuf::from(archive_base).join(file_name)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_keep_policy() {
        let result = apply_policy(
            Path::new("/original.mp4"),
            Path::new("/transcoded.mp4"),
            "keep",
            None,
        )
        .unwrap();

        assert_eq!(result.policy, "keep");
        assert!(!result.original_deleted);
        assert!(result.archive_path.is_none());
    }

    #[test]
    fn test_archive_policy_requires_archive_dir() {
        let result = apply_policy(
            Path::new("/original.mp4"),
            Path::new("/transcoded.mp4"),
            "archive",
            None,
        );

        assert!(result.is_err());
    }

    #[test]
    fn test_archive_policy() {
        let dir = tempfile::tempdir().unwrap();
        let original = dir.path().join("original.mp4");
        let transcoded = dir.path().join("transcoded.mp4");
        let archive_dir = dir.path().join("archive");

        std::fs::write(&original, b"original content").unwrap();
        std::fs::write(&transcoded, b"transcoded").unwrap();

        let result = apply_policy(
            &original,
            &transcoded,
            "archive",
            Some(archive_dir.to_str().unwrap()),
        )
        .unwrap();

        assert_eq!(result.policy, "archive");
        assert!(!result.original_deleted);
        assert!(result.archive_path.is_some());
        // Original should be moved to archive
        assert!(!original.exists());
        assert!(result.archive_path.unwrap().exists());
        // Transcoded should still be in place
        assert!(transcoded.exists());
    }

    #[test]
    fn test_replace_policy() {
        let dir = tempfile::tempdir().unwrap();
        let original = dir.path().join("original.mp4");
        let transcoded = dir.path().join("transcoded.mp4");

        std::fs::write(&original, b"original content that is big").unwrap();
        std::fs::write(&transcoded, b"smaller transcoded").unwrap();

        let result = apply_policy(&original, &transcoded, "replace", None).unwrap();

        assert_eq!(result.policy, "replace");
        assert!(result.original_deleted);
        // Original path should now contain transcoded content
        assert!(original.exists());
        let content = std::fs::read_to_string(&original).unwrap();
        assert_eq!(content, "smaller transcoded");
        // Transcoded staging file should be removed
        assert!(!transcoded.exists());
    }

    #[test]
    fn test_unknown_policy_errors() {
        let result = apply_policy(
            Path::new("/original.mp4"),
            Path::new("/transcoded.mp4"),
            "unknown",
            None,
        );

        assert!(result.is_err());
    }

    #[test]
    fn test_build_archive_path() {
        let path = build_archive_path(Path::new("/nas/media/videos/vacation.mp4"), "/nas/archive");
        assert_eq!(path, PathBuf::from("/nas/archive/vacation.mp4"));
    }
}
