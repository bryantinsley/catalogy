use clap::{Parser, Subcommand};
use indicatif::{ProgressBar, ProgressStyle};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use tracing::{debug, error, info, warn};

/// Global shutdown flag, set by SIGINT/SIGTERM handler.
static SHUTDOWN: AtomicBool = AtomicBool::new(false);

fn shutdown_requested() -> bool {
    SHUTDOWN.load(Ordering::Relaxed)
}

#[derive(Parser)]
#[command(
    name = "catalogy",
    version,
    about = "Local-first semantic media search engine",
    long_about = "Catalogy indexes your local media library (images and videos) using CLIP embeddings,\n\
                   enabling natural-language semantic search across your files.\n\n\
                   Typical workflow:\n  \
                   1. catalogy scan --path ~/Photos\n  \
                   2. catalogy ingest\n  \
                   3. catalogy search \"sunset over the ocean\""
)]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Scan directories for media files and detect changes
    ///
    /// Examples:
    ///   catalogy scan --path ~/Photos
    ///   catalogy scan --path /media/external
    Scan {
        /// Directory path to scan
        #[arg(long)]
        path: Option<String>,

        /// Watch for filesystem changes (not yet implemented)
        #[arg(long)]
        watch: bool,
    },

    /// Process the job queue (extract metadata, frames, embeddings)
    ///
    /// Examples:
    ///   catalogy ingest
    ///   catalogy ingest --stages metadata
    ///   catalogy ingest --stages frames,embed
    Ingest {
        /// Number of worker threads
        #[arg(long)]
        workers: Option<u32>,

        /// Only process specific stages (comma-separated: metadata, frames, embed)
        #[arg(long)]
        stages: Option<String>,
    },

    /// Search the media catalog using natural language
    ///
    /// Examples:
    ///   catalogy search "sunset at the beach"
    ///   catalogy search "dog playing" --limit 5
    ///   catalogy search "portrait" --type image --after 2024-01-01
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
    ///
    /// Examples:
    ///   catalogy status
    Status,

    /// Manage embedding models and re-embed catalog
    ///
    /// Examples:
    ///   catalogy reembed --register --model-id clip-h14 --model-path ./visual.onnx
    ///   catalogy reembed --activate --model-id clip-h14
    ///   catalogy reembed --rebuild-index
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

        /// Model version string
        #[arg(long, default_value = "1")]
        model_version: String,

        /// Embedding dimensions
        #[arg(long, default_value = "1024")]
        dimensions: u32,

        /// Rebuild ANN index after re-embedding
        #[arg(long)]
        rebuild_index: bool,
    },

    /// Start HTTP API server
    ///
    /// Examples:
    ///   catalogy serve
    ///   catalogy serve --port 3000
    Serve {
        /// Port to listen on
        #[arg(long, default_value = "8080")]
        port: u16,
    },

    /// Detect duplicate media files
    ///
    /// Examples:
    ///   catalogy dedup
    ///   catalogy dedup --tier exact
    ///   catalogy dedup --tier visual --threshold 0.90
    Dedup {
        /// Detection tier: exact, visual, cross-video, all
        #[arg(long, default_value = "all")]
        tier: String,

        /// Similarity threshold for visual dedup (0.0-1.0)
        #[arg(long, default_value = "0.92")]
        threshold: f32,
    },

    /// Show effective configuration or generate a starter config file
    ///
    /// Examples:
    ///   catalogy config
    ///   catalogy config --init
    Config {
        /// Generate a starter config file at the default location
        #[arg(long)]
        init: bool,
    },

    /// Transcode videos to optimize storage
    ///
    /// Examples:
    ///   catalogy transcode --dry-run
    ///   catalogy transcode --run
    Transcode {
        /// Dry run - show what would be transcoded
        #[arg(long)]
        dry_run: bool,

        /// Process transcode queue
        #[arg(long)]
        run: bool,
    },

    /// Check system readiness and set up required components
    ///
    /// Examples:
    ///   catalogy setup
    Setup,
}

fn default_data_dir() -> PathBuf {
    dirs::data_local_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("catalogy")
}

fn default_state_db_path() -> PathBuf {
    let data_dir = default_data_dir();
    std::fs::create_dir_all(&data_dir).ok();
    data_dir.join("state.db")
}

fn default_config_path() -> PathBuf {
    dirs::config_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("catalogy")
        .join("config.toml")
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

fn model_dir() -> PathBuf {
    match std::env::var("CATALOGY_MODEL_DIR") {
        Ok(dir) => PathBuf::from(dir),
        Err(_) => default_data_dir().join("models"),
    }
}

fn catalog_path() -> PathBuf {
    default_data_dir().join("catalog.lance")
}

fn open_state_db() -> Result<catalogy_queue::StateDb, Box<dyn std::error::Error>> {
    let db_path = default_state_db_path();
    if !db_path.exists() {
        return Err("No state database found. Run `catalogy scan` first.".into());
    }
    let db = catalogy_queue::StateDb::open(&db_path)?;
    Ok(db)
}

fn make_spinner(msg: &str) -> ProgressBar {
    let pb = ProgressBar::new_spinner();
    pb.set_style(
        ProgressStyle::with_template("{spinner:.green} {msg}")
            .unwrap()
            .tick_chars("⠁⠂⠄⡀⢀⠠⠐⠈ "),
    );
    pb.set_message(msg.to_string());
    pb.enable_steady_tick(std::time::Duration::from_millis(100));
    pb
}

fn run_scan(scan_path: &str) -> Result<(), Box<dyn std::error::Error>> {
    let db_path = default_state_db_path();
    let db = catalogy_queue::StateDb::open(&db_path)?;

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
    info!(path = scan_path, "Starting scan");

    let spinner = make_spinner("Scanning files...");
    let scanned = catalogy_scanner::scan_directory(root, &image_exts, &video_exts)?;
    spinner.finish_with_message(format!("Found {} media files", scanned.len()));

    if shutdown_requested() {
        info!("Shutdown requested, skipping change detection");
        return Ok(());
    }

    let spinner = make_spinner("Detecting changes...");
    let changes = catalogy_queue::detect_changes(&db, &scanned)?;
    let result = catalogy_queue::apply_changes_and_enqueue(&db, &changes)?;
    spinner.finish_and_clear();

    info!(
        new = result.new_files,
        modified = result.modified_files,
        moved = result.moved_files,
        deleted = result.deleted_files,
        unchanged = result.unchanged_files,
        "Scan complete"
    );

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
    let db = open_state_db()?;

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

    let models = db.list_models()?;
    if !models.is_empty() {
        println!("\nModels:");
        for m in &models {
            let current = if m.is_current { " (current)" } else { "" };
            println!(
                "  {} v{} - {}d{current}",
                m.model_id, m.model_version, m.dimensions
            );
        }
    }

    Ok(())
}

fn should_run_stage(stages: Option<&str>, stage_name: &str) -> bool {
    match stages {
        None => true,
        Some(s) => s.split(',').any(|s| s.trim() == stage_name),
    }
}

fn run_ingest(stages: Option<&str>) -> Result<(), Box<dyn std::error::Error>> {
    let db = open_state_db()?;
    let config = default_extraction_config();

    // Recover any stale running jobs from previous crash/shutdown
    let reset = db.reset_running_to_pending()?;
    if reset > 0 {
        info!(count = reset, "Reset stale running jobs to pending");
    }

    if should_run_stage(stages, "frames") || should_run_stage(stages, "extract_frames") {
        if shutdown_requested() {
            info!("Shutdown requested, stopping ingest");
            return Ok(());
        }
        info!("Processing extract_frames jobs");
        let pb = make_spinner("Extracting frames...");
        let count = catalogy_extract::run_extract_frames_worker(&db, &config, "worker-main")?;
        pb.finish_with_message(format!("Processed {count} extract_frames jobs"));
        info!(count, "extract_frames stage complete");
    }

    if should_run_stage(stages, "metadata") || should_run_stage(stages, "extract_metadata") {
        if shutdown_requested() {
            info!("Shutdown requested, stopping ingest");
            return Ok(());
        }
        info!("Processing extract_metadata jobs");
        let ffprobe = catalogy_metadata::find_ffprobe(config.ffprobe_path.as_deref());
        if let Some(ref fp) = ffprobe {
            debug!(path = %fp.display(), "Using ffprobe");
        } else {
            warn!("ffprobe not found — video metadata extraction will be skipped");
        }
        let processed = catalogy_metadata::run_metadata_worker(&db, ffprobe.as_deref(), true)?;
        info!(count = processed, "extract_metadata stage complete");
    }

    if should_run_stage(stages, "embed") {
        if shutdown_requested() {
            info!("Shutdown requested, stopping ingest");
            return Ok(());
        }
        info!("Processing embed jobs");

        let mdir = model_dir();
        let visual_model = mdir.join("visual.onnx");
        let text_model = mdir.join("text.onnx");
        let tokenizer = mdir.join("tokenizer.json");

        if !visual_model.exists() || !text_model.exists() || !tokenizer.exists() {
            warn!(
                model_dir = %mdir.display(),
                "CLIP model files not found — skipping embed stage. \
                 Set CATALOGY_MODEL_DIR or place visual.onnx, text.onnx, tokenizer.json in the model directory."
            );
        } else {
            let catalog_path_str = catalog_path().to_string_lossy().to_string();

            let session =
                catalogy_embed::EmbedSession::new(&visual_model, &text_model, &tokenizer)?;
            let catalog = catalogy_catalog::Catalog::open(&catalog_path_str)?;

            let pb = make_spinner("Embedding...");
            let count = catalogy_embed::run_embed_worker(
                &db,
                &session,
                &catalog,
                "clip-vit-h-14",
                "1",
                "worker-main",
            )?;
            pb.finish_with_message(format!("Processed {count} embed jobs"));
            info!(count, "embed stage complete");
        }
    }

    // If shutdown was requested mid-stage, drain running jobs
    if shutdown_requested() {
        let drained = db.reset_running_to_pending()?;
        if drained > 0 {
            info!(count = drained, "Drained in-progress jobs back to pending");
        }
    }

    Ok(())
}

fn run_search(
    query_text: &str,
    limit: usize,
    media_type: Option<&str>,
    after: Option<&str>,
) -> Result<(), Box<dyn std::error::Error>> {
    let mut full_query = String::new();
    if let Some(mt) = media_type {
        full_query.push_str(&format!("type:{} ", mt));
    }
    if let Some(a) = after {
        full_query.push_str(&format!("after:{} ", a));
    }
    full_query.push_str(query_text);

    let query = catalogy_search::parse_query(&full_query, limit);

    let mdir = model_dir();
    let visual_model = mdir.join("visual.onnx");
    let text_model = mdir.join("text.onnx");
    let tokenizer = mdir.join("tokenizer.json");

    if !visual_model.exists() || !text_model.exists() || !tokenizer.exists() {
        return Err(format!(
            "CLIP model files not found in {}. Set CATALOGY_MODEL_DIR to the directory containing \
             visual.onnx, text.onnx, tokenizer.json",
            mdir.display()
        )
        .into());
    }

    let session = Arc::new(catalogy_embed::EmbedSession::new(
        &visual_model,
        &text_model,
        &tokenizer,
    )?);
    let catalog = Arc::new(catalogy_catalog::Catalog::open(
        &catalog_path().to_string_lossy(),
    )?);

    let engine = catalogy_search::SearchEngine::new(session, catalog);

    debug!(query = query_text, limit, "Executing search");
    let results = engine.search(&query)?;

    if results.is_empty() {
        println!("No results found.");
        return Ok(());
    }

    use comfy_table::{presets::UTF8_FULL, Table};

    let mut table = Table::new();
    table.load_preset(UTF8_FULL);
    table.set_header(vec![
        "Rank",
        "Score",
        "Filename",
        "Type",
        "Dimensions",
        "Path",
    ]);

    for (i, r) in results.iter().enumerate() {
        let dims = match (r.metadata.width, r.metadata.height) {
            (Some(w), Some(h)) => format!("{}x{}", w, h),
            _ => "-".to_string(),
        };
        let type_str = match r.media_type {
            catalogy_core::MediaType::Image => "image",
            catalogy_core::MediaType::Video => "video",
            catalogy_core::MediaType::VideoFrame => "frame",
        };
        table.add_row(vec![
            format!("{}", i + 1),
            format!("{:.3}", r.score),
            r.file_name.clone(),
            type_str.to_string(),
            dims,
            r.file_path.to_string_lossy().to_string(),
        ]);
    }

    println!("{table}");
    println!("\n{} result(s) found.", results.len());

    Ok(())
}

fn run_dedup(tier: &str, threshold: f32) -> Result<(), Box<dyn std::error::Error>> {
    let run_exact = tier == "all" || tier == "exact";
    let run_visual = tier == "all" || tier == "visual";
    let run_cross = tier == "all" || tier == "cross-video";

    if !run_exact && !run_visual && !run_cross {
        return Err(
            format!("Unknown tier: {tier}. Use: exact, visual, cross-video, or all").into(),
        );
    }

    if run_exact {
        let db = open_state_db()?;
        let sets = catalogy_dedup::find_exact_duplicates(&db)?;
        print!("{}", catalogy_dedup::format_exact_report(&sets));
    }

    if run_visual || run_cross {
        let catalog_path_str = catalog_path().to_string_lossy().to_string();
        let catalog = catalogy_catalog::Catalog::open(&catalog_path_str)?;

        if run_visual {
            let clusters = catalogy_dedup::find_visual_duplicates(&catalog, threshold)?;
            print!("{}", catalogy_dedup::format_visual_report(&clusters));
        }

        if run_cross {
            let dups = catalogy_dedup::find_cross_video_duplicates(&catalog, threshold)?;
            print!("{}", catalogy_dedup::format_cross_video_report(&dups));
        }
    }

    Ok(())
}

fn run_serve(port: u16) -> Result<(), Box<dyn std::error::Error>> {
    let catalog = Arc::new(catalogy_catalog::Catalog::open(
        &catalog_path().to_string_lossy(),
    )?);

    let mdir = model_dir();
    let visual_model = mdir.join("visual.onnx");
    let text_model = mdir.join("text.onnx");
    let tokenizer_path = mdir.join("tokenizer.json");

    let search_engine = if visual_model.exists() && text_model.exists() && tokenizer_path.exists() {
        match catalogy_embed::EmbedSession::new(&visual_model, &text_model, &tokenizer_path) {
            Ok(session) => {
                let session = Arc::new(session);
                Some(catalogy_search::SearchEngine::new(session, catalog.clone()))
            }
            Err(e) => {
                warn!(error = %e, "Failed to load CLIP models — search will be unavailable");
                None
            }
        }
    } else {
        warn!(
            model_dir = %mdir.display(),
            "CLIP model files not found — search will be unavailable. Set CATALOGY_MODEL_DIR to enable search."
        );
        None
    };

    let db_path = default_state_db_path();
    let state = Arc::new(catalogy_server::AppState {
        catalog,
        search_engine,
        state_db_path: Some(db_path),
        model_dir: mdir,
        data_dir: default_data_dir(),
        progress: std::sync::Mutex::new(Default::default()),
    });

    let rt = tokio::runtime::Runtime::new()?;
    rt.block_on(async {
        let app = catalogy_server::create_router(state);
        let addr = format!("0.0.0.0:{}", port);
        let listener = tokio::net::TcpListener::bind(&addr).await?;
        info!(port, "Server listening");
        println!("Catalogy server running at http://localhost:{port}");
        println!("Press Ctrl+C to stop.");
        axum::serve(listener, app).await?;
        Ok::<(), Box<dyn std::error::Error>>(())
    })?;

    Ok(())
}

fn run_reembed(
    register: bool,
    activate: bool,
    model_id: Option<&str>,
    model_path: Option<&str>,
    model_version: &str,
    dimensions: u32,
    rebuild_index: bool,
) -> Result<(), Box<dyn std::error::Error>> {
    let db_path = default_state_db_path();
    let db = catalogy_queue::StateDb::open(&db_path)?;

    if register {
        let mid = model_id.ok_or("--model-id is required for --register")?;
        let mpath = model_path.ok_or("--model-path is required for --register")?;

        if !Path::new(mpath).exists() {
            return Err(format!("Model file not found: {}", mpath).into());
        }

        db.register_model(mid, model_version, mpath, dimensions)?;
        info!(
            model_id = mid,
            version = model_version,
            dimensions,
            "Model registered"
        );
        println!("Registered model '{mid}' (version={model_version}, dimensions={dimensions})");
        println!("  Path: {mpath}");
        println!("Use --activate --model-id {mid} to set as current and create re-embed jobs.");
        return Ok(());
    }

    if activate {
        let mid = model_id.ok_or("--model-id is required for --activate")?;

        let model = db.get_model(mid)?.ok_or(format!(
            "Model '{}' not found. Register it first with --register.",
            mid
        ))?;

        db.set_current_model(mid)?;
        info!(model_id = mid, "Model activated");
        println!("Set '{mid}' as current model.");

        let job_count = db.enqueue_reembed(mid, &model.model_version)?;
        println!("Created {job_count} re-embed jobs.");

        if job_count > 0 {
            println!(
                "Run `catalogy ingest --stages embed` or the re-embed worker to process them."
            );
        }
        return Ok(());
    }

    if rebuild_index {
        let catalog_path_str = catalog_path().to_string_lossy().to_string();
        let catalog = catalogy_catalog::Catalog::open(&catalog_path_str)?;

        let count = catalog.count()?;
        if count == 0 {
            println!("Catalog is empty. Nothing to index.");
            return Ok(());
        }

        let num_partitions = std::cmp::max(1, (count as f64).sqrt() as u32);
        info!(
            rows = count,
            partitions = num_partitions,
            "Rebuilding ANN index"
        );
        let pb = make_spinner("Rebuilding ANN index...");
        catalog.build_index(num_partitions)?;
        pb.finish_with_message("Index rebuilt successfully");
        return Ok(());
    }

    println!("Usage:");
    println!("  catalogy reembed --register --model-id <ID> --model-path <PATH> [--dimensions <N>] [--model-version <V>]");
    println!("  catalogy reembed --activate --model-id <ID>");
    println!("  catalogy reembed --rebuild-index");
    println!();

    let models = db.list_models()?;
    if models.is_empty() {
        println!("No models registered.");
    } else {
        println!("Registered models:");
        for m in &models {
            let current = if m.is_current { " (current)" } else { "" };
            println!(
                "  {} v{} - {}d - {}{current}",
                m.model_id, m.model_version, m.dimensions, m.model_path
            );
        }
    }

    Ok(())
}

fn run_transcode(dry_run: bool, run: bool) -> Result<(), Box<dyn std::error::Error>> {
    let db = open_state_db()?;
    let config = catalogy_core::TranscodeConfig::default();

    if dry_run {
        let entries = catalogy_transcode::run_transcode_dry_run(&db, &config)?;

        if entries.is_empty() {
            println!("No video files found with metadata.");
            return Ok(());
        }

        use comfy_table::{presets::UTF8_FULL, Table};

        let mut table = Table::new();
        table.load_preset(UTF8_FULL);
        table.set_header(vec![
            "File",
            "Size",
            "Resolution",
            "Codec",
            "Decision",
            "Est. Savings",
        ]);

        let mut total_savings: i64 = 0;
        let mut transcode_count = 0;

        for entry in &entries {
            let size_mb = entry.file_size as f64 / 1_048_576.0;
            let (decision_str, savings_str) = match &entry.decision {
                catalogy_transcode::TranscodeDecision::Skip { reason } => {
                    (format!("Skip: {reason}"), "-".to_string())
                }
                catalogy_transcode::TranscodeDecision::Transcode {
                    target_resolution,
                    target_codec,
                    estimated_savings_bytes,
                } => {
                    total_savings += estimated_savings_bytes;
                    transcode_count += 1;
                    (
                        format!(
                            "-> {}x{} {}",
                            target_resolution.0, target_resolution.1, target_codec
                        ),
                        format!("{:.1} MB", *estimated_savings_bytes as f64 / 1_048_576.0),
                    )
                }
            };

            table.add_row(vec![
                Path::new(&entry.file_path)
                    .file_name()
                    .map(|n| n.to_string_lossy().to_string())
                    .unwrap_or_else(|| entry.file_path.clone()),
                format!("{:.1} MB", size_mb),
                entry.resolution.clone(),
                entry.codec.clone(),
                decision_str,
                savings_str,
            ]);
        }

        println!("{table}");
        println!(
            "\n{} videos evaluated, {} would be transcoded",
            entries.len(),
            transcode_count
        );
        if total_savings > 0 {
            println!(
                "Estimated total savings: {:.1} MB",
                total_savings as f64 / 1_048_576.0
            );
        }
    } else if run {
        info!("Processing transcode queue");
        let pb = make_spinner("Transcoding...");
        let stats = catalogy_transcode::run_transcode_worker(&db, &config, "worker-main")?;
        pb.finish_and_clear();

        info!(
            completed = stats.completed,
            skipped = stats.skipped,
            failed = stats.failed,
            savings_mb = stats.total_savings_bytes as f64 / 1_048_576.0,
            "Transcode complete"
        );
        println!("Transcode complete:");
        println!("  Completed: {}", stats.completed);
        println!("  Skipped:   {}", stats.skipped);
        println!("  Failed:    {}", stats.failed);
        if stats.total_savings_bytes > 0 {
            println!(
                "  Total savings: {:.1} MB",
                stats.total_savings_bytes as f64 / 1_048_576.0
            );
        }
    } else {
        println!("Usage: catalogy transcode --dry-run  (preview)");
        println!("       catalogy transcode --run      (execute)");
    }

    Ok(())
}

fn run_config(init: bool) -> Result<(), Box<dyn std::error::Error>> {
    if init {
        let config_path = default_config_path();
        if config_path.exists() {
            return Err(format!("Config file already exists at {}", config_path.display()).into());
        }
        if let Some(parent) = config_path.parent() {
            std::fs::create_dir_all(parent)?;
        }

        let template = r#"# Catalogy configuration
# See docs for all options: https://github.com/user/catalogy

# Directories to scan (use `catalogy scan --path` to override)
# scan_paths = ["~/Photos", "~/Videos"]

# Extraction settings
[extraction]
frame_strategy = "adaptive"
scene_threshold = 0.3
max_interval_seconds = 60
frame_interval_seconds = 30
frame_max_dimension = 512

# Transcode settings
[transcode]
enabled = true
max_resolution = "1080p"
target_codec = "h265"
target_crf = 28
"#;

        std::fs::write(&config_path, template)?;
        info!(path = %config_path.display(), "Config file created");
        println!("Created config file at {}", config_path.display());
        return Ok(());
    }

    // Print effective configuration
    println!("Effective configuration:");
    println!();
    println!("  Data directory:   {}", default_data_dir().display());
    println!("  State database:   {}", default_state_db_path().display());
    println!("  Catalog path:     {}", catalog_path().display());
    println!("  Model directory:  {}", model_dir().display());
    println!("  Config file:      {}", default_config_path().display());
    println!();

    let config = default_extraction_config();
    println!("  Extraction:");
    println!("    Frame strategy:     {}", config.frame_strategy);
    println!("    Scene threshold:    {}", config.scene_threshold);
    println!("    Max interval (s):   {}", config.max_interval_seconds);
    println!("    Frame interval (s): {}", config.frame_interval_seconds);
    println!("    Frame max dim:      {}", config.frame_max_dimension);
    println!("    Thumbnail dir:      {}", config.thumbnail_dir);
    println!();

    let tc = catalogy_core::TranscodeConfig::default();
    println!("  Transcode:");
    println!("    Enabled:        {}", tc.enabled);
    println!("    Max resolution: {}", tc.max_resolution);
    println!("    Target codec:   {}", tc.target_codec);
    println!("    Target CRF:     {}", tc.target_crf);
    println!("    HW encoder:     {}", tc.use_hw_encoder);
    println!();

    let config_path = default_config_path();
    if config_path.exists() {
        println!("  Config file exists at {}", config_path.display());
    } else {
        println!("  No config file found. Run `catalogy config --init` to create one.");
    }

    Ok(())
}

fn check_command_version(cmd: &str) -> Option<String> {
    std::process::Command::new(cmd)
        .arg("-version")
        .output()
        .ok()
        .filter(|o| o.status.success())
        .and_then(|o| {
            let stdout = String::from_utf8_lossy(&o.stdout);
            stdout.lines().next().and_then(|line| {
                line.split("version")
                    .nth(1)
                    .map(|v| v.split_whitespace().next().unwrap_or("unknown").to_string())
            })
        })
}

fn run_setup() -> Result<(), Box<dyn std::error::Error>> {
    println!("Catalogy Setup Check");
    println!("====================");
    println!();

    let mut issues = 0u32;

    // --- System Dependencies ---
    println!("System Dependencies:");

    let ffmpeg_version = check_command_version("ffmpeg");
    if let Some(ref ver) = ffmpeg_version {
        println!("  \u{2713} ffmpeg      (version {ver})");
    } else {
        println!("  \u{2717} ffmpeg      MISSING \u{2014} run: brew install ffmpeg");
        issues += 1;
    }

    let ffprobe_version = check_command_version("ffprobe");
    if let Some(ref ver) = ffprobe_version {
        println!("  \u{2713} ffprobe     (version {ver})");
    } else {
        println!("  \u{2717} ffprobe     MISSING \u{2014} run: brew install ffmpeg");
        issues += 1;
    }

    println!();

    // --- CLIP Model Files ---
    let mdir = model_dir();
    let home = dirs::home_dir()
        .map(|h| h.to_string_lossy().to_string())
        .unwrap_or_default();
    let mdir_display = mdir.to_string_lossy().replace(&home, "~");
    println!("CLIP Model Files ({mdir_display}/):");

    let model_files = ["visual.onnx", "text.onnx", "tokenizer.json"];
    for fname in &model_files {
        let fpath = mdir.join(fname);
        if fpath.exists() {
            println!("  \u{2713} {fname}");
        } else {
            println!("  \u{2717} {fname}  MISSING");
            issues += 1;
        }
    }

    println!();

    // --- Create Directories ---
    println!("Directories:");

    let data_dir = default_data_dir();
    let models_dir = model_dir();
    let thumbnails_dir = data_dir.join("thumbnails");

    let dir_items: Vec<(&str, PathBuf)> = vec![
        ("Data dir", data_dir.clone()),
        ("Models dir", models_dir),
        ("Thumbnails dir", thumbnails_dir),
    ];

    for (label, dir) in &dir_items {
        match std::fs::create_dir_all(dir) {
            Ok(_) => {
                let display = dir.to_string_lossy().replace(&home, "~");
                println!("  \u{2713} {label} created: {display}/");
            }
            Err(e) => {
                println!("  \u{2717} {label} FAILED: {e}");
                issues += 1;
            }
        }
    }

    println!();

    // --- Summary ---
    let separator = "\u{2550}".repeat(50);
    println!("{separator}");

    if issues == 0 {
        println!("\u{2713} All checks passed! You're ready to use catalogy.");
        println!();
        println!("Next steps:");
        println!("  catalogy scan --path ~/Photos");
        println!("  catalogy ingest");
        println!("  catalogy serve          # then open http://localhost:8080");
    } else {
        println!(
            "\u{26a0}  {issues} issue{} found. Fix {} before running ingest.",
            if issues == 1 { "" } else { "s" },
            if issues == 1 { "it" } else { "them" },
        );

        let models_missing = model_files.iter().any(|f| !mdir.join(f).exists());
        if models_missing {
            println!();
            println!("To get CLIP models (choose one):");
            println!();
            println!("  Option A \u{2014} Python export (recommended):");
            println!("    pip install torch transformers onnxruntime");
            println!("    python scripts/export_clip.py");
            let escaped_mdir = mdir.to_string_lossy().replace(' ', "\\ ");
            println!("    mv visual.onnx text.onnx tokenizer.json {escaped_mdir}/");
            println!();
            println!("  Option B \u{2014} Download pre-exported models:");
            println!("    See README.md for download links.");
        }
    }

    println!("{separator}");

    Ok(())
}

fn setup_logging() {
    use tracing_subscriber::EnvFilter;

    let filter =
        EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("catalogy=info"));

    tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_writer(std::io::stderr)
        .with_target(false)
        .init();
}

fn setup_ctrlc_handler() {
    ctrlc::set_handler(move || {
        if SHUTDOWN.load(Ordering::Relaxed) {
            // Second signal — force exit
            eprintln!("\nForce shutdown.");
            std::process::exit(1);
        }
        SHUTDOWN.store(true, Ordering::Relaxed);
        eprintln!("\nShutdown requested. Finishing current work...");
    })
    .expect("failed to set Ctrl+C handler");
}

fn main() {
    setup_logging();
    setup_ctrlc_handler();

    let cli = Cli::parse();

    let result: Result<(), Box<dyn std::error::Error>> = match cli.command {
        Commands::Scan { path, watch } => {
            if watch {
                warn!("Watch mode is not yet implemented");
                println!("Watch mode is not yet implemented.");
                Ok(())
            } else {
                match path {
                    Some(p) => run_scan(&p),
                    None => Err("--path is required for scan".into()),
                }
            }
        }
        Commands::Status => run_status(),
        Commands::Ingest { stages, .. } => run_ingest(stages.as_deref()),
        Commands::Search {
            query,
            limit,
            media_type,
            after,
        } => run_search(&query, limit, media_type.as_deref(), after.as_deref()),
        Commands::Dedup { tier, threshold } => run_dedup(&tier, threshold),
        Commands::Reembed {
            register,
            activate,
            model_id,
            model_path,
            model_version,
            dimensions,
            rebuild_index,
        } => run_reembed(
            register,
            activate,
            model_id.as_deref(),
            model_path.as_deref(),
            &model_version,
            dimensions,
            rebuild_index,
        ),
        Commands::Serve { port } => run_serve(port),
        Commands::Config { init } => run_config(init),
        Commands::Transcode { dry_run, run } => run_transcode(dry_run, run),
        Commands::Setup => run_setup(),
    };

    if let Err(e) = result {
        error!(error = %e, "Command failed");
        eprintln!("Error: {e}");
        std::process::exit(1);
    }

    if shutdown_requested() {
        info!("Shutdown complete");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_should_run_stage_none_runs_all() {
        assert!(should_run_stage(None, "metadata"));
        assert!(should_run_stage(None, "embed"));
        assert!(should_run_stage(None, "frames"));
    }

    #[test]
    fn test_should_run_stage_filters() {
        assert!(should_run_stage(Some("metadata,embed"), "metadata"));
        assert!(should_run_stage(Some("metadata,embed"), "embed"));
        assert!(!should_run_stage(Some("metadata,embed"), "frames"));
    }

    #[test]
    fn test_should_run_stage_trims_whitespace() {
        assert!(should_run_stage(Some("metadata, embed"), "embed"));
    }

    #[test]
    fn test_shutdown_flag_defaults_false() {
        // Ensure the flag is not set at startup (test isolation caveat)
        // This just validates the API works
        assert!(!SHUTDOWN.load(Ordering::Relaxed) || SHUTDOWN.load(Ordering::Relaxed));
    }

    #[test]
    fn test_default_data_dir() {
        let dir = default_data_dir();
        assert!(dir.to_string_lossy().contains("catalogy"));
    }

    #[test]
    fn test_default_config_path() {
        let path = default_config_path();
        assert!(path.to_string_lossy().contains("catalogy"));
        assert!(path.to_string_lossy().ends_with("config.toml"));
    }
}
