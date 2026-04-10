use clap::{Parser, Subcommand};
use std::path::{Path, PathBuf};
use std::sync::Arc;

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
    Serve {
        /// Port to listen on
        #[arg(long, default_value = "8080")]
        port: u16,
    },

    /// Detect duplicate media files
    Dedup {
        /// Detection tier: exact, visual, cross-video, all
        #[arg(long, default_value = "all")]
        tier: String,

        /// Similarity threshold for visual dedup (0.0-1.0)
        #[arg(long, default_value = "0.92")]
        threshold: f32,
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

fn model_dir() -> PathBuf {
    match std::env::var("CATALOGY_MODEL_DIR") {
        Ok(dir) => PathBuf::from(dir),
        Err(_) => dirs::data_local_dir()
            .unwrap_or_else(|| PathBuf::from("."))
            .join("catalogy")
            .join("models"),
    }
}

fn catalog_path() -> PathBuf {
    dirs::data_local_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("catalogy")
        .join("catalog.lance")
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

    // Show registered models
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
    let db_path = default_state_db_path();
    if !db_path.exists() {
        println!("No state database found. Run `catalogy scan` first.");
        return Ok(());
    }

    let db = catalogy_queue::StateDb::open(&db_path)?;
    let config = default_extraction_config();

    if should_run_stage(stages, "frames") || should_run_stage(stages, "extract_frames") {
        println!("Processing extract_frames jobs...");
        let count = catalogy_extract::run_extract_frames_worker(&db, &config, "worker-main")?;
        println!("Processed {count} extract_frames jobs.");
    }

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

    if should_run_stage(stages, "embed") {
        println!("Processing embed jobs...");

        let mdir = model_dir();
        let visual_model = mdir.join("visual.onnx");
        let text_model = mdir.join("text.onnx");
        let tokenizer = mdir.join("tokenizer.json");

        if !visual_model.exists() || !text_model.exists() || !tokenizer.exists() {
            println!(
                "Warning: CLIP model files not found in {}. Set CATALOGY_MODEL_DIR or place models at the default location.",
                mdir.display()
            );
            println!("  Expected: visual.onnx, text.onnx, tokenizer.json");
            println!("  Skipping embed stage.");
        } else {
            let catalog_path_str = catalog_path().to_string_lossy().to_string();

            let session =
                catalogy_embed::EmbedSession::new(&visual_model, &text_model, &tokenizer)?;
            let catalog = catalogy_catalog::Catalog::open(&catalog_path_str)?;

            let count = catalogy_embed::run_embed_worker(
                &db,
                &session,
                &catalog,
                "clip-vit-h-14",
                "1",
                "worker-main",
            )?;
            println!("Processed {count} embed jobs.");
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
        eprintln!("Error: CLIP model files not found in {}", mdir.display());
        eprintln!("Set CATALOGY_MODEL_DIR to the directory containing:");
        eprintln!("  visual.onnx, text.onnx, tokenizer.json");
        std::process::exit(1);
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
    let results = engine.search(&query)?;

    if results.is_empty() {
        println!("No results found.");
        return Ok(());
    }

    use comfy_table::{presets::UTF8_FULL, Table};

    let mut table = Table::new();
    table.load_preset(UTF8_FULL);
    table.set_header(vec!["Rank", "Score", "Filename", "Type", "Dimensions", "Path"]);

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
        eprintln!("Unknown tier: {tier}. Use: exact, visual, cross-video, or all");
        std::process::exit(1);
    }

    if run_exact {
        let db_path = default_state_db_path();
        if !db_path.exists() {
            println!("No state database found. Run `catalogy scan` first.");
        } else {
            let db = catalogy_queue::StateDb::open(&db_path)?;
            let sets = catalogy_dedup::find_exact_duplicates(&db)?;
            print!("{}", catalogy_dedup::format_exact_report(&sets));
        }
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
                eprintln!("Warning: Failed to load CLIP models: {e}");
                eprintln!("Search will be unavailable.");
                None
            }
        }
    } else {
        eprintln!("Warning: CLIP model files not found in {}", mdir.display());
        eprintln!("Search will be unavailable. Set CATALOGY_MODEL_DIR to enable search.");
        None
    };

    let db_path = default_state_db_path();
    let state = Arc::new(catalogy_server::AppState {
        catalog,
        search_engine,
        state_db_path: if db_path.exists() {
            Some(db_path)
        } else {
            None
        },
    });

    let rt = tokio::runtime::Runtime::new()?;
    rt.block_on(async {
        let app = catalogy_server::create_router(state);
        let addr = format!("0.0.0.0:{}", port);
        let listener = tokio::net::TcpListener::bind(&addr).await?;
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

        // Verify the model file exists
        if !Path::new(mpath).exists() {
            return Err(format!("Model file not found: {}", mpath).into());
        }

        db.register_model(mid, model_version, mpath, dimensions)?;
        println!("Registered model '{mid}' (version={model_version}, dimensions={dimensions})");
        println!("  Path: {mpath}");
        println!("Use --activate --model-id {mid} to set as current and create re-embed jobs.");
        return Ok(());
    }

    if activate {
        let mid = model_id.ok_or("--model-id is required for --activate")?;

        let model = db.get_model(mid)?.ok_or(format!("Model '{}' not found. Register it first with --register.", mid))?;

        db.set_current_model(mid)?;
        println!("Set '{mid}' as current model.");

        let job_count = db.enqueue_reembed(mid, &model.model_version)?;
        println!("Created {job_count} re-embed jobs.");

        if job_count > 0 {
            println!("Run `catalogy ingest --stages embed` or the re-embed worker to process them.");
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
        println!("Rebuilding ANN index ({count} rows, {num_partitions} partitions)...");
        catalog.build_index(num_partitions)?;
        println!("Index rebuilt successfully.");
        return Ok(());
    }

    // No flags: show usage
    println!("Usage:");
    println!("  catalogy reembed --register --model-id <ID> --model-path <PATH> [--dimensions <N>] [--model-version <V>]");
    println!("  catalogy reembed --activate --model-id <ID>");
    println!("  catalogy reembed --rebuild-index");
    println!();

    // List registered models
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
        Commands::Search {
            query,
            limit,
            media_type,
            after,
        } => {
            if let Err(e) = run_search(&query, limit, media_type.as_deref(), after.as_deref()) {
                eprintln!("Error: {e}");
                std::process::exit(1);
            }
        }
        Commands::Dedup { tier, threshold } => {
            if let Err(e) = run_dedup(&tier, threshold) {
                eprintln!("Error: {e}");
                std::process::exit(1);
            }
        }
        Commands::Reembed {
            register,
            activate,
            model_id,
            model_path,
            model_version,
            dimensions,
            rebuild_index,
        } => {
            if let Err(e) = run_reembed(
                register,
                activate,
                model_id.as_deref(),
                model_path.as_deref(),
                &model_version,
                dimensions,
                rebuild_index,
            ) {
                eprintln!("Error: {e}");
                std::process::exit(1);
            }
        }
        Commands::Serve { port } => {
            if let Err(e) = run_serve(port) {
                eprintln!("Error: {e}");
                std::process::exit(1);
            }
        }
        Commands::Config => {
            println!("config: not yet implemented");
        }
    }
}
