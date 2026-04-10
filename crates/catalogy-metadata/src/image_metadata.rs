use catalogy_core::{ExifData, MediaMetadata, Result};
use std::fs::File;
use std::io::BufReader;
use std::path::Path;

/// Extract metadata from an image file.
///
/// Reads EXIF data with kamadak-exif, falls back to image crate for dimensions.
pub fn extract_image_metadata(path: &Path) -> Result<MediaMetadata> {
    let exif_data = read_exif(path);
    let (exif_width, exif_height) = exif_dimensions(&exif_data);

    // Try image crate for dimensions (header-only decode)
    let (width, height) = match image::image_dimensions(path) {
        Ok((w, h)) => (Some(w), Some(h)),
        Err(_) => (exif_width, exif_height),
    };

    Ok(MediaMetadata {
        width,
        height,
        duration_ms: None,
        fps: None,
        codec: None,
        bitrate_kbps: None,
        exif: exif_data,
    })
}

/// Try to read EXIF data from a file. Returns None on any failure.
fn read_exif(path: &Path) -> Option<ExifData> {
    let file = File::open(path).ok()?;
    let mut reader = BufReader::new(file);
    let exif_reader = exif::Reader::new();
    let exif = exif_reader.read_from_container(&mut reader).ok()?;

    let camera_make = get_string_field(&exif, exif::Tag::Make);
    let camera_model = get_string_field(&exif, exif::Tag::Model);
    let date_taken = get_date_taken(&exif);
    let (gps_lat, gps_lon) = get_gps(&exif);
    let focal_length_mm = get_rational_field(&exif, exif::Tag::FocalLength);
    let iso = get_u32_field(&exif, exif::Tag::PhotographicSensitivity);
    let orientation = get_u16_field(&exif, exif::Tag::Orientation).map(|v| v as u8);

    Some(ExifData {
        camera_make,
        camera_model,
        date_taken,
        gps_lat,
        gps_lon,
        focal_length_mm,
        iso,
        orientation,
    })
}

/// Extract dimensions from EXIF data if available.
fn exif_dimensions(exif_data: &Option<ExifData>) -> (Option<u32>, Option<u32>) {
    // EXIF doesn't directly store pixel dimensions in a standard way we can
    // easily use here; we rely on the image crate for that.
    let _ = exif_data;
    (None, None)
}

fn get_string_field(exif: &exif::Exif, tag: exif::Tag) -> Option<String> {
    let field = exif.get_field(tag, exif::In::PRIMARY)?;
    let val = field.display_value().to_string();
    let trimmed = val.trim().trim_matches('"').to_string();
    if trimmed.is_empty() {
        None
    } else {
        Some(trimmed)
    }
}

fn get_u32_field(exif: &exif::Exif, tag: exif::Tag) -> Option<u32> {
    let field = exif.get_field(tag, exif::In::PRIMARY)?;
    match &field.value {
        exif::Value::Short(v) => v.first().map(|&x| x as u32),
        exif::Value::Long(v) => v.first().copied(),
        _ => field.display_value().to_string().trim().parse().ok(),
    }
}

fn get_u16_field(exif: &exif::Exif, tag: exif::Tag) -> Option<u16> {
    let field = exif.get_field(tag, exif::In::PRIMARY)?;
    match &field.value {
        exif::Value::Short(v) => v.first().copied(),
        _ => field.display_value().to_string().trim().parse().ok(),
    }
}

fn get_rational_field(exif: &exif::Exif, tag: exif::Tag) -> Option<f32> {
    let field = exif.get_field(tag, exif::In::PRIMARY)?;
    match &field.value {
        exif::Value::Rational(v) => v.first().map(|r| r.num as f32 / r.denom as f32),
        _ => field.display_value().to_string().trim().parse().ok(),
    }
}

fn get_date_taken(exif: &exif::Exif) -> Option<chrono::NaiveDateTime> {
    let field = exif.get_field(exif::Tag::DateTimeOriginal, exif::In::PRIMARY)?;
    let val = field.display_value().to_string();
    let val = val.trim().trim_matches('"');

    // EXIF format: "2024:01:15 10:30:45"
    chrono::NaiveDateTime::parse_from_str(val, "%Y-%m-%d %H:%M:%S")
        .or_else(|_| chrono::NaiveDateTime::parse_from_str(val, "%Y:%m:%d %H:%M:%S"))
        .ok()
}

fn get_gps(exif: &exif::Exif) -> (Option<f64>, Option<f64>) {
    let lat = get_gps_coord(exif, exif::Tag::GPSLatitude, exif::Tag::GPSLatitudeRef);
    let lon = get_gps_coord(exif, exif::Tag::GPSLongitude, exif::Tag::GPSLongitudeRef);
    (lat, lon)
}

fn get_gps_coord(exif: &exif::Exif, coord_tag: exif::Tag, ref_tag: exif::Tag) -> Option<f64> {
    let field = exif.get_field(coord_tag, exif::In::PRIMARY)?;
    let rationals = match &field.value {
        exif::Value::Rational(v) if v.len() >= 3 => v,
        _ => return None,
    };

    let degrees = rationals[0].num as f64 / rationals[0].denom as f64;
    let minutes = rationals[1].num as f64 / rationals[1].denom as f64;
    let seconds = rationals[2].num as f64 / rationals[2].denom as f64;

    let mut coord = degrees + minutes / 60.0 + seconds / 3600.0;

    if let Some(ref_field) = exif.get_field(ref_tag, exif::In::PRIMARY) {
        let ref_val = ref_field.display_value().to_string();
        let ref_val = ref_val.trim().trim_matches('"');
        if ref_val == "S" || ref_val == "W" {
            coord = -coord;
        }
    }

    Some(coord)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_extract_image_no_exif() {
        // Create a minimal PNG in a temp file
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("test.png");

        // Minimal 1x1 red PNG
        let png_data: &[u8] = &[
            0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A, // PNG signature
            0x00, 0x00, 0x00, 0x0D, 0x49, 0x48, 0x44, 0x52, // IHDR chunk
            0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x01, // 1x1
            0x08, 0x02, 0x00, 0x00, 0x00, 0x90, 0x77, 0x53, 0xDE, // 8-bit RGB
            0x00, 0x00, 0x00, 0x0C, 0x49, 0x44, 0x41, 0x54, // IDAT chunk
            0x08, 0xD7, 0x63, 0xF8, 0xCF, 0xC0, 0x00, 0x00, 0x00, 0x02, 0x00, 0x01, 0xE2, 0x21,
            0xBC, 0x33, // compressed data
            0x00, 0x00, 0x00, 0x00, 0x49, 0x45, 0x4E, 0x44, // IEND
            0xAE, 0x42, 0x60, 0x82,
        ];
        std::fs::write(&path, png_data).unwrap();

        let meta = extract_image_metadata(&path).unwrap();
        assert_eq!(meta.width, Some(1));
        assert_eq!(meta.height, Some(1));
        assert!(meta.exif.is_none());
        assert!(meta.duration_ms.is_none());
    }

    #[test]
    fn test_extract_nonexistent_file() {
        let result = extract_image_metadata(Path::new("/nonexistent/file.jpg"));
        // Should still succeed with no exif data, but image dimensions will fail
        // and fall back to None
        assert!(result.is_ok());
        let meta = result.unwrap();
        assert!(meta.width.is_none());
        assert!(meta.height.is_none());
    }
}
