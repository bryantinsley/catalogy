use serde::{Deserialize, Serialize};

/// A record in the LanceDB media catalog.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct CatalogRecord {
    pub id: String,
    pub file_hash: String,
    pub file_path: String,
    pub file_name: String,
    pub file_size: i64,
    pub file_ext: String,
    pub media_type: String,
    pub embedding: Vec<f32>,
    pub model_id: String,
    pub model_version: String,

    // Optional metadata fields
    pub width: Option<i32>,
    pub height: Option<i32>,
    pub duration_ms: Option<i64>,
    pub fps: Option<f32>,
    pub codec: Option<String>,
    pub bitrate_kbps: Option<i32>,

    // EXIF fields
    pub exif_camera_make: Option<String>,
    pub exif_camera_model: Option<String>,
    pub exif_date_taken: Option<String>,
    pub exif_gps_lat: Option<f64>,
    pub exif_gps_lon: Option<f64>,
    pub exif_focal_length_mm: Option<f32>,
    pub exif_iso: Option<i32>,
    pub exif_orientation: Option<i32>,

    // Video frame fields
    pub source_video_path: Option<String>,
    pub frame_index: Option<i32>,
    pub frame_timestamp_ms: Option<i64>,

    // File timestamps
    pub file_created: Option<String>,
    pub file_modified: Option<String>,

    // Catalog timestamps
    pub indexed_at: String,
    pub updated_at: String,
    pub tombstone: bool,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_catalog_record_creation() {
        let record = CatalogRecord {
            id: "test-id".to_string(),
            file_hash: "abc123".to_string(),
            file_path: "/test/photo.jpg".to_string(),
            file_name: "photo.jpg".to_string(),
            file_size: 1024,
            file_ext: "jpg".to_string(),
            media_type: "image".to_string(),
            embedding: vec![0.1; 1024],
            model_id: "clip-vit-h-14".to_string(),
            model_version: "1".to_string(),
            width: Some(1920),
            height: Some(1080),
            duration_ms: None,
            fps: None,
            codec: None,
            bitrate_kbps: None,
            exif_camera_make: None,
            exif_camera_model: None,
            exif_date_taken: None,
            exif_gps_lat: None,
            exif_gps_lon: None,
            exif_focal_length_mm: None,
            exif_iso: None,
            exif_orientation: None,
            source_video_path: None,
            frame_index: None,
            frame_timestamp_ms: None,
            file_created: None,
            file_modified: None,
            indexed_at: "2024-01-01T00:00:00Z".to_string(),
            updated_at: "2024-01-01T00:00:00Z".to_string(),
            tombstone: false,
        };

        assert_eq!(record.file_name, "photo.jpg");
        assert_eq!(record.embedding.len(), 1024);
        assert_eq!(record.media_type, "image");
    }

    #[test]
    fn test_catalog_record_serialization() {
        let record = CatalogRecord {
            id: "test-id".to_string(),
            file_hash: "abc123".to_string(),
            file_path: "/test/photo.jpg".to_string(),
            file_name: "photo.jpg".to_string(),
            file_size: 1024,
            file_ext: "jpg".to_string(),
            media_type: "image".to_string(),
            embedding: vec![0.1, 0.2, 0.3],
            model_id: "clip".to_string(),
            model_version: "1".to_string(),
            width: None,
            height: None,
            duration_ms: None,
            fps: None,
            codec: None,
            bitrate_kbps: None,
            exif_camera_make: None,
            exif_camera_model: None,
            exif_date_taken: None,
            exif_gps_lat: None,
            exif_gps_lon: None,
            exif_focal_length_mm: None,
            exif_iso: None,
            exif_orientation: None,
            source_video_path: None,
            frame_index: None,
            frame_timestamp_ms: None,
            file_created: None,
            file_modified: None,
            indexed_at: "2024-01-01T00:00:00Z".to_string(),
            updated_at: "2024-01-01T00:00:00Z".to_string(),
            tombstone: false,
        };

        let json = serde_json::to_string(&record).unwrap();
        let deserialized: CatalogRecord = serde_json::from_str(&json).unwrap();
        assert_eq!(deserialized.id, record.id);
        assert_eq!(deserialized.embedding, record.embedding);
    }
}
