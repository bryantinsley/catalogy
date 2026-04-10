use catalogy_core::{CatalogyError, Result};
use image::imageops::FilterType;
use image::GenericImageView;
use std::path::{Path, PathBuf};

const THUMBNAIL_MAX_DIMENSION: u32 = 300;
const THUMBNAIL_QUALITY: u8 = 85;

/// Generate a 300px max-dimension JPEG thumbnail from a source image or frame.
///
/// - `source`: path to the source image (JPEG/PNG) or extracted frame
/// - `thumb_dir`: directory to cache thumbnails
/// - `thumb_id`: unique identifier for this thumbnail (used as filename)
///
/// Returns the path to the generated thumbnail.
/// If the thumbnail already exists, returns the cached path.
pub fn generate_thumbnail(source: &Path, thumb_dir: &Path, thumb_id: &str) -> Result<PathBuf> {
    let thumb_path = thumb_dir.join(format!("{thumb_id}.jpg"));

    // Return cached thumbnail if it exists
    if thumb_path.exists() {
        return Ok(thumb_path);
    }

    // Ensure thumbnail directory exists
    std::fs::create_dir_all(thumb_dir).map_err(|e| {
        CatalogyError::Extraction(format!(
            "creating thumbnail dir {}: {e}",
            thumb_dir.display()
        ))
    })?;

    let img = image::open(source).map_err(|e| {
        CatalogyError::Extraction(format!("opening image {}: {e}", source.display()))
    })?;

    let (w, h) = img.dimensions();
    let (new_w, new_h) = fit_dimensions(w, h, THUMBNAIL_MAX_DIMENSION);

    let resized = img.resize_exact(new_w, new_h, FilterType::Lanczos3);

    let mut output = std::io::BufWriter::new(std::fs::File::create(&thumb_path).map_err(|e| {
        CatalogyError::Extraction(format!("creating thumbnail {}: {e}", thumb_path.display()))
    })?);

    let encoder =
        image::codecs::jpeg::JpegEncoder::new_with_quality(&mut output, THUMBNAIL_QUALITY);
    resized
        .write_with_encoder(encoder)
        .map_err(|e| CatalogyError::Extraction(format!("encoding thumbnail: {e}")))?;

    Ok(thumb_path)
}

/// Compute new dimensions to fit within max_dimension while preserving aspect ratio.
fn fit_dimensions(w: u32, h: u32, max_dim: u32) -> (u32, u32) {
    if w <= max_dim && h <= max_dim {
        return (w, h);
    }

    if w >= h {
        let new_w = max_dim;
        let new_h = (h as f64 * max_dim as f64 / w as f64).round() as u32;
        (new_w, new_h.max(1))
    } else {
        let new_h = max_dim;
        let new_w = (w as f64 * max_dim as f64 / h as f64).round() as u32;
        (new_w.max(1), new_h)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_fit_dimensions_landscape() {
        let (w, h) = fit_dimensions(1920, 1080, 300);
        assert_eq!(w, 300);
        assert_eq!(h, 169); // 1080 * 300 / 1920 ≈ 168.75 → 169
    }

    #[test]
    fn test_fit_dimensions_portrait() {
        let (w, h) = fit_dimensions(1080, 1920, 300);
        assert_eq!(w, 169);
        assert_eq!(h, 300);
    }

    #[test]
    fn test_fit_dimensions_square() {
        let (w, h) = fit_dimensions(1000, 1000, 300);
        assert_eq!(w, 300);
        assert_eq!(h, 300);
    }

    #[test]
    fn test_fit_dimensions_already_small() {
        let (w, h) = fit_dimensions(200, 150, 300);
        assert_eq!(w, 200);
        assert_eq!(h, 150);
    }

    #[test]
    fn test_fit_dimensions_exact_max() {
        let (w, h) = fit_dimensions(300, 300, 300);
        assert_eq!(w, 300);
        assert_eq!(h, 300);
    }

    #[test]
    fn test_generate_thumbnail_from_image() {
        let dir = tempfile::tempdir().unwrap();
        let thumb_dir = tempfile::tempdir().unwrap();

        // Create a small test image (100x50 red)
        let img = image::RgbImage::from_fn(100, 50, |_, _| image::Rgb([255, 0, 0]));
        let src_path = dir.path().join("test.jpg");
        img.save(&src_path).unwrap();

        let result = generate_thumbnail(&src_path, thumb_dir.path(), "test_thumb").unwrap();

        assert!(result.exists());
        assert!(result.to_string_lossy().ends_with(".jpg"));

        // Verify the thumbnail dimensions
        let thumb = image::open(&result).unwrap();
        let (w, h) = thumb.dimensions();
        // 100x50 is already within 300px, so no resize
        assert_eq!(w, 100);
        assert_eq!(h, 50);
    }

    #[test]
    fn test_generate_thumbnail_resize() {
        let dir = tempfile::tempdir().unwrap();
        let thumb_dir = tempfile::tempdir().unwrap();

        // Create a larger test image (800x600)
        let img = image::RgbImage::from_fn(800, 600, |x, y| {
            image::Rgb([(x % 256) as u8, (y % 256) as u8, 128])
        });
        let src_path = dir.path().join("large.png");
        img.save(&src_path).unwrap();

        let result = generate_thumbnail(&src_path, thumb_dir.path(), "large_thumb").unwrap();

        assert!(result.exists());
        let thumb = image::open(&result).unwrap();
        let (w, h) = thumb.dimensions();
        assert_eq!(w, 300);
        assert_eq!(h, 225); // 600 * 300 / 800 = 225
    }

    #[test]
    fn test_generate_thumbnail_cached() {
        let dir = tempfile::tempdir().unwrap();
        let thumb_dir = tempfile::tempdir().unwrap();

        let img = image::RgbImage::from_fn(100, 100, |_, _| image::Rgb([0, 128, 0]));
        let src_path = dir.path().join("test.jpg");
        img.save(&src_path).unwrap();

        // Generate once
        let path1 = generate_thumbnail(&src_path, thumb_dir.path(), "cached_test").unwrap();

        // Second call should return cached version
        let path2 = generate_thumbnail(&src_path, thumb_dir.path(), "cached_test").unwrap();

        assert_eq!(path1, path2);
    }

    #[test]
    fn test_generate_thumbnail_missing_source() {
        let thumb_dir = tempfile::tempdir().unwrap();
        let result = generate_thumbnail(
            Path::new("/nonexistent/image.jpg"),
            thumb_dir.path(),
            "missing",
        );
        assert!(result.is_err());
    }
}
