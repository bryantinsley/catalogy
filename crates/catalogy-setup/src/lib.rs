use sha2::{Digest, Sha256};
use std::fmt;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

#[derive(Debug, thiserror::Error)]
pub enum SetupError {
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),

    #[error("Setup error: {0}")]
    General(String),

    #[error("Model export failed: {0}")]
    ModelExport(String),
}

pub type Result<T> = std::result::Result<T, SetupError>;

/// Status of an individual dependency check.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CheckStatus {
    Ok(String),
    Missing(String),
    Error(String),
}

impl CheckStatus {
    pub fn is_ok(&self) -> bool {
        matches!(self, CheckStatus::Ok(_))
    }

    pub fn message(&self) -> &str {
        match self {
            CheckStatus::Ok(m) | CheckStatus::Missing(m) | CheckStatus::Error(m) => m,
        }
    }

    pub fn symbol(&self) -> &str {
        match self {
            CheckStatus::Ok(_) => "[ok]",
            CheckStatus::Missing(_) => "[missing]",
            CheckStatus::Error(_) => "[error]",
        }
    }
}

impl fmt::Display for CheckStatus {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{} {}", self.symbol(), self.message())
    }
}

/// Overall setup status from check_dependencies.
#[derive(Debug)]
pub struct SetupStatus {
    pub ffmpeg: CheckStatus,
    pub ffprobe: CheckStatus,
    pub visual_model: CheckStatus,
    pub text_model: CheckStatus,
    pub tokenizer: CheckStatus,
    pub data_dir: CheckStatus,
    pub python: CheckStatus,
}

impl SetupStatus {
    pub fn all_ok(&self) -> bool {
        self.ffmpeg.is_ok()
            && self.ffprobe.is_ok()
            && self.visual_model.is_ok()
            && self.text_model.is_ok()
            && self.tokenizer.is_ok()
            && self.data_dir.is_ok()
    }

    pub fn models_ok(&self) -> bool {
        self.visual_model.is_ok() && self.text_model.is_ok() && self.tokenizer.is_ok()
    }
}

/// Check if a command is available in PATH by running it with a version flag.
fn check_command(cmd: &str, flag: &str) -> CheckStatus {
    match Command::new(cmd).arg(flag).output() {
        Ok(output) => {
            if output.status.success() {
                let stdout = String::from_utf8_lossy(&output.stdout);
                let first_line = stdout.lines().next().unwrap_or("").to_string();
                CheckStatus::Ok(format!("{cmd}: {first_line}"))
            } else {
                CheckStatus::Error(format!("{cmd} returned non-zero exit code"))
            }
        }
        Err(_) => CheckStatus::Missing(format!("{cmd} not found in PATH")),
    }
}

/// Check if a model file exists and report its size.
fn check_model_file(model_dir: &Path, filename: &str) -> CheckStatus {
    let path = model_dir.join(filename);
    if path.exists() {
        match fs::metadata(&path) {
            Ok(meta) => {
                let size_mb = meta.len() as f64 / 1_048_576.0;
                CheckStatus::Ok(format!("{filename} ({size_mb:.1} MB)"))
            }
            Err(e) => CheckStatus::Error(format!("{filename}: {e}")),
        }
    } else {
        CheckStatus::Missing(format!("{filename} not found in {}", model_dir.display()))
    }
}

/// Check all dependencies and return a structured status.
pub fn check_dependencies(model_dir: &Path, data_dir: &Path) -> SetupStatus {
    let ffmpeg = check_command("ffmpeg", "-version");
    let ffprobe = check_command("ffprobe", "-version");
    let visual_model = check_model_file(model_dir, "visual.onnx");
    let text_model = check_model_file(model_dir, "text.onnx");
    let tokenizer = check_model_file(model_dir, "tokenizer.json");

    let data_dir_status = if data_dir.exists() {
        // Check writability by trying to create a temp file
        let test_file = data_dir.join(".catalogy_write_test");
        match fs::write(&test_file, b"test") {
            Ok(_) => {
                let _ = fs::remove_file(&test_file);
                CheckStatus::Ok(format!("Data directory: {}", data_dir.display()))
            }
            Err(e) => CheckStatus::Error(format!(
                "Data directory not writable: {} ({})",
                data_dir.display(),
                e
            )),
        }
    } else {
        CheckStatus::Missing(format!("Data directory: {}", data_dir.display()))
    };

    let python = check_python();

    SetupStatus {
        ffmpeg,
        ffprobe,
        visual_model,
        text_model,
        tokenizer,
        data_dir: data_dir_status,
        python,
    }
}

/// Check Python3 and required packages for model export.
fn check_python() -> CheckStatus {
    match Command::new("python3").arg("--version").output() {
        Ok(output) if output.status.success() => {
            let version = String::from_utf8_lossy(&output.stdout).trim().to_string();
            // Check if required packages are available
            let check = Command::new("python3")
                .args([
                    "-c",
                    "import torch; import open_clip; import onnx; print('ok')",
                ])
                .output();
            match check {
                Ok(o) if o.status.success() => {
                    CheckStatus::Ok(format!("{version} (torch, open_clip, onnx available)"))
                }
                _ => CheckStatus::Error(format!(
                    "{version} (missing packages: run `pip install torch open-clip-torch onnx onnxruntime transformers`)"
                )),
            }
        }
        _ => CheckStatus::Missing("python3 not found in PATH".to_string()),
    }
}

/// Create all required directories for catalogy.
pub fn ensure_directories(data_dir: &Path, config_dir: &Path) -> Result<()> {
    let dirs_to_create = [
        data_dir.to_path_buf(),
        data_dir.join("models"),
        data_dir.join("thumbs"),
        config_dir.to_path_buf(),
    ];

    for dir in &dirs_to_create {
        fs::create_dir_all(dir)?;
    }

    Ok(())
}

/// Copy model files from a source directory (air-gapped setup).
pub fn copy_models_from_dir(src_dir: &Path, model_dir: &Path) -> Result<()> {
    let files = ["visual.onnx", "text.onnx", "tokenizer.json"];

    fs::create_dir_all(model_dir)?;

    for filename in &files {
        let src = src_dir.join(filename);
        let dst = model_dir.join(filename);
        if !src.exists() {
            return Err(SetupError::General(format!(
                "Required file not found: {}",
                src.display()
            )));
        }
        fs::copy(&src, &dst)?;
    }

    Ok(())
}

/// Export models using the Python script.
pub fn export_models_via_python(script_path: &Path, model_dir: &Path) -> Result<()> {
    fs::create_dir_all(model_dir)?;

    let output = Command::new("python3")
        .arg(script_path)
        .arg("--output-dir")
        .arg(model_dir)
        .arg("--skip-validation")
        .output()
        .map_err(|e| SetupError::ModelExport(format!("Failed to run python3: {e}")))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(SetupError::ModelExport(format!(
            "Export script failed:\n{stderr}"
        )));
    }

    // Verify all files exist
    for filename in &["visual.onnx", "text.onnx", "tokenizer.json"] {
        if !model_dir.join(filename).exists() {
            return Err(SetupError::ModelExport(format!(
                "{filename} was not created by export script"
            )));
        }
    }

    Ok(())
}

/// Compute SHA256 hash of a file and return hex string.
pub fn sha256_file(path: &Path) -> Result<String> {
    let data = fs::read(path)?;
    let mut hasher = Sha256::new();
    hasher.update(&data);
    let result = hasher.finalize();
    Ok(format!("{:x}", result))
}

/// Generate a starter config.toml at the given path.
pub fn generate_config(config_path: &Path, scan_paths: &[String]) -> Result<()> {
    if let Some(parent) = config_path.parent() {
        fs::create_dir_all(parent)?;
    }

    let paths_toml = if scan_paths.is_empty() {
        "# paths = [\"~/Photos\", \"~/Videos\"]".to_string()
    } else {
        let quoted: Vec<String> = scan_paths.iter().map(|p| format!("\"{p}\"")).collect();
        format!("paths = [{}]", quoted.join(", "))
    };

    let config = format!(
        r#"# Catalogy configuration
# Generated by `catalogy setup`

[library]
{paths_toml}
extensions_image = ["jpg", "jpeg", "png", "webp", "gif", "bmp", "tiff"]
extensions_video = ["mp4", "mov", "avi", "mkv", "webm"]

[database]
# catalog_path = "~/.local/share/catalogy/catalog.lance"
# state_path = "~/.local/share/catalogy/state.db"

[embedding]
model_id = "clip-vit-h-14"
model_version = "1"
dimensions = 1024
batch_size = 16

[extraction]
frame_strategy = "adaptive"
scene_threshold = 0.3
max_interval_seconds = 60
frame_interval_seconds = 30
frame_max_dimension = 512

[ingest]
workers = 4
hash_algorithm = "sha256"

[server]
port = 8080
host = "127.0.0.1"
"#
    );

    fs::write(config_path, config)?;
    Ok(())
}

/// Find the export_clip.py script relative to the binary or in common locations.
pub fn find_export_script() -> Option<PathBuf> {
    // Check relative to current executable
    if let Ok(exe) = std::env::current_exe() {
        // In development: exe is in target/debug or target/release
        // Script is at repo_root/scripts/export_clip.py
        if let Some(exe_dir) = exe.parent() {
            for ancestor in exe_dir.ancestors().take(5) {
                let script = ancestor.join("scripts").join("export_clip.py");
                if script.exists() {
                    return Some(script);
                }
            }
        }
    }

    // Check current directory
    let cwd_script = PathBuf::from("scripts/export_clip.py");
    if cwd_script.exists() {
        return Some(cwd_script);
    }

    None
}

/// Print a formatted doctor report.
pub fn format_doctor_report(status: &SetupStatus) -> String {
    let mut report = String::new();
    report.push_str("Catalogy Doctor\n");
    report.push_str(&"=".repeat(50));
    report.push('\n');
    report.push('\n');

    report.push_str("Dependencies:\n");
    report.push_str(&format!("  {}\n", status.ffmpeg));
    report.push_str(&format!("  {}\n", status.ffprobe));
    report.push('\n');

    report.push_str("Models:\n");
    report.push_str(&format!("  {}\n", status.visual_model));
    report.push_str(&format!("  {}\n", status.text_model));
    report.push_str(&format!("  {}\n", status.tokenizer));
    report.push('\n');

    report.push_str("Environment:\n");
    report.push_str(&format!("  {}\n", status.data_dir));
    report.push_str(&format!("  {}\n", status.python));
    report.push('\n');

    if status.all_ok() {
        report.push_str("All checks passed.\n");
    } else {
        report.push_str("Some checks failed. Run `catalogy setup` to fix.\n");
    }

    report
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn test_check_dependencies_missing_models() {
        let tmp = TempDir::new().unwrap();
        let model_dir = tmp.path().join("models");
        let data_dir = tmp.path().join("data");
        fs::create_dir_all(&data_dir).unwrap();

        let status = check_dependencies(&model_dir, &data_dir);

        assert!(!status.visual_model.is_ok());
        assert!(!status.text_model.is_ok());
        assert!(!status.tokenizer.is_ok());
        assert!(status.data_dir.is_ok());
    }

    #[test]
    fn test_check_dependencies_with_model_files() {
        let tmp = TempDir::new().unwrap();
        let model_dir = tmp.path().join("models");
        let data_dir = tmp.path().join("data");
        fs::create_dir_all(&model_dir).unwrap();
        fs::create_dir_all(&data_dir).unwrap();

        // Create fake model files
        fs::write(model_dir.join("visual.onnx"), b"fake model data").unwrap();
        fs::write(model_dir.join("text.onnx"), b"fake model data").unwrap();
        fs::write(model_dir.join("tokenizer.json"), b"{}").unwrap();

        let status = check_dependencies(&model_dir, &data_dir);

        assert!(status.visual_model.is_ok());
        assert!(status.text_model.is_ok());
        assert!(status.tokenizer.is_ok());
    }

    #[test]
    fn test_ensure_directories() {
        let tmp = TempDir::new().unwrap();
        let data_dir = tmp.path().join("data");
        let config_dir = tmp.path().join("config");

        ensure_directories(&data_dir, &config_dir).unwrap();

        assert!(data_dir.exists());
        assert!(data_dir.join("models").exists());
        assert!(data_dir.join("thumbs").exists());
        assert!(config_dir.exists());
    }

    #[test]
    fn test_generate_config_with_paths() {
        let tmp = TempDir::new().unwrap();
        let config_path = tmp.path().join("catalogy").join("config.toml");

        let paths = vec!["~/Photos".to_string(), "~/Videos".to_string()];
        generate_config(&config_path, &paths).unwrap();

        let content = fs::read_to_string(&config_path).unwrap();
        assert!(content.contains("[library]"));
        assert!(content.contains("paths = [\"~/Photos\", \"~/Videos\"]"));
        assert!(content.contains("[embedding]"));
        assert!(content.contains("[extraction]"));
        assert!(content.contains("[server]"));
    }

    #[test]
    fn test_generate_config_no_paths() {
        let tmp = TempDir::new().unwrap();
        let config_path = tmp.path().join("config.toml");

        generate_config(&config_path, &[]).unwrap();

        let content = fs::read_to_string(&config_path).unwrap();
        assert!(content.contains("# paths = [\"~/Photos\", \"~/Videos\"]"));
    }

    #[test]
    fn test_copy_models_from_dir() {
        let tmp = TempDir::new().unwrap();
        let src_dir = tmp.path().join("source");
        let dst_dir = tmp.path().join("dest");
        fs::create_dir_all(&src_dir).unwrap();

        fs::write(src_dir.join("visual.onnx"), b"visual data").unwrap();
        fs::write(src_dir.join("text.onnx"), b"text data").unwrap();
        fs::write(src_dir.join("tokenizer.json"), b"{}").unwrap();

        copy_models_from_dir(&src_dir, &dst_dir).unwrap();

        assert!(dst_dir.join("visual.onnx").exists());
        assert!(dst_dir.join("text.onnx").exists());
        assert!(dst_dir.join("tokenizer.json").exists());
    }

    #[test]
    fn test_copy_models_missing_file() {
        let tmp = TempDir::new().unwrap();
        let src_dir = tmp.path().join("source");
        let dst_dir = tmp.path().join("dest");
        fs::create_dir_all(&src_dir).unwrap();

        fs::write(src_dir.join("visual.onnx"), b"visual data").unwrap();

        let result = copy_models_from_dir(&src_dir, &dst_dir);
        assert!(result.is_err());
    }

    #[test]
    fn test_sha256_file() {
        let tmp = TempDir::new().unwrap();
        let file = tmp.path().join("test.txt");
        fs::write(&file, b"hello world").unwrap();

        let hash = sha256_file(&file).unwrap();
        assert_eq!(
            hash,
            "b94d27b9934d3e08a52e52d7da7dabfac484efe37a5380ee9088f7ace2efcde9"
        );
    }

    #[test]
    fn test_check_status_display() {
        let ok = CheckStatus::Ok("ffmpeg found".to_string());
        assert_eq!(format!("{ok}"), "[ok] ffmpeg found");

        let missing = CheckStatus::Missing("not found".to_string());
        assert_eq!(format!("{missing}"), "[missing] not found");

        let error = CheckStatus::Error("failed".to_string());
        assert_eq!(format!("{error}"), "[error] failed");
    }

    #[test]
    fn test_doctor_report_formatting() {
        let status = SetupStatus {
            ffmpeg: CheckStatus::Ok("ffmpeg version 6.0".to_string()),
            ffprobe: CheckStatus::Ok("ffprobe version 6.0".to_string()),
            visual_model: CheckStatus::Ok("visual.onnx (500.0 MB)".to_string()),
            text_model: CheckStatus::Ok("text.onnx (300.0 MB)".to_string()),
            tokenizer: CheckStatus::Ok("tokenizer.json (0.1 MB)".to_string()),
            data_dir: CheckStatus::Ok("Data directory: /tmp/test".to_string()),
            python: CheckStatus::Ok("Python 3.11".to_string()),
        };

        let report = format_doctor_report(&status);
        assert!(report.contains("Catalogy Doctor"));
        assert!(report.contains("Dependencies:"));
        assert!(report.contains("Models:"));
        assert!(report.contains("All checks passed."));
    }

    #[test]
    fn test_doctor_report_with_failures() {
        let status = SetupStatus {
            ffmpeg: CheckStatus::Missing("ffmpeg not found".to_string()),
            ffprobe: CheckStatus::Missing("ffprobe not found".to_string()),
            visual_model: CheckStatus::Missing("visual.onnx not found".to_string()),
            text_model: CheckStatus::Missing("text.onnx not found".to_string()),
            tokenizer: CheckStatus::Missing("tokenizer.json not found".to_string()),
            data_dir: CheckStatus::Ok("Data directory exists".to_string()),
            python: CheckStatus::Missing("python3 not found".to_string()),
        };

        let report = format_doctor_report(&status);
        assert!(report.contains("Some checks failed"));
    }

    #[test]
    fn test_setup_status_all_ok() {
        let status = SetupStatus {
            ffmpeg: CheckStatus::Ok("ok".to_string()),
            ffprobe: CheckStatus::Ok("ok".to_string()),
            visual_model: CheckStatus::Ok("ok".to_string()),
            text_model: CheckStatus::Ok("ok".to_string()),
            tokenizer: CheckStatus::Ok("ok".to_string()),
            data_dir: CheckStatus::Ok("ok".to_string()),
            python: CheckStatus::Ok("ok".to_string()),
        };
        assert!(status.all_ok());
        assert!(status.models_ok());
    }

    #[test]
    fn test_setup_status_partial() {
        let status = SetupStatus {
            ffmpeg: CheckStatus::Ok("ok".to_string()),
            ffprobe: CheckStatus::Missing("missing".to_string()),
            visual_model: CheckStatus::Ok("ok".to_string()),
            text_model: CheckStatus::Ok("ok".to_string()),
            tokenizer: CheckStatus::Ok("ok".to_string()),
            data_dir: CheckStatus::Ok("ok".to_string()),
            python: CheckStatus::Ok("ok".to_string()),
        };
        assert!(!status.all_ok());
        assert!(status.models_ok());
    }
}
