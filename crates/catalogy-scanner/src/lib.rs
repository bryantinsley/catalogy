use catalogy_core::{CatalogyError, FileHash, MediaType, Result, ScannedFile};
use sha2::{Digest, Sha256};
use std::fs::File;
use std::io::Read;
use std::path::Path;
use walkdir::WalkDir;

const HASH_CHUNK_SIZE: usize = 64 * 1024; // 64KB

/// Scan a directory for media files matching the given extensions.
pub fn scan_directory(
    root: &Path,
    image_extensions: &[String],
    video_extensions: &[String],
) -> Result<Vec<ScannedFile>> {
    if !root.exists() {
        return Err(CatalogyError::FileNotFound {
            path: root.to_path_buf(),
        });
    }

    let mut files = Vec::new();

    for entry in WalkDir::new(root)
        .follow_links(true)
        .into_iter()
        .filter_map(|e| e.ok())
    {
        if !entry.file_type().is_file() {
            continue;
        }

        let path = entry.path();
        let ext = match path.extension().and_then(|e| e.to_str()) {
            Some(e) => e.to_lowercase(),
            None => continue,
        };

        let media_type = if image_extensions.iter().any(|ie| ie.to_lowercase() == ext) {
            MediaType::Image
        } else if video_extensions.iter().any(|ve| ve.to_lowercase() == ext) {
            MediaType::Video
        } else {
            continue;
        };

        let metadata = entry
            .metadata()
            .map_err(|e| CatalogyError::Io(std::io::Error::other(e.to_string())))?;

        let hash = hash_file(path)?;
        let modified = metadata
            .modified()
            .unwrap_or(std::time::SystemTime::UNIX_EPOCH);

        files.push(ScannedFile {
            path: path.to_path_buf(),
            hash,
            size: metadata.len(),
            modified,
            media_type,
        });
    }

    Ok(files)
}

/// Compute SHA256 hash of a file using streaming 64KB chunks.
pub fn hash_file(path: &Path) -> Result<FileHash> {
    let mut file = File::open(path)?;
    let mut hasher = Sha256::new();
    let mut buffer = vec![0u8; HASH_CHUNK_SIZE];

    loop {
        let bytes_read = file.read(&mut buffer)?;
        if bytes_read == 0 {
            break;
        }
        hasher.update(&buffer[..bytes_read]);
    }

    let hash = hasher.finalize();
    Ok(FileHash(hex::encode(hash)))
}

/// Hex encoding helper (no extra dependency needed).
mod hex {
    pub fn encode(bytes: impl AsRef<[u8]>) -> String {
        bytes.as_ref().iter().map(|b| format!("{b:02x}")).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use tempfile::TempDir;

    fn create_test_dir() -> TempDir {
        let dir = TempDir::new().unwrap();
        // Create some test files
        std::fs::write(dir.path().join("photo1.jpg"), b"fake jpeg data 1").unwrap();
        std::fs::write(dir.path().join("photo2.png"), b"fake png data 2").unwrap();
        std::fs::write(dir.path().join("video1.mp4"), b"fake mp4 data").unwrap();
        std::fs::write(dir.path().join("readme.txt"), b"not a media file").unwrap();
        std::fs::write(dir.path().join("document.pdf"), b"not media").unwrap();
        dir
    }

    fn image_exts() -> Vec<String> {
        vec!["jpg".into(), "jpeg".into(), "png".into()]
    }

    fn video_exts() -> Vec<String> {
        vec!["mp4".into(), "mov".into()]
    }

    #[test]
    fn test_scan_filters_by_extension() {
        let dir = create_test_dir();
        let files = scan_directory(dir.path(), &image_exts(), &video_exts()).unwrap();

        // Should find 3 media files, not txt or pdf
        assert_eq!(files.len(), 3);

        let names: Vec<String> = files
            .iter()
            .map(|f| f.path.file_name().unwrap().to_string_lossy().to_string())
            .collect();
        assert!(names.contains(&"photo1.jpg".to_string()));
        assert!(names.contains(&"photo2.png".to_string()));
        assert!(names.contains(&"video1.mp4".to_string()));
        assert!(!names.contains(&"readme.txt".to_string()));
    }

    #[test]
    fn test_scan_classifies_media_types() {
        let dir = create_test_dir();
        let files = scan_directory(dir.path(), &image_exts(), &video_exts()).unwrap();

        let images: Vec<_> = files
            .iter()
            .filter(|f| f.media_type == MediaType::Image)
            .collect();
        let videos: Vec<_> = files
            .iter()
            .filter(|f| f.media_type == MediaType::Video)
            .collect();

        assert_eq!(images.len(), 2);
        assert_eq!(videos.len(), 1);
    }

    #[test]
    fn test_scan_recursive() {
        let dir = TempDir::new().unwrap();
        let subdir = dir.path().join("subdir");
        std::fs::create_dir(&subdir).unwrap();
        std::fs::write(dir.path().join("top.jpg"), b"top level").unwrap();
        std::fs::write(subdir.join("nested.jpg"), b"nested file").unwrap();

        let files = scan_directory(dir.path(), &image_exts(), &video_exts()).unwrap();
        assert_eq!(files.len(), 2);
    }

    #[test]
    fn test_scan_empty_dir() {
        let dir = TempDir::new().unwrap();
        let files = scan_directory(dir.path(), &image_exts(), &video_exts()).unwrap();
        assert!(files.is_empty());
    }

    #[test]
    fn test_scan_nonexistent_dir() {
        let result = scan_directory(Path::new("/nonexistent/path"), &image_exts(), &video_exts());
        assert!(result.is_err());
    }

    #[test]
    fn test_hash_deterministic() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("test.bin");
        std::fs::write(&path, b"hello world").unwrap();

        let hash1 = hash_file(&path).unwrap();
        let hash2 = hash_file(&path).unwrap();
        assert_eq!(hash1, hash2);
    }

    #[test]
    fn test_hash_different_content() {
        let dir = TempDir::new().unwrap();

        let path1 = dir.path().join("file1.bin");
        let path2 = dir.path().join("file2.bin");
        std::fs::write(&path1, b"content one").unwrap();
        std::fs::write(&path2, b"content two").unwrap();

        let hash1 = hash_file(&path1).unwrap();
        let hash2 = hash_file(&path2).unwrap();
        assert_ne!(hash1, hash2);
    }

    #[test]
    fn test_hash_known_value() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("test.bin");
        std::fs::write(&path, b"hello world").unwrap();

        let hash = hash_file(&path).unwrap();
        // SHA256 of "hello world"
        assert_eq!(
            hash.0,
            "b94d27b9934d3e08a52e52d7da7dabfac484efe37a5380ee9088f7ace2efcde9"
        );
    }

    #[test]
    fn test_hash_large_file() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("large.bin");
        // Create a file larger than the 64KB chunk size
        let mut file = File::create(&path).unwrap();
        let chunk = vec![0xABu8; HASH_CHUNK_SIZE + 1];
        file.write_all(&chunk).unwrap();
        drop(file);

        let hash = hash_file(&path).unwrap();
        assert!(!hash.0.is_empty());
        assert_eq!(hash.0.len(), 64); // SHA256 hex is 64 chars
    }

    #[test]
    fn test_hash_empty_file() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("empty.bin");
        std::fs::write(&path, b"").unwrap();

        let hash = hash_file(&path).unwrap();
        // SHA256 of empty string
        assert_eq!(
            hash.0,
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
        );
    }

    #[test]
    fn test_scan_case_insensitive_extensions() {
        let dir = TempDir::new().unwrap();
        std::fs::write(dir.path().join("photo.JPG"), b"uppercase ext").unwrap();
        std::fs::write(dir.path().join("photo.Png"), b"mixed case").unwrap();

        let files = scan_directory(dir.path(), &image_exts(), &video_exts()).unwrap();
        assert_eq!(files.len(), 2);
    }

    #[test]
    fn test_scanned_file_has_size() {
        let dir = TempDir::new().unwrap();
        let content = b"some content here";
        std::fs::write(dir.path().join("test.jpg"), content).unwrap();

        let files = scan_directory(dir.path(), &image_exts(), &video_exts()).unwrap();
        assert_eq!(files.len(), 1);
        assert_eq!(files[0].size, content.len() as u64);
    }
}
