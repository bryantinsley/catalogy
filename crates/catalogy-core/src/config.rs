use serde::Deserialize;

#[derive(Clone, Debug, Deserialize)]
pub struct Config {
    pub library: LibraryConfig,
    pub database: DatabaseConfig,
    pub embedding: EmbeddingConfig,
    pub extraction: ExtractionConfig,
    pub ingest: IngestConfig,
    pub server: ServerConfig,
}

#[derive(Clone, Debug, Deserialize)]
pub struct LibraryConfig {
    pub paths: Vec<String>,
    pub extensions_image: Vec<String>,
    pub extensions_video: Vec<String>,
}

#[derive(Clone, Debug, Deserialize)]
pub struct DatabaseConfig {
    pub catalog_path: String,
    pub state_path: String,
}

#[derive(Clone, Debug, Deserialize)]
pub struct EmbeddingConfig {
    pub model_path: String,
    pub model_id: String,
    pub model_version: String,
    pub dimensions: u32,
    pub batch_size: u32,
    pub execution_provider: String,
}

#[derive(Clone, Debug, Deserialize)]
pub struct ExtractionConfig {
    pub frame_strategy: String,
    pub scene_threshold: f32,
    pub max_interval_seconds: u32,
    pub frame_interval_seconds: u32,
    pub frame_max_dimension: u32,
    pub dedup_similarity_threshold: f32,
    #[serde(default)]
    pub ffprobe_path: Option<String>,
    #[serde(default = "default_thumbnail_dir")]
    pub thumbnail_dir: String,
}

fn default_thumbnail_dir() -> String {
    "~/.local/share/catalogy/thumbs".to_string()
}

#[derive(Clone, Debug, Deserialize)]
pub struct IngestConfig {
    pub workers: u32,
    pub hash_algorithm: String,
}

#[derive(Clone, Debug, Deserialize)]
pub struct ServerConfig {
    pub port: u16,
    pub host: String,
}

impl Config {
    pub fn from_file(path: &str) -> crate::Result<Self> {
        let content = std::fs::read_to_string(path)
            .map_err(|e| crate::CatalogyError::Config(e.to_string()))?;
        Self::parse(content.as_str())
    }

    pub fn parse(content: &str) -> crate::Result<Self> {
        toml::from_str(content).map_err(|e| crate::CatalogyError::Config(e.to_string()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const TEST_CONFIG: &str = r#"
[library]
paths = ["/Volumes/Media/Photos"]
extensions_image = ["jpg", "jpeg", "png"]
extensions_video = ["mp4", "mov"]

[database]
catalog_path = "~/.local/share/catalogy/catalog.lance"
state_path = "~/.local/share/catalogy/state.db"

[embedding]
model_path = "~/.local/share/catalogy/models/clip-vit-h-14.onnx"
model_id = "clip-vit-h-14"
model_version = "1"
dimensions = 1024
batch_size = 16
execution_provider = "coreml"

[extraction]
frame_strategy = "adaptive"
scene_threshold = 0.3
max_interval_seconds = 60
frame_interval_seconds = 30
frame_max_dimension = 512
dedup_similarity_threshold = 0.95

[ingest]
workers = 4
hash_algorithm = "sha256"

[server]
port = 8080
host = "127.0.0.1"
"#;

    #[test]
    fn test_parse_config() {
        let config = Config::parse(TEST_CONFIG).unwrap();
        assert_eq!(config.library.paths, vec!["/Volumes/Media/Photos"]);
        assert_eq!(config.embedding.dimensions, 1024);
        assert_eq!(config.server.port, 8080);
        assert_eq!(config.extraction.frame_strategy, "adaptive");
        assert_eq!(config.ingest.workers, 4);
    }

    #[test]
    fn test_invalid_config() {
        let result = Config::parse("invalid toml [[[");
        assert!(result.is_err());
    }

    #[test]
    fn test_missing_field() {
        let result = Config::parse("[library]\npaths = []");
        assert!(result.is_err());
    }
}
