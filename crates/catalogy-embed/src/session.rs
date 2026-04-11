use catalogy_core::{CatalogyError, Result};
use ndarray::Array4;
use ort::session::Session;
use ort::value::Tensor;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use crate::image_encoder;
use crate::text_encoder::ClipTokenizer;

const CLIP_DIMENSIONS: usize = 1024;

/// Manages ONNX Runtime sessions for CLIP visual and text encoders.
pub struct EmbedSession {
    visual_session: Mutex<Session>,
    text_session: Mutex<Session>,
    tokenizer: ClipTokenizer,
}

impl EmbedSession {
    /// Load ONNX models and tokenizer from file paths.
    pub fn new(
        visual_model_path: &Path,
        text_model_path: &Path,
        tokenizer_path: &Path,
    ) -> Result<Self> {
        let visual_session = Session::builder()
            .map_err(|e| {
                CatalogyError::Embedding(format!("Failed to create ORT session builder: {}", e))
            })?
            .with_execution_providers([
                #[cfg(target_os = "macos")]
                ort::ep::CoreML::default().build(),
                ort::ep::CPU::default().build(),
            ])
            .map_err(|e| {
                CatalogyError::Embedding(format!("Failed to set execution providers: {}", e))
            })?
            .commit_from_file(visual_model_path)
            .map_err(|e| {
                CatalogyError::Embedding(format!(
                    "Failed to load visual model from {}: {}",
                    visual_model_path.display(),
                    e
                ))
            })?;

        let text_session = Session::builder()
            .map_err(|e| {
                CatalogyError::Embedding(format!("Failed to create ORT session builder: {}", e))
            })?
            .with_execution_providers([
                #[cfg(target_os = "macos")]
                ort::ep::CoreML::default().build(),
                ort::ep::CPU::default().build(),
            ])
            .map_err(|e| {
                CatalogyError::Embedding(format!("Failed to set execution providers: {}", e))
            })?
            .commit_from_file(text_model_path)
            .map_err(|e| {
                CatalogyError::Embedding(format!(
                    "Failed to load text model from {}: {}",
                    text_model_path.display(),
                    e
                ))
            })?;

        let tokenizer = ClipTokenizer::new(tokenizer_path)?;

        Ok(Self {
            visual_session: Mutex::new(visual_session),
            text_session: Mutex::new(text_session),
            tokenizer,
        })
    }

    /// Embed a single image from file path.
    pub fn embed_image(&self, image_path: &Path) -> Result<Vec<f32>> {
        let input = image_encoder::preprocess_image(image_path)?;
        self.run_visual_inference(input)
    }

    /// Embed a batch of images from file paths.
    pub fn embed_images(&self, image_paths: &[PathBuf]) -> Result<Vec<Vec<f32>>> {
        if image_paths.is_empty() {
            return Ok(Vec::new());
        }

        let input = image_encoder::preprocess_image_batch(image_paths)?;
        let batch_size = image_paths.len();
        let flat = self.run_visual_inference(input)?;

        // Split flat vector into per-image vectors
        let dim = self.dimensions();
        let mut results = Vec::with_capacity(batch_size);
        for i in 0..batch_size {
            let start = i * dim;
            let end = start + dim;
            results.push(flat[start..end].to_vec());
        }
        Ok(results)
    }

    /// Embed a text query.
    pub fn embed_text(&self, text: &str) -> Result<Vec<f32>> {
        let tokens = self.tokenizer.tokenize(text)?;

        // Convert ndarray to owned data for Tensor::from_array
        let shape: Vec<i64> = tokens.shape().iter().map(|&d| d as i64).collect();
        let data: Vec<i64> = tokens.into_raw_vec_and_offset().0;
        let input_tensor = Tensor::from_array((shape, data)).map_err(|e| {
            CatalogyError::Embedding(format!("Failed to create text input tensor: {}", e))
        })?;

        let embedding = {
            let mut text_session = self
                .text_session
                .lock()
                .map_err(|e| CatalogyError::Embedding(format!("Session lock poisoned: {}", e)))?;
            let outputs = text_session
                .run(ort::inputs![input_tensor])
                .map_err(|e| CatalogyError::Embedding(format!("Text inference failed: {}", e)))?;

            let output_tensor = outputs[0].try_extract_tensor::<f32>().map_err(|e| {
                CatalogyError::Embedding(format!("Failed to extract text embedding: {}", e))
            })?;

            output_tensor.1.to_vec()
        };
        Ok(l2_normalize(&embedding))
    }

    /// Get embedding dimensions.
    pub fn dimensions(&self) -> usize {
        CLIP_DIMENSIONS
    }

    fn run_visual_inference(&self, input: Array4<f32>) -> Result<Vec<f32>> {
        // Convert ndarray to owned data for Tensor::from_array
        let shape: Vec<i64> = input.shape().iter().map(|&d| d as i64).collect();
        let data: Vec<f32> = input.into_raw_vec_and_offset().0;
        let input_tensor = Tensor::from_array((shape, data)).map_err(|e| {
            CatalogyError::Embedding(format!("Failed to create visual input tensor: {}", e))
        })?;

        let embedding = {
            let mut visual_session = self
                .visual_session
                .lock()
                .map_err(|e| CatalogyError::Embedding(format!("Session lock poisoned: {}", e)))?;
            let outputs = visual_session
                .run(ort::inputs![input_tensor])
                .map_err(|e| CatalogyError::Embedding(format!("Visual inference failed: {}", e)))?;

            let output_tensor = outputs[0].try_extract_tensor::<f32>().map_err(|e| {
                CatalogyError::Embedding(format!("Failed to extract visual embedding: {}", e))
            })?;

            output_tensor.1.to_vec()
        };
        Ok(l2_normalize(&embedding))
    }
}

/// L2-normalize an embedding vector.
pub fn l2_normalize(v: &[f32]) -> Vec<f32> {
    let norm: f32 = v.iter().map(|x| x * x).sum::<f32>().sqrt();
    if norm < f32::EPSILON {
        return v.to_vec();
    }
    v.iter().map(|x| x / norm).collect()
}

/// Compute cosine similarity between two L2-normalized vectors.
pub fn cosine_similarity(a: &[f32], b: &[f32]) -> f32 {
    a.iter().zip(b.iter()).map(|(x, y)| x * y).sum()
}

/// Deduplicate frame embeddings by cosine similarity.
/// Drops frames with similarity > threshold to the previous kept frame.
/// Returns indices of kept frames.
pub fn dedup_frames(embeddings: &[Vec<f32>], threshold: f32) -> Vec<usize> {
    if embeddings.is_empty() {
        return Vec::new();
    }

    let mut kept = vec![0usize]; // Always keep first frame

    for i in 1..embeddings.len() {
        let last_kept = kept[kept.len() - 1];
        let sim = cosine_similarity(&embeddings[last_kept], &embeddings[i]);
        if sim <= threshold {
            kept.push(i);
        }
    }

    kept
}

/// Mean-pool a set of embeddings into a single vector.
pub fn mean_pool(embeddings: &[Vec<f32>]) -> Vec<f32> {
    if embeddings.is_empty() {
        return Vec::new();
    }

    let dim = embeddings[0].len();
    let mut mean = vec![0.0f32; dim];
    let n = embeddings.len() as f32;

    for emb in embeddings {
        for (i, &v) in emb.iter().enumerate() {
            mean[i] += v;
        }
    }

    for v in &mut mean {
        *v /= n;
    }

    l2_normalize(&mean)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn skip_if_no_models() -> Option<(PathBuf, PathBuf, PathBuf)> {
        let model_dir = std::env::var("CATALOGY_MODEL_DIR").ok()?;
        let dir = PathBuf::from(&model_dir);
        let visual = dir.join("visual.onnx");
        let text = dir.join("text.onnx");
        let tokenizer = dir.join("tokenizer.json");
        if visual.exists() && text.exists() && tokenizer.exists() {
            Some((visual, text, tokenizer))
        } else {
            None
        }
    }

    #[test]
    fn test_l2_normalize() {
        let v = vec![3.0, 4.0];
        let n = l2_normalize(&v);
        assert!((n[0] - 0.6).abs() < 1e-5);
        assert!((n[1] - 0.8).abs() < 1e-5);
    }

    #[test]
    fn test_l2_normalize_zero_vector() {
        let v = vec![0.0, 0.0, 0.0];
        let n = l2_normalize(&v);
        assert_eq!(n, vec![0.0, 0.0, 0.0]);
    }

    #[test]
    fn test_cosine_similarity_identical() {
        let a = l2_normalize(&vec![1.0, 2.0, 3.0]);
        let sim = cosine_similarity(&a, &a);
        assert!((sim - 1.0).abs() < 1e-5);
    }

    #[test]
    fn test_cosine_similarity_orthogonal() {
        let a = l2_normalize(&vec![1.0, 0.0]);
        let b = l2_normalize(&vec![0.0, 1.0]);
        let sim = cosine_similarity(&a, &b);
        assert!(sim.abs() < 1e-5);
    }

    #[test]
    fn test_dedup_frames_all_identical() {
        let emb = l2_normalize(&vec![1.0, 0.0, 0.0]);
        let embeddings = vec![emb.clone(), emb.clone(), emb.clone(), emb.clone()];
        let kept = dedup_frames(&embeddings, 0.95);
        assert_eq!(kept, vec![0]); // Only first kept
    }

    #[test]
    fn test_dedup_frames_all_different() {
        let embeddings = vec![
            l2_normalize(&vec![1.0, 0.0, 0.0]),
            l2_normalize(&vec![0.0, 1.0, 0.0]),
            l2_normalize(&vec![0.0, 0.0, 1.0]),
        ];
        let kept = dedup_frames(&embeddings, 0.95);
        assert_eq!(kept, vec![0, 1, 2]); // All kept
    }

    #[test]
    fn test_dedup_frames_mixed() {
        let embeddings = vec![
            l2_normalize(&vec![1.0, 0.0, 0.0]),
            l2_normalize(&vec![0.99, 0.01, 0.0]), // Very similar to [0]
            l2_normalize(&vec![0.0, 1.0, 0.0]),   // Different
            l2_normalize(&vec![0.01, 0.99, 0.0]), // Very similar to [2]
        ];
        let kept = dedup_frames(&embeddings, 0.95);
        assert_eq!(kept.len(), 2);
        assert_eq!(kept[0], 0);
        assert_eq!(kept[1], 2);
    }

    #[test]
    fn test_dedup_frames_empty() {
        let kept = dedup_frames(&[], 0.95);
        assert!(kept.is_empty());
    }

    #[test]
    fn test_mean_pool() {
        let embeddings = vec![vec![1.0, 0.0], vec![0.0, 1.0]];
        let mean = mean_pool(&embeddings);
        // Mean is [0.5, 0.5], L2-normalized = [0.707.., 0.707..]
        let expected = 0.5_f32 / (0.5_f32.powi(2) * 2.0).sqrt();
        assert!((mean[0] - expected).abs() < 1e-5);
        assert!((mean[1] - expected).abs() < 1e-5);
    }

    #[test]
    fn test_mean_pool_empty() {
        let mean = mean_pool(&[]);
        assert!(mean.is_empty());
    }

    #[test]
    fn test_embed_session_load_with_models() {
        let Some((visual, text, tokenizer)) = skip_if_no_models() else {
            eprintln!("Skipping: CATALOGY_MODEL_DIR not set or model files missing");
            return;
        };

        let session = EmbedSession::new(&visual, &text, &tokenizer).unwrap();
        assert_eq!(session.dimensions(), 1024);
    }

    #[test]
    fn test_embed_image_with_models() {
        let Some((visual, text, tokenizer)) = skip_if_no_models() else {
            eprintln!("Skipping: CATALOGY_MODEL_DIR not set or model files missing");
            return;
        };

        let session = EmbedSession::new(&visual, &text, &tokenizer).unwrap();

        // Create a test image
        let dir = tempfile::tempdir().unwrap();
        let img_path = dir.path().join("test.png");
        let img = image::ImageBuffer::from_fn(300, 200, |x, y| {
            image::Rgb([(x % 256) as u8, (y % 256) as u8, 128u8])
        });
        image::DynamicImage::ImageRgb8(img).save(&img_path).unwrap();

        let embedding = session.embed_image(&img_path).unwrap();
        assert_eq!(embedding.len(), 1024);

        // Should be L2-normalized
        let norm: f32 = embedding.iter().map(|x| x * x).sum::<f32>().sqrt();
        assert!((norm - 1.0).abs() < 1e-3);
    }

    #[test]
    fn test_embed_text_with_models() {
        let Some((visual, text, tokenizer)) = skip_if_no_models() else {
            eprintln!("Skipping: CATALOGY_MODEL_DIR not set or model files missing");
            return;
        };

        let session = EmbedSession::new(&visual, &text, &tokenizer).unwrap();
        let embedding = session.embed_text("a photo of a cat").unwrap();
        assert_eq!(embedding.len(), 1024);

        // Should be L2-normalized
        let norm: f32 = embedding.iter().map(|x| x * x).sum::<f32>().sqrt();
        assert!((norm - 1.0).abs() < 1e-3);
    }
}
