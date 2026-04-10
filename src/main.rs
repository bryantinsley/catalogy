use clap::{Parser, Subcommand};
use std::path::{Path, PathBuf};

#[derive(Parser)]
#[command(
    name = "catalogy",
    version,
    about = "Local-first semantic media search engine"
)]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Scan directories for media files
    Scan {
        /// Directory path to scan
        #[arg(long)]
        path: Option<String>,

        /// Watch for filesystem changes
        #[arg(long)]
        watch: bool,
    },

    /// Process job queue (extract, embed, index)
    Ingest {
        /// Number of worker threads
        #[arg(long)]
        workers: Option<u32>,

        /// Only process specific stages (comma-separated: metadata,frames)
        #[arg(long)]
        stages: Option<String>,
    },

    /// Search the media catalog
    Search {
        /// Search query text
        query: String,

        /// Maximum number of results
        #[arg(long, default_value = "20")]
        limit: usize,

        /// Filter by media type (image, video)
        #[arg(long, name = "type")]
        media_type: Option<String>,

        /// Filter by date (after)
        #[arg(long)]
        after: Option<String>,
    },

    /// Show queue and catalog statistics
    Status,

    /// Manage embedding models and re-embed catalog
    Reembed {
        /// Register a new model
        #[arg(long)]
        register: bool,

        /// Activate a model for re-embedding
        #[arg(long)]
        activate: bool,

        /// Model ID
        #[arg(long)]
        model_id: Option<String>,

        /// Path to ONNX model file
        #[arg(long)]
        model_path: Option<String>,

        /// Rebuild ANN index after re-embedding
        #[arg(long)]
        rebuild_index: bool,
    },

    /// Start HTTP API server
    Serve {
        /// Port to listen on
        #[arg(long, default_value = "8080")]
        port: u16,
    },

    /// Show or edit configuration
    Config,
}

fn default_state_db_path() -> PathBuf {
    let data_dir = dirs::data_local_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("catalogy");
    std::fs::create_dir_all(&data_dir).ok();
    data_dir.join("state.db")
}

fn default_extraction_config() -> catalogy_core::ExtractionConfig {
    catalogy_core::ExtractionConfig {
        frame_strategy: "adaptive".to_string(),
        scene_threshold: 0.3,
        max_interval_seconds: 60,
        frame_interval_seconds: 30,
        frame_max_dimension: 512,
        dedup_similarity_threshold: 0.95,
        ffprobe_path: None,
        thumbnail_dir: "~/.local/share/catalogy/thumbs".to_string(),
    }
}

fn run_scan(scan_path: &str) -> Result<(), Box<dyn std::error::Error>> {
    let db_path = default_state_db_path();
    let db = catalogy_queue::StateDb::open(&db_path)?;

    // Default extensions
    let image_exts: Vec<String> = vec![
        "jpg", "jpeg", "png", "gif", "bmp", "tiff", "tif", "webp", "heic", "heif", "avif",
    ]
    .into_iter()
    .map(String::from)
    .collect();
    let video_exts: Vec<String> = vec![
        "mp4", "mov", "avi", "mkv", "wmv", "flv", "webm", "m4v", "mpg", "mpeg",
    ]
    .into_iter()
    .map(String::from)
    .collect();

    let root = Path::new(scan_path);
    println!("Scanning {scan_path}...");

    let scanned = catalogy_scanner::scan_directory(root, &image_exts, &video_exts)?;
    println!("Found {} media files", scanned.len());

    let changes = catalogy_queue::detect_changes(&db, &scanned)?;
    let result = catalogy_queue::apply_changes_and_enqueue(&db, &changes)?;

    println!("Scan complete:");
    println!("  New:       {}", result.new_files);
    println!("  Modified:  {}", result.modified_files);
    println!("  Moved:     {}", result.moved_files);
    println!("  Deleted:   {}", result.deleted_files);
    println!("  Unchanged: {}", result.unchanged_files);

    db.set_config("last_scan_time", &chrono::Utc::now().to_rfc3339())?;

    Ok(())
}

fn run_status() -> Result<(), Box<dyn std::error::Error>> {
    let db_path = default_state_db_path();
    if !db_path.exists() {
        println!("No state database found. Run `catalogy scan` first.");
        return Ok(());
    }

    let db = catalogy_queue::StateDb::open(&db_path)?;

    let file_count = db.file_count()?;
    println!("Tracked files: {file_count}");

    let stats = db.stats()?;
    println!("\nJob Queue:");
    println!("  Pending:   {}", stats.pending);
    println!("  Running:   {}", stats.running);
    println!("  Completed: {}", stats.completed);
    println!("  Failed:    {}", stats.failed);
    println!("  Skipped:   {}", stats.skipped);

    if !stats.by_stage.is_empty() {
        println!("\nBy Stage:");
        println!(
            "  {:<20} {:>8} {:>8} {:>10} {:>8} {:>8}",
            "Stage", "Pending", "Running", "Completed", "Failed", "Skipped"
        );
        for (stage, p, r, c, f, s) in &stats.by_stage {
            println!("  {stage:<20} {p:>8} {r:>8} {c:>10} {f:>8} {s:>8}");
        }
    }

    if let Some(last_scan) = db.get_config("last_scan_time")? {
        println!("\nLast scan: {last_scan}");
    }

    Ok(())
}

fn should_run_stage(stages: Option<&str>, stage_name: &str) -> bool {
    match stages {
        None => true, // No filter = run all stages
        Some(s) => s.split(',').any(|s| s.trim() == stage_name),
    }
}

fn run_ingest(stages: Option<&str>) -> Result<(), Box<dyn std::error::Error>> {
    let db_path = default_state_db_path();
    if !db_path.exists() {
        println!("No state database found. Run `catalogy scan` first.");
        return Ok(());
    }

    let db = catalogy_queue::StateDb::open(&db_path)?;
    let config = default_extraction_config();

    // Stage: extract_frames
    if should_run_stage(stages, "frames") || should_run_stage(stages, "extract_frames") {
        println!("Processing extract_frames jobs...");
        let count = catalogy_extract::run_extract_frames_worker(&db, &config, "worker-main")?;
        println!("Processed {count} extract_frames jobs.");
    }

    // Stage: extract_metadata
    if should_run_stage(stages, "metadata") || should_run_stage(stages, "extract_metadata") {
        println!("Processing extract_metadata jobs...");
        let ffprobe = catalogy_metadata::find_ffprobe(config.ffprobe_path.as_deref());
        if let Some(ref fp) = ffprobe {
            println!("Using ffprobe: {}", fp.display());
        } else {
            println!("Warning: ffprobe not found. Video metadata extraction will be skipped.");
        }
        let processed = catalogy_metadata::run_metadata_worker(&db, ffprobe.as_deref(), true)?;
        println!("Processed {processed} metadata jobs.");
    }

    Ok(())
}

fn main() {
    let cli = Cli::parse();

    match cli.command {
        Commands::Scan { path, watch } => {
            if watch {
                println!("Watch mode is not yet implemented.");
                return;
            }

            let scan_path = match path {
                Some(p) => p,
                None => {
                    eprintln!("Error: --path is required");
                    std::process::exit(1);
                }
            };

            if let Err(e) = run_scan(&scan_path) {
                eprintln!("Error: {e}");
                std::process::exit(1);
            }
        }
        Commands::Status => {
            if let Err(e) = run_status() {
                eprintln!("Error: {e}");
                std::process::exit(1);
            }
        }
        Commands::Ingest { stages, .. } => {
            if let Err(e) = run_ingest(stages.as_deref()) {
                eprintln!("Error: {e}");
                std::process::exit(1);
            }
        }
        Commands::Search { .. } => {
            println!("search: not yet implemented");
        }
        Commands::Reembed { .. } => {
            println!("reembed: not yet implemented");
        }
        Commands::Serve { .. } => {
            println!("serve: not yet implemented");
        }
        Commands::Config => {
            println!("config: not yet implemented");
        }
    }
}
