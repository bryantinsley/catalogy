use std::path::PathBuf;
use std::time::SystemTime;

// --- Identity ---

/// Time-sortable unique ID for catalog entries.
pub type MediaId = uuid::Uuid;

/// Content-based identity for files (hex-encoded SHA256).
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct FileHash(pub String);

// --- Media ---

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum MediaType {
    Image,
    Video,
    VideoFrame,
}

#[derive(Clone, Debug)]
pub struct ScannedFile {
    pub path: PathBuf,
    pub hash: FileHash,
    pub size: u64,
    pub modified: SystemTime,
    pub media_type: MediaType,
}

#[derive(Clone, Debug)]
pub struct MediaMetadata {
    pub width: Option<u32>,
    pub height: Option<u32>,
    pub duration_ms: Option<u64>,
    pub fps: Option<f32>,
    pub codec: Option<String>,
    pub bitrate_kbps: Option<u32>,
    pub exif: Option<ExifData>,
}

#[derive(Clone, Debug)]
pub struct ExifData {
    pub camera_make: Option<String>,
    pub camera_model: Option<String>,
    pub date_taken: Option<chrono::NaiveDateTime>,
    pub gps_lat: Option<f64>,
    pub gps_lon: Option<f64>,
    pub focal_length_mm: Option<f32>,
    pub iso: Option<u32>,
    pub orientation: Option<u8>,
}

#[derive(Clone, Debug)]
pub struct Embedding {
    pub vector: Vec<f32>,
    pub model_id: String,
    pub model_version: String,
}

#[derive(Clone, Debug)]
pub struct ExtractedFrame {
    pub source_video: PathBuf,
    pub frame_index: u32,
    pub timestamp_ms: u64,
    pub image_data: Vec<u8>,
}

// --- Jobs ---

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum JobStage {
    ExtractFrames,
    ExtractMetadata,
    Embed,
    Index,
    ReEmbed,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum JobStatus {
    Pending,
    Running,
    Completed,
    Failed,
    Skipped,
}

#[derive(Clone, Debug)]
pub struct Job {
    pub id: i64,
    pub file_hash: FileHash,
    pub file_path: PathBuf,
    pub stage: JobStage,
    pub status: JobStatus,
    pub attempts: u32,
    pub error_message: Option<String>,
}

// --- Search ---

#[derive(Clone, Debug)]
pub struct SearchQuery {
    pub text: String,
    pub filters: SearchFilters,
    pub limit: usize,
}

#[derive(Clone, Debug, Default)]
pub struct SearchFilters {
    pub media_type: Option<MediaType>,
    pub after: Option<chrono::NaiveDateTime>,
    pub before: Option<chrono::NaiveDateTime>,
    pub min_width: Option<u32>,
    pub min_height: Option<u32>,
    pub file_ext: Option<String>,
    pub camera_model: Option<String>,
    pub has_gps: Option<bool>,
}

#[derive(Clone, Debug)]
pub struct SearchResult {
    pub id: MediaId,
    pub score: f32,
    pub file_path: PathBuf,
    pub file_name: String,
    pub media_type: MediaType,
    pub metadata: MediaMetadata,
    pub frame_info: Option<FrameInfo>,
}

#[derive(Clone, Debug)]
pub struct FrameInfo {
    pub source_video: PathBuf,
    pub frame_index: u32,
    pub timestamp_ms: u64,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_file_hash_equality() {
        let h1 = FileHash("abc123".to_string());
        let h2 = FileHash("abc123".to_string());
        assert_eq!(h1, h2);
    }

    #[test]
    fn test_media_type_variants() {
        assert_eq!(MediaType::Image, MediaType::Image);
        assert_ne!(MediaType::Image, MediaType::Video);
        assert_ne!(MediaType::Video, MediaType::VideoFrame);
    }

    #[test]
    fn test_job_stage_variants() {
        assert_eq!(JobStage::ExtractFrames, JobStage::ExtractFrames);
        assert_ne!(JobStage::Embed, JobStage::Index);
    }

    #[test]
    fn test_search_filters_default() {
        let filters = SearchFilters::default();
        assert!(filters.media_type.is_none());
        assert!(filters.after.is_none());
        assert!(filters.file_ext.is_none());
    }

    #[test]
    fn test_media_id_creation() {
        let id = uuid::Uuid::now_v7();
        assert_eq!(id.get_version(), Some(uuid::Version::SortRand));
    }

    #[test]
    fn test_construct_scanned_file() {
        let file = ScannedFile {
            path: PathBuf::from("/test/photo.jpg"),
            hash: FileHash("deadbeef".to_string()),
            size: 1024,
            modified: SystemTime::now(),
            media_type: MediaType::Image,
        };
        assert_eq!(file.size, 1024);
        assert_eq!(file.media_type, MediaType::Image);
    }

    #[test]
    fn test_construct_media_metadata() {
        let meta = MediaMetadata {
            width: Some(1920),
            height: Some(1080),
            duration_ms: None,
            fps: None,
            codec: Some("h264".to_string()),
            bitrate_kbps: None,
            exif: None,
        };
        assert_eq!(meta.width, Some(1920));
        assert!(meta.duration_ms.is_none());
    }

    #[test]
    fn test_construct_embedding() {
        let emb = Embedding {
            vector: vec![0.1, 0.2, 0.3],
            model_id: "clip-vit-h-14".to_string(),
            model_version: "1".to_string(),
        };
        assert_eq!(emb.vector.len(), 3);
    }
}
