use std::path::PathBuf;

#[derive(Debug, thiserror::Error)]
pub enum CatalogyError {
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),

    #[error("Database error: {0}")]
    Database(String),

    #[error("Embedding error: {0}")]
    Embedding(String),

    #[error("Extraction error: {0}")]
    Extraction(String),

    #[error("Config error: {0}")]
    Config(String),

    #[error("File not found: {path}")]
    FileNotFound { path: PathBuf },

    #[error("Unsupported format: {ext}")]
    UnsupportedFormat { ext: String },
}

pub type Result<T> = std::result::Result<T, CatalogyError>;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_error_display() {
        let err = CatalogyError::Config("bad toml".to_string());
        assert_eq!(err.to_string(), "Config error: bad toml");
    }

    #[test]
    fn test_file_not_found_display() {
        let err = CatalogyError::FileNotFound {
            path: PathBuf::from("/missing/file.jpg"),
        };
        assert!(err.to_string().contains("/missing/file.jpg"));
    }

    #[test]
    fn test_io_error_conversion() {
        let io_err = std::io::Error::new(std::io::ErrorKind::NotFound, "gone");
        let err: CatalogyError = io_err.into();
        assert!(matches!(err, CatalogyError::Io(_)));
    }
}
