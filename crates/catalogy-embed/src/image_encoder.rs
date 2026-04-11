use catalogy_core::{CatalogyError, Result};
use image::imageops::FilterType;
use image::{DynamicImage, GenericImageView, RgbImage};
use ndarray::Array4;
use std::path::Path;

/// CLIP image normalization constants
const CLIP_MEAN: [f32; 3] = [0.48145466, 0.4578275, 0.40821073];
const CLIP_STD: [f32; 3] = [0.26862954, 0.26130258, 0.27577711];
const CLIP_INPUT_SIZE: u32 = 224;

/// Load and preprocess an image for CLIP inference.
/// Returns an ndarray of shape [1, 3, 224, 224].
pub fn preprocess_image(image_path: &Path) -> Result<Array4<f32>> {
    let img = image::open(image_path).map_err(|e| {
        CatalogyError::Embedding(format!(
            "Failed to open image {}: {}",
            image_path.display(),
            e
        ))
    })?;
    preprocess_dynamic_image(&img)
}

/// Preprocess a DynamicImage for CLIP inference.
/// Returns an ndarray of shape [1, 3, 224, 224].
pub fn preprocess_dynamic_image(img: &DynamicImage) -> Result<Array4<f32>> {
    let resized = resize_and_center_crop(img, CLIP_INPUT_SIZE);
    let normalized = normalize_to_chw(&resized);
    Ok(normalized)
}

/// Preprocess a batch of images for CLIP inference.
/// Returns an ndarray of shape [N, 3, 224, 224].
pub fn preprocess_image_batch(image_paths: &[impl AsRef<Path>]) -> Result<Array4<f32>> {
    let mut batch = Array4::<f32>::zeros((
        image_paths.len(),
        3,
        CLIP_INPUT_SIZE as usize,
        CLIP_INPUT_SIZE as usize,
    ));

    for (i, path) in image_paths.iter().enumerate() {
        let single = preprocess_image(path.as_ref())?;
        batch
            .slice_mut(ndarray::s![i, .., .., ..])
            .assign(&single.slice(ndarray::s![0, .., .., ..]));
    }

    Ok(batch)
}

/// Resize image so shortest side = target_size, then center crop to target_size x target_size.
pub fn resize_and_center_crop(img: &DynamicImage, target_size: u32) -> RgbImage {
    let (w, h) = img.dimensions();

    // Resize so shortest side = target_size
    let (new_w, new_h) = if w < h {
        (
            target_size,
            (target_size as f64 * h as f64 / w as f64).round() as u32,
        )
    } else {
        (
            (target_size as f64 * w as f64 / h as f64).round() as u32,
            target_size,
        )
    };

    let resized = img.resize_exact(new_w, new_h, FilterType::Lanczos3);

    // Center crop
    let x_offset = (new_w.saturating_sub(target_size)) / 2;
    let y_offset = (new_h.saturating_sub(target_size)) / 2;
    let cropped = resized.crop_imm(x_offset, y_offset, target_size, target_size);

    cropped.to_rgb8()
}

/// Normalize an RGB image to CLIP format: [1, 3, H, W] CHW float tensor.
/// Applies CLIP normalization: (pixel/255 - mean) / std
pub fn normalize_to_chw(img: &RgbImage) -> Array4<f32> {
    let (w, h) = (img.width() as usize, img.height() as usize);
    let mut tensor = Array4::<f32>::zeros((1, 3, h, w));

    for y in 0..h {
        for x in 0..w {
            let pixel = img.get_pixel(x as u32, y as u32);
            for c in 0..3 {
                let val = pixel[c] as f32 / 255.0;
                tensor[[0, c, y, x]] = (val - CLIP_MEAN[c]) / CLIP_STD[c];
            }
        }
    }

    tensor
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::{ImageBuffer, Rgb};

    fn create_test_image(w: u32, h: u32) -> DynamicImage {
        let img = ImageBuffer::from_fn(w, h, |x, y| {
            Rgb([(x % 256) as u8, (y % 256) as u8, ((x + y) % 256) as u8])
        });
        DynamicImage::ImageRgb8(img)
    }

    #[test]
    fn test_resize_and_center_crop_square() {
        let img = create_test_image(256, 256);
        let result = resize_and_center_crop(&img, 224);
        assert_eq!(result.width(), 224);
        assert_eq!(result.height(), 224);
    }

    #[test]
    fn test_resize_and_center_crop_landscape() {
        let img = create_test_image(640, 480);
        let result = resize_and_center_crop(&img, 224);
        assert_eq!(result.width(), 224);
        assert_eq!(result.height(), 224);
    }

    #[test]
    fn test_resize_and_center_crop_portrait() {
        let img = create_test_image(480, 640);
        let result = resize_and_center_crop(&img, 224);
        assert_eq!(result.width(), 224);
        assert_eq!(result.height(), 224);
    }

    #[test]
    fn test_resize_and_center_crop_small() {
        // Smaller than target — should upscale
        let img = create_test_image(100, 80);
        let result = resize_and_center_crop(&img, 224);
        assert_eq!(result.width(), 224);
        assert_eq!(result.height(), 224);
    }

    #[test]
    fn test_normalize_to_chw_shape() {
        let img = ImageBuffer::from_fn(224, 224, |_, _| Rgb([128u8, 128, 128]));
        let tensor = normalize_to_chw(&img);
        assert_eq!(tensor.shape(), &[1, 3, 224, 224]);
    }

    #[test]
    fn test_normalize_to_chw_values() {
        // All-black image: pixel=0 => (0.0 - mean) / std
        let img = ImageBuffer::from_fn(4, 4, |_, _| Rgb([0u8, 0, 0]));
        let tensor = normalize_to_chw(&img);

        let expected_r = (0.0 - CLIP_MEAN[0]) / CLIP_STD[0];
        let expected_g = (0.0 - CLIP_MEAN[1]) / CLIP_STD[1];
        let expected_b = (0.0 - CLIP_MEAN[2]) / CLIP_STD[2];

        assert!((tensor[[0, 0, 0, 0]] - expected_r).abs() < 1e-5);
        assert!((tensor[[0, 1, 0, 0]] - expected_g).abs() < 1e-5);
        assert!((tensor[[0, 2, 0, 0]] - expected_b).abs() < 1e-5);
    }

    #[test]
    fn test_normalize_to_chw_white() {
        // All-white image: pixel=255 => (1.0 - mean) / std
        let img = ImageBuffer::from_fn(4, 4, |_, _| Rgb([255u8, 255, 255]));
        let tensor = normalize_to_chw(&img);

        let expected_r = (1.0 - CLIP_MEAN[0]) / CLIP_STD[0];
        assert!((tensor[[0, 0, 0, 0]] - expected_r).abs() < 1e-5);
    }

    #[test]
    fn test_preprocess_roundtrip() {
        let img = create_test_image(800, 600);
        let tensor = preprocess_dynamic_image(&img).unwrap();
        assert_eq!(tensor.shape(), &[1, 3, 224, 224]);
    }

    #[test]
    fn test_preprocess_image_file() {
        // Create a temp image file
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("test.png");
        let img = create_test_image(300, 200);
        img.save(&path).unwrap();

        let tensor = preprocess_image(&path).unwrap();
        assert_eq!(tensor.shape(), &[1, 3, 224, 224]);
    }

    #[test]
    fn test_preprocess_nonexistent_file() {
        let result = preprocess_image(Path::new("/nonexistent/image.jpg"));
        assert!(result.is_err());
    }
}
