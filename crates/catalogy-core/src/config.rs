use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct Config {
    #[serde(default)]
    pub library: LibraryConfig,
    #[serde(default)]
    pub database: DatabaseConfig,
    #[serde(default)]
    pub embedding: EmbeddingConfig,
    #[serde(default)]
    pub extraction: ExtractionConfig,
    #[serde(default)]
    pub ingest: IngestConfig,
    #[serde(default)]
    pub server: ServerConfig,
    #[serde(default)]
    pub transcode: TranscodeConfig,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            library: Default::default(),
            database: Default::default(),
            embedding: Default::default(),
            extraction: Default::default(),
            ingest: Default::default(),
            server: Default::default(),
            transcode: Default::default(),
        }
    }
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct LibraryConfig {
    #[serde(default)]
    pub paths: Vec<String>,
    #[serde(default = "default_image_exts")]
    pub extensions_image: Vec<String>,
    #[serde(default = "default_video_exts")]
    pub extensions_video: Vec<String>,
}

impl Default for LibraryConfig {
    fn default() -> Self {
        Self {
            paths: Vec::new(),
            extensions_image: default_image_exts(),
            extensions_video: default_video_exts(),
        }
    }
}

fn default_image_exts() -> Vec<String> {
    vec!["jpg".into(), "jpeg".into(), "png".into(), "gif".into(), "bmp".into(),
         "tiff".into(), "tif".into(), "webp".into(), "heic".into(), "heif".into(), "avif".into()]
}

fn default_video_exts() -> Vec<String> {
    vec!["mp4".into(), "mov".into(), "avi".into(), "mkv".into(), "wmv".into(),
         "flv".into(), "webm".into(), "m4v".into(), "mpg".into(), "mpeg".into()]
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct DatabaseConfig {
    #[serde(default = "default_catalog_path")]
    pub catalog_path: String,
    #[serde(default = "default_state_path")]
    pub state_path: String,
}

impl Default for DatabaseConfig {
    fn default() -> Self {
        Self {
            catalog_path: default_catalog_path(),
            state_path: default_state_path(),
        }
    }
}

fn default_catalog_path() -> String {
    "~/.local/share/catalogy/catalog.lance".to_string()
}

fn default_state_path() -> String {
    "~/.local/share/catalogy/state.db".to_string()
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct EmbeddingConfig {
    #[serde(default = "default_model_path")]
    pub model_path: String,
    #[serde(default = "default_model_id")]
    pub model_id: String,
    #[serde(default = "default_model_version")]
    pub model_version: String,
    #[serde(default = "default_dimensions")]
    pub dimensions: u32,
    #[serde(default = "default_batch_size")]
    pub batch_size: u32,
    #[serde(default = "default_execution_provider")]
    pub execution_provider: String,
}

impl Default for EmbeddingConfig {
    fn default() -> Self {
        Self {
            model_path: default_model_path(),
            model_id: default_model_id(),
            model_version: default_model_version(),
            dimensions: default_dimensions(),
            batch_size: default_batch_size(),
            execution_provider: default_execution_provider(),
        }
    }
}

fn default_model_path() -> String {
    "~/.local/share/catalogy/models/clip-vit-h-14.onnx".to_string()
}
fn default_model_id() -> String { "clip-vit-h-14".into() }
fn default_model_version() -> String { "1".into() }
fn default_dimensions() -> u32 { 1024 }
fn default_batch_size() -> u32 { 16 }
fn default_execution_provider() -> String { "cpu".into() }

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct ExtractionConfig {
    #[serde(default = "default_frame_strategy")]
    pub frame_strategy: String,
    #[serde(default = "default_scene_threshold")]
    pub scene_threshold: f64,
    #[serde(default = "default_max_interval_seconds")]
    pub max_interval_seconds: u32,
    #[serde(default = "default_frame_interval_seconds")]
    pub frame_interval_seconds: u32,
    #[serde(default = "default_frame_max_dimension")]
    pub frame_max_dimension: u32,
    #[serde(default = "default_dedup_similarity_threshold")]
    pub dedup_similarity_threshold: f64,
    #[serde(default)]
    pub ffprobe_path: Option<String>,
    #[serde(default = "default_thumbnail_dir")]
    pub thumbnail_dir: String,
}

impl Default for ExtractionConfig {
    fn default() -> Self {
        Self {
            frame_strategy: default_frame_strategy(),
            scene_threshold: default_scene_threshold(),
            max_interval_seconds: default_max_interval_seconds(),
            frame_interval_seconds: default_frame_interval_seconds(),
            frame_max_dimension: default_frame_max_dimension(),
            dedup_similarity_threshold: default_dedup_similarity_threshold(),
            ffprobe_path: None,
            thumbnail_dir: default_thumbnail_dir(),
        }
    }
}

fn default_frame_strategy() -> String { "adaptive".into() }
fn default_scene_threshold() -> f64 { 0.3 }
fn default_max_interval_seconds() -> u32 { 60 }
fn default_frame_interval_seconds() -> u32 { 30 }
fn default_frame_max_dimension() -> u32 { 512 }
fn default_dedup_similarity_threshold() -> f64 { 0.95 }

fn default_thumbnail_dir() -> String {
    "~/.local/share/catalogy/thumbs".to_string()
}

fn default_staging_dir() -> String {
    "~/.local/share/catalogy/transcode_staging".to_string()
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct TranscodeConfig {
    #[serde(default)]
    pub enabled: bool,
    #[serde(default = "default_max_resolution")]
    pub max_resolution: String,
    #[serde(default = "default_target_codec")]
    pub target_codec: String,
    #[serde(default = "default_target_crf")]
    pub target_crf: u32,
    #[serde(default = "default_use_hw_encoder")]
    pub use_hw_encoder: bool,
    #[serde(default = "default_original_policy")]
    pub original_policy: String,
    #[serde(default = "default_staging_dir")]
    pub staging_dir: String,
    #[serde(default)]
    pub archive_dir: Option<String>,
}

impl Default for TranscodeConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            max_resolution: default_max_resolution(),
            target_codec: default_target_codec(),
            target_crf: default_target_crf(),
            use_hw_encoder: default_use_hw_encoder(),
            original_policy: default_original_policy(),
            staging_dir: default_staging_dir(),
            archive_dir: None,
        }
    }
}

fn default_max_resolution() -> String { "1080p".into() }
fn default_target_codec() -> String { "h265".into() }
fn default_target_crf() -> u32 { 23 }
fn default_use_hw_encoder() -> bool { true }
fn default_original_policy() -> String { "keep".into() }

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct IngestConfig {
    #[serde(default = "default_workers")]
    pub workers: u32,
    #[serde(default = "default_hash_algorithm")]
    pub hash_algorithm: String,
}

impl Default for IngestConfig {
    fn default() -> Self {
        Self {
            workers: default_workers(),
            hash_algorithm: default_hash_algorithm(),
        }
    }
}

fn default_workers() -> u32 { 4 }
fn default_hash_algorithm() -> String { "sha256".into() }

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct ServerConfig {
    #[serde(default = "default_server_port")]
    pub port: u16,
    #[serde(default = "default_server_host")]
    pub host: String,
}

impl Default for ServerConfig {
    fn default() -> Self {
        Self {
            port: default_server_port(),
            host: default_server_host(),
        }
    }
}

fn default_server_port() -> u16 { 18080 }
fn default_server_host() -> String { "127.0.0.1".into() }

impl Config {
    pub fn from_file(path: &str) -> crate::Result<Self> {
        let content = std::fs::read_to_string(path)
            .map_err(|e| crate::CatalogyError::Config(e.to_string()))?;
        Self::parse(content.as_str())
    }

    pub fn parse(content: &str) -> crate::Result<Self> {
        toml::from_str(content).map_err(|e| crate::CatalogyError::Config(e.to_string()))
    }

    /// Serialize this config to TOML and write it to *path*.
    pub fn to_file(&self, path: &str) -> crate::Result<()> {
        let content = toml::to_string_pretty(self)
            .map_err(|e| crate::CatalogyError::Config(e.to_string()))?;
        // Ensure the parent directory exists — on a fresh system the config dir
        // (e.g. ~/.config/catalogy) won't exist yet, so saving from the web UI
        // would otherwise fail with ENOENT.
        if let Some(parent) = std::path::Path::new(path).parent() {
            std::fs::create_dir_all(parent)
                .map_err(|e| crate::CatalogyError::Config(e.to_string()))?;
        }
        std::fs::write(path, content)
            .map_err(|e| crate::CatalogyError::Config(e.to_string()))
    }

    /// Merge another config into this one. Non-default values in *other* overwrite
    /// the corresponding fields in *self*.  This is used for partial config updates
    /// from the web UI.
    pub fn merge(&mut self, other: &Config) {
        // Library
        if !other.library.paths.is_empty() {
            self.library.paths.clone_from(&other.library.paths);
        }
        if other.library.extensions_image != default_image_exts() {
            self.library.extensions_image.clone_from(&other.library.extensions_image);
        }
        if other.library.extensions_video != default_video_exts() {
            self.library.extensions_video.clone_from(&other.library.extensions_video);
        }

        // Extraction
        if other.extraction.frame_strategy != default_frame_strategy() {
            self.extraction.frame_strategy.clone_from(&other.extraction.frame_strategy);
        }
        if other.extraction.scene_threshold != default_scene_threshold() {
            self.extraction.scene_threshold = other.extraction.scene_threshold;
        }
        if other.extraction.max_interval_seconds != default_max_interval_seconds() {
            self.extraction.max_interval_seconds = other.extraction.max_interval_seconds;
        }
        if other.extraction.frame_interval_seconds != default_frame_interval_seconds() {
            self.extraction.frame_interval_seconds = other.extraction.frame_interval_seconds;
        }
        if other.extraction.frame_max_dimension != default_frame_max_dimension() {
            self.extraction.frame_max_dimension = other.extraction.frame_max_dimension;
        }
        if other.extraction.dedup_similarity_threshold != default_dedup_similarity_threshold() {
            self.extraction.dedup_similarity_threshold = other.extraction.dedup_similarity_threshold;
        }
        if other.extraction.ffprobe_path.is_some() {
            self.extraction.ffprobe_path.clone_from(&other.extraction.ffprobe_path);
        }
        if other.extraction.thumbnail_dir != default_thumbnail_dir() {
            self.extraction.thumbnail_dir.clone_from(&other.extraction.thumbnail_dir);
        }

        // Transcode
        self.transcode.enabled = other.transcode.enabled;
        if other.transcode.max_resolution != default_max_resolution() {
            self.transcode.max_resolution.clone_from(&other.transcode.max_resolution);
        }
        if other.transcode.target_codec != default_target_codec() {
            self.transcode.target_codec.clone_from(&other.transcode.target_codec);
        }
        if other.transcode.target_crf != default_target_crf() {
            self.transcode.target_crf = other.transcode.target_crf;
        }
        self.transcode.use_hw_encoder = other.transcode.use_hw_encoder;
        if other.transcode.original_policy != default_original_policy() {
            self.transcode.original_policy.clone_from(&other.transcode.original_policy);
        }
        if other.transcode.staging_dir != default_staging_dir() {
            self.transcode.staging_dir.clone_from(&other.transcode.staging_dir);
        }
        if other.transcode.archive_dir.is_some() {
            self.transcode.archive_dir.clone_from(&other.transcode.archive_dir);
        }

        // Ingest
        if other.ingest.workers != default_workers() {
            self.ingest.workers = other.ingest.workers;
        }
        if other.ingest.hash_algorithm != default_hash_algorithm() {
            self.ingest.hash_algorithm.clone_from(&other.ingest.hash_algorithm);
        }

        // Server
        if other.server.port != default_server_port() {
            self.server.port = other.server.port;
        }
        if other.server.host != default_server_host() {
            self.server.host.clone_from(&other.server.host);
        }
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
port = 18080
host = "127.0.0.1"
"#;

    #[test]
    fn test_parse_config() {
        let config = Config::parse(TEST_CONFIG).unwrap();
        assert_eq!(config.library.paths, vec!["/Volumes/Media/Photos"]);
        assert_eq!(config.embedding.dimensions, 1024);
        assert_eq!(config.server.port, 18080);
        assert_eq!(config.extraction.frame_strategy, "adaptive");
        assert_eq!(config.ingest.workers, 4);
        // Transcode should use defaults when section is omitted
        assert!(!config.transcode.enabled);
        assert_eq!(config.transcode.max_resolution, "1080p");
        assert_eq!(config.transcode.target_codec, "h265");
    }

    #[test]
    fn test_parse_config_with_transcode() {
        let config_str = format!(
            r#"{}
[transcode]
enabled = true
max_resolution = "4k"
target_codec = "h265"
target_crf = 18
use_hw_encoder = false
original_policy = "archive"
archive_dir = "/nas/archive"
"#,
            TEST_CONFIG
        );
        let config = Config::parse(&config_str).unwrap();
        assert!(config.transcode.enabled);
        assert_eq!(config.transcode.max_resolution, "4k");
        assert_eq!(config.transcode.target_crf, 18);
        assert!(!config.transcode.use_hw_encoder);
        assert_eq!(config.transcode.original_policy, "archive");
        assert_eq!(
            config.transcode.archive_dir,
            Some("/nas/archive".to_string())
        );
    }

    #[test]
    fn test_invalid_config() {
        let result = Config::parse("invalid toml [[[");
        assert!(result.is_err());
    }

    #[test]
    fn test_default_config_has_sensible_values() {
        let config = Config::default();
        assert!(config.library.paths.is_empty());
        assert_eq!(config.extraction.frame_strategy, "adaptive");
        assert!(!config.transcode.enabled);
        assert_eq!(config.transcode.max_resolution, "1080p");
        assert_eq!(config.ingest.workers, 4);
        assert_eq!(config.server.port, 18080);
    }

    #[test]
    fn test_merge_partial_config() {
        let mut base = Config::default();
        let partial = Config {
            library: LibraryConfig {
                paths: vec!["/new/path".into()],
                ..Default::default()
            },
            transcode: TranscodeConfig {
                enabled: true,
                ..Default::default()
            },
            ..Default::default()
        };
        base.merge(&partial);
        assert_eq!(base.library.paths, vec!["/new/path"]);
        assert!(base.transcode.enabled);
        // Unchanged fields keep their defaults
        assert_eq!(base.ingest.workers, 4);
    }

    #[test]
    fn test_roundtrip() {
        let original = Config {
            library: LibraryConfig {
                paths: vec!["/a".into(), "/b".into()],
                ..Default::default()
            },
            ..Default::default()
        };
        let toml_str = toml::to_string_pretty(&original).unwrap();
        let parsed = Config::parse(&toml_str).unwrap();
        assert_eq!(parsed.library.paths, vec!["/a", "/b"]);
    }

    #[test]
    fn test_to_file_creates_parent_dirs_and_roundtrips() {
        // Write into a config dir that does not exist yet (mirrors a fresh
        // system saving config from the web UI for the first time).
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("nested/catalogy/config.toml");
        assert!(!path.parent().unwrap().exists());

        let mut cfg = Config::default();
        cfg.library.paths = vec!["/tmp/media".into()];
        cfg.to_file(path.to_str().unwrap()).unwrap();

        assert!(path.exists(), "config file should have been created");
        let loaded = Config::from_file(path.to_str().unwrap()).unwrap();
        assert_eq!(loaded.library.paths, vec!["/tmp/media"]);
    }
}
