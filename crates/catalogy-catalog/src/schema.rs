use arrow_schema::{DataType, Field, Schema};
use std::sync::Arc;

const EMBEDDING_DIM: i32 = 1024;

/// Build the Arrow schema for the LanceDB media table.
pub fn media_schema() -> Schema {
    Schema::new(vec![
        Field::new("id", DataType::Utf8, false),
        Field::new("file_hash", DataType::Utf8, false),
        Field::new("file_path", DataType::Utf8, false),
        Field::new("file_name", DataType::Utf8, false),
        Field::new("file_size", DataType::Int64, false),
        Field::new("file_ext", DataType::Utf8, false),
        Field::new("media_type", DataType::Utf8, false),
        Field::new(
            "embedding",
            DataType::FixedSizeList(
                Arc::new(Field::new("item", DataType::Float32, true)),
                EMBEDDING_DIM,
            ),
            false,
        ),
        Field::new("model_id", DataType::Utf8, false),
        Field::new("model_version", DataType::Utf8, false),
        // Optional metadata
        Field::new("width", DataType::Int32, true),
        Field::new("height", DataType::Int32, true),
        Field::new("duration_ms", DataType::Int64, true),
        Field::new("fps", DataType::Float32, true),
        Field::new("codec", DataType::Utf8, true),
        Field::new("bitrate_kbps", DataType::Int32, true),
        // EXIF
        Field::new("exif_camera_make", DataType::Utf8, true),
        Field::new("exif_camera_model", DataType::Utf8, true),
        Field::new("exif_date_taken", DataType::Utf8, true),
        Field::new("exif_gps_lat", DataType::Float64, true),
        Field::new("exif_gps_lon", DataType::Float64, true),
        Field::new("exif_focal_length_mm", DataType::Float32, true),
        Field::new("exif_iso", DataType::Int32, true),
        Field::new("exif_orientation", DataType::Int32, true),
        // Video frame
        Field::new("source_video_path", DataType::Utf8, true),
        Field::new("frame_index", DataType::Int32, true),
        Field::new("frame_timestamp_ms", DataType::Int64, true),
        // File timestamps
        Field::new("file_created", DataType::Utf8, true),
        Field::new("file_modified", DataType::Utf8, true),
        // Catalog metadata
        Field::new("indexed_at", DataType::Utf8, false),
        Field::new("updated_at", DataType::Utf8, false),
        Field::new("tombstone", DataType::Boolean, false),
    ])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_schema_field_count() {
        let schema = media_schema();
        // 10 required + 14 optional + 5 frame/timestamp + 3 catalog = 32 fields
        assert_eq!(schema.fields().len(), 32);
    }

    #[test]
    fn test_schema_embedding_field() {
        let schema = media_schema();
        let field = schema.field_with_name("embedding").unwrap();
        match field.data_type() {
            DataType::FixedSizeList(inner, size) => {
                assert_eq!(*size, 1024);
                assert_eq!(*inner.data_type(), DataType::Float32);
            }
            _ => panic!("embedding field should be FixedSizeList"),
        }
    }

    #[test]
    fn test_schema_required_fields() {
        let schema = media_schema();
        for name in &["id", "file_hash", "file_path", "embedding", "model_id"] {
            let field = schema.field_with_name(name).unwrap();
            assert!(!field.is_nullable(), "{} should not be nullable", name);
        }
    }

    #[test]
    fn test_schema_nullable_fields() {
        let schema = media_schema();
        for name in &["width", "height", "codec", "exif_camera_make", "exif_gps_lat"] {
            let field = schema.field_with_name(name).unwrap();
            assert!(field.is_nullable(), "{} should be nullable", name);
        }
    }
}
