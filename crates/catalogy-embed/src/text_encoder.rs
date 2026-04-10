use catalogy_core::{CatalogyError, Result};
use ndarray::Array2;
use std::path::Path;
use tokenizers::Tokenizer;

const MAX_SEQ_LENGTH: usize = 77;

/// CLIP text tokenizer wrapper.
pub struct ClipTokenizer {
    tokenizer: Tokenizer,
}

impl ClipTokenizer {
    /// Load the tokenizer from a JSON file (HuggingFace tokenizer format).
    pub fn new(tokenizer_path: &Path) -> Result<Self> {
        let tokenizer = Tokenizer::from_file(tokenizer_path).map_err(|e| {
            CatalogyError::Embedding(format!(
                "Failed to load tokenizer from {}: {}",
                tokenizer_path.display(),
                e
            ))
        })?;
        Ok(Self { tokenizer })
    }

    /// Tokenize text into input_ids with CLIP-style padding/truncation.
    /// Returns shape [1, 77] i64 tensor.
    pub fn tokenize(&self, text: &str) -> Result<Array2<i64>> {
        let encoding = self.tokenizer.encode(text, true).map_err(|e| {
            CatalogyError::Embedding(format!("Tokenization failed: {}", e))
        })?;

        let mut ids = encoding
            .get_ids()
            .iter()
            .map(|&id| id as i64)
            .collect::<Vec<i64>>();

        // Truncate to max length
        if ids.len() > MAX_SEQ_LENGTH {
            ids.truncate(MAX_SEQ_LENGTH);
            // Ensure last token is EOS (49407 for CLIP)
            if let Some(last) = ids.last_mut() {
                *last = 49407;
            }
        }

        // Pad with zeros to max length
        while ids.len() < MAX_SEQ_LENGTH {
            ids.push(0);
        }

        let arr = Array2::from_shape_vec((1, MAX_SEQ_LENGTH), ids).map_err(|e| {
            CatalogyError::Embedding(format!("Failed to create token array: {}", e))
        })?;

        Ok(arr)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn skip_if_no_tokenizer() -> Option<PathBuf> {
        let model_dir = std::env::var("CATALOGY_MODEL_DIR").ok()?;
        let path = PathBuf::from(&model_dir).join("tokenizer.json");
        if path.exists() {
            Some(path)
        } else {
            None
        }
    }

    use std::path::PathBuf;

    #[test]
    fn test_tokenizer_load_missing_file() {
        let result = ClipTokenizer::new(Path::new("/nonexistent/tokenizer.json"));
        assert!(result.is_err());
    }

    #[test]
    fn test_tokenizer_with_real_file() {
        let Some(path) = skip_if_no_tokenizer() else {
            eprintln!("Skipping: CATALOGY_MODEL_DIR not set or tokenizer.json missing");
            return;
        };

        let tokenizer = ClipTokenizer::new(&path).unwrap();
        let tokens = tokenizer.tokenize("a photo of a cat").unwrap();
        assert_eq!(tokens.shape(), &[1, 77]);

        // First token should be SOT (49406 for CLIP)
        assert_eq!(tokens[[0, 0]], 49406);
    }
}
