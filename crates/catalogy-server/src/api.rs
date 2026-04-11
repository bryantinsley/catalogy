use axum::{
    body::Body,
    extract::{Path, Query, State},
    http::{header, StatusCode},
    response::{Json, Response},
};
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use tokio::fs::File;
use tokio_util::io::ReaderStream;

use catalogy_core::MediaType;
use catalogy_search::parse_query;

use crate::app::AppState;

// ── Existing endpoint types ────────────────────────────────────

#[derive(Deserialize)]
pub struct SearchRequest {
    pub query: String,
    #[serde(default = "default_limit")]
    pub limit: usize,
}

fn default_limit() -> usize {
    20
}

#[derive(Serialize)]
pub struct SearchResultItem {
    pub id: String,
    pub score: f32,
    pub file_path: String,
    pub file_name: String,
    pub media_type: String,
    pub width: Option<u32>,
    pub height: Option<u32>,
    pub duration_ms: Option<u64>,
}

#[derive(Serialize)]
pub struct StatsResponse {
    pub total_items: u64,
}

// ── New endpoint types ─────────────────────────────────────────

#[derive(Serialize)]
pub struct SetupStatusResponse {
    pub ffmpeg: bool,
    pub ffprobe: bool,
    pub models: bool,
    pub database: bool,
    pub catalog: CatalogStatus,
}

#[derive(Serialize)]
pub struct CatalogStatus {
    pub exists: bool,
    pub count: u64,
}

#[derive(Serialize)]
pub struct FullStatsResponse {
    pub total_items: u64,
    pub files_tracked: u64,
    pub queue: QueueInfo,
    pub last_scan: Option<String>,
    pub models: Vec<ModelInfo>,
}

#[derive(Serialize)]
pub struct QueueInfo {
    pub pending: u64,
    pub running: u64,
    pub completed: u64,
    pub failed: u64,
    pub skipped: u64,
    pub by_stage: Vec<StageInfo>,
}

#[derive(Serialize)]
pub struct StageInfo {
    pub stage: String,
    pub pending: u64,
    pub running: u64,
    pub completed: u64,
    pub failed: u64,
    pub skipped: u64,
}

#[derive(Serialize)]
pub struct ModelInfo {
    pub model_id: String,
    pub version: String,
    pub dimensions: u32,
    pub is_current: bool,
}

#[derive(Deserialize)]
pub struct FilesQuery {
    #[serde(default = "default_page")]
    pub page: u64,
    #[serde(default = "default_per_page")]
    pub per_page: u64,
    #[serde(rename = "type", default)]
    pub media_type: Option<String>,
    #[serde(default = "default_sort")]
    pub sort: String,
}

fn default_page() -> u64 {
    1
}
fn default_per_page() -> u64 {
    50
}
fn default_sort() -> String {
    "name".to_string()
}

#[derive(Serialize)]
pub struct FilesResponse {
    pub files: Vec<FileInfo>,
    pub total: u64,
    pub page: u64,
    pub per_page: u64,
}

#[derive(Serialize)]
pub struct FileInfo {
    pub hash: String,
    pub path: String,
    pub name: String,
    pub size: u64,
    pub media_type: String,
    pub created: String,
    pub modified: String,
    pub has_thumbnail: bool,
}

#[derive(Deserialize)]
pub struct ScanRequest {
    pub path: String,
}

#[derive(Deserialize)]
pub struct IngestRequest {
    pub stages: Option<String>,
}

#[derive(Serialize)]
pub struct ActionResponse {
    pub ok: bool,
    pub message: String,
}

#[derive(Serialize)]
pub struct ProgressEvent {
    #[serde(rename = "type")]
    pub op_type: String,
    pub stage: Option<String>,
    pub processed: u64,
    pub total: u64,
    pub message: String,
}

#[derive(Deserialize)]
pub struct BrowseQuery {
    #[serde(default = "default_page")]
    pub page: u64,
    #[serde(default = "default_per_page")]
    pub per_page: u64,
    #[serde(rename = "type", default = "default_browse_type")]
    pub media_type: String,
    #[serde(default = "default_browse_sort")]
    pub sort: String,
}

fn default_browse_type() -> String {
    "all".to_string()
}
fn default_browse_sort() -> String {
    "date".to_string()
}

#[derive(Serialize)]
pub struct BrowseResponse {
    pub items: Vec<BrowseItem>,
    pub total: u64,
    pub page: u64,
}

#[derive(Serialize)]
pub struct BrowseItem {
    pub id: String,
    pub file_path: String,
    pub file_name: String,
    pub media_type: String,
    pub width: Option<u32>,
    pub height: Option<u32>,
    pub duration_ms: Option<u64>,
    pub score: Option<f32>,
}

#[derive(Serialize)]
pub struct DedupResponse {
    pub tier: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub exact: Option<Vec<catalogy_dedup::DuplicateSet>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub visual: Option<Vec<catalogy_dedup::VisualDuplicateCluster>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cross_video: Option<Vec<catalogy_dedup::CrossVideoDuplicate>>,
}

#[derive(Deserialize)]
pub struct DedupQuery {
    #[serde(default = "default_tier")]
    pub tier: String,
    #[serde(default = "default_threshold")]
    pub threshold: f32,
}

fn default_tier() -> String {
    "all".to_string()
}
fn default_threshold() -> f32 {
    0.92
}

// ── Existing handlers ──────────────────────────────────────────

pub async fn search_handler(
    State(state): State<Arc<AppState>>,
    Json(req): Json<SearchRequest>,
) -> Result<Json<Vec<SearchResultItem>>, (StatusCode, String)> {
    let engine = state.search_engine.as_ref().ok_or_else(|| {
        (
            StatusCode::SERVICE_UNAVAILABLE,
            "Search engine not initialized (CLIP models not loaded)".to_string(),
        )
    })?;

    let query = parse_query(&req.query, req.limit);

    let results = engine.search(&query).map_err(|e| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("Search failed: {}", e),
        )
    })?;

    let items: Vec<SearchResultItem> = results
        .into_iter()
        .map(|r| SearchResultItem {
            id: r.id.to_string(),
            score: r.score,
            file_path: r.file_path.to_string_lossy().to_string(),
            file_name: r.file_name,
            media_type: match r.media_type {
                MediaType::Image => "image".to_string(),
                MediaType::Video => "video".to_string(),
                MediaType::VideoFrame => "video_frame".to_string(),
            },
            width: r.metadata.width,
            height: r.metadata.height,
            duration_ms: r.metadata.duration_ms,
        })
        .collect();

    Ok(Json(items))
}

pub async fn media_handler(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> Result<Response, (StatusCode, String)> {
    let record = state
        .catalog
        .get_by_id(&id)
        .map_err(|e| {
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("Database error: {}", e),
            )
        })?
        .ok_or_else(|| (StatusCode::NOT_FOUND, "Media not found".to_string()))?;

    let path = std::path::Path::new(&record.file_path);
    if !path.exists() {
        return Err((StatusCode::NOT_FOUND, "File not found on disk".to_string()));
    }

    let mime = mime_guess::from_path(path)
        .first_or_octet_stream()
        .to_string();

    let file = File::open(path).await.map_err(|e| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("Failed to open file: {}", e),
        )
    })?;

    let stream = ReaderStream::new(file);
    let body = Body::from_stream(stream);

    Ok(Response::builder()
        .header(header::CONTENT_TYPE, mime)
        .body(body)
        .unwrap())
}

pub async fn thumb_handler(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> Result<Response, (StatusCode, String)> {
    media_handler(State(state), Path(id)).await
}

pub async fn stats_handler(
    State(state): State<Arc<AppState>>,
) -> Result<Json<StatsResponse>, (StatusCode, String)> {
    let total = state.catalog.count().map_err(|e| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("Stats error: {}", e),
        )
    })?;

    Ok(Json(StatsResponse {
        total_items: total,
    }))
}

pub async fn dedup_handler(
    State(state): State<Arc<AppState>>,
    Query(params): Query<DedupQuery>,
) -> Result<Json<DedupResponse>, (StatusCode, String)> {
    let tier = params.tier.as_str();
    let threshold = params.threshold;

    let run_exact = tier == "all" || tier == "exact";
    let run_visual = tier == "all" || tier == "visual";
    let run_cross = tier == "all" || tier == "cross-video";

    if !run_exact && !run_visual && !run_cross {
        return Err((
            StatusCode::BAD_REQUEST,
            format!(
                "Unknown tier: {}. Use: exact, visual, cross-video, or all",
                tier
            ),
        ));
    }

    let exact = if run_exact {
        if let Some(ref db_path) = state.state_db_path {
            let db = catalogy_queue::StateDb::open(db_path).map_err(|e| {
                (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    format!("Database error: {}", e),
                )
            })?;
            Some(catalogy_dedup::find_exact_duplicates(&db).map_err(|e| {
                (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    format!("Exact dedup error: {}", e),
                )
            })?)
        } else {
            Some(Vec::new())
        }
    } else {
        None
    };

    let visual = if run_visual {
        Some(
            catalogy_dedup::find_visual_duplicates(&state.catalog, threshold).map_err(|e| {
                (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    format!("Visual dedup error: {}", e),
                )
            })?,
        )
    } else {
        None
    };

    let cross_video = if run_cross {
        Some(
            catalogy_dedup::find_cross_video_duplicates(&state.catalog, threshold).map_err(
                |e| {
                    (
                        StatusCode::INTERNAL_SERVER_ERROR,
                        format!("Cross-video dedup error: {}", e),
                    )
                },
            )?,
        )
    } else {
        None
    };

    Ok(Json(DedupResponse {
        tier: params.tier,
        exact,
        visual,
        cross_video,
    }))
}

// ── New handlers ───────────────────────────────────────────────

pub async fn setup_status_handler(
    State(state): State<Arc<AppState>>,
) -> Result<Json<SetupStatusResponse>, (StatusCode, String)> {
    let ffmpeg = check_command("ffmpeg");
    let ffprobe = check_command("ffprobe");

    let models = state.model_dir.join("visual.onnx").exists()
        && state.model_dir.join("text.onnx").exists()
        && state.model_dir.join("tokenizer.json").exists();

    let database = state.state_db_path.is_some();

    let count = state.catalog.count().unwrap_or(0);
    let catalog = CatalogStatus {
        exists: count > 0,
        count,
    };

    Ok(Json(SetupStatusResponse {
        ffmpeg,
        ffprobe,
        models,
        database,
        catalog,
    }))
}

fn check_command(cmd: &str) -> bool {
    std::process::Command::new(cmd)
        .arg("-version")
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

pub async fn stats_full_handler(
    State(state): State<Arc<AppState>>,
) -> Result<Json<FullStatsResponse>, (StatusCode, String)> {
    let total_items = state.catalog.count().unwrap_or(0);

    let (files_tracked, queue, last_scan, models) = if let Some(ref db_path) = state.state_db_path
    {
        let db = catalogy_queue::StateDb::open(db_path).map_err(|e| {
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("Database error: {}", e),
            )
        })?;

        let fc = db.file_count().unwrap_or(0);
        let qs = db.stats().unwrap_or_default();
        let ls = db.get_config("last_scan_time").unwrap_or(None);
        let model_records = db.list_models().unwrap_or_default();

        let queue = QueueInfo {
            pending: qs.pending,
            running: qs.running,
            completed: qs.completed,
            failed: qs.failed,
            skipped: qs.skipped,
            by_stage: qs
                .by_stage
                .into_iter()
                .map(|(stage, p, r, c, f, s)| StageInfo {
                    stage,
                    pending: p,
                    running: r,
                    completed: c,
                    failed: f,
                    skipped: s,
                })
                .collect(),
        };

        let models: Vec<ModelInfo> = model_records
            .into_iter()
            .map(|m| ModelInfo {
                model_id: m.model_id,
                version: m.model_version,
                dimensions: m.dimensions,
                is_current: m.is_current,
            })
            .collect();

        (fc, queue, ls, models)
    } else {
        (
            0,
            QueueInfo {
                pending: 0,
                running: 0,
                completed: 0,
                failed: 0,
                skipped: 0,
                by_stage: vec![],
            },
            None,
            vec![],
        )
    };

    Ok(Json(FullStatsResponse {
        total_items,
        files_tracked,
        queue,
        last_scan,
        models,
    }))
}

pub async fn files_handler(
    State(state): State<Arc<AppState>>,
    Query(params): Query<FilesQuery>,
) -> Result<Json<FilesResponse>, (StatusCode, String)> {
    let db_path = state.state_db_path.as_ref().ok_or_else(|| {
        (
            StatusCode::SERVICE_UNAVAILABLE,
            "State database not available".to_string(),
        )
    })?;

    let db = catalogy_queue::StateDb::open(db_path).map_err(|e| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("Database error: {}", e),
        )
    })?;

    let all_files = db.get_all_active_files().map_err(|e| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("Query error: {}", e),
        )
    })?;

    // Filter by media type
    let filtered: Vec<_> = all_files
        .into_iter()
        .filter(|f| {
            if let Some(ref mt) = params.media_type {
                if mt == "all" {
                    return true;
                }
                let ext = std::path::Path::new(&f.file_path)
                    .extension()
                    .and_then(|e| e.to_str())
                    .unwrap_or("")
                    .to_lowercase();
                match mt.as_str() {
                    "image" => matches!(
                        ext.as_str(),
                        "jpg" | "jpeg" | "png" | "gif" | "bmp" | "tiff" | "tif" | "webp"
                            | "heic" | "heif" | "avif"
                    ),
                    "video" => matches!(
                        ext.as_str(),
                        "mp4" | "mov" | "avi" | "mkv" | "wmv" | "flv" | "webm" | "m4v" | "mpg"
                            | "mpeg"
                    ),
                    _ => true,
                }
            } else {
                true
            }
        })
        .collect();

    let total = filtered.len() as u64;
    let page = params.page.max(1);
    let per_page = params.per_page.min(200).max(1);
    let skip = ((page - 1) * per_page) as usize;

    let mut sorted = filtered;
    match params.sort.as_str() {
        "size" => sorted.sort_by(|a, b| b.file_size.cmp(&a.file_size)),
        "date" => sorted.sort_by(|a, b| b.file_modified.cmp(&a.file_modified)),
        _ => sorted.sort_by(|a, b| {
            let name_a = std::path::Path::new(&a.file_path)
                .file_name()
                .map(|n| n.to_string_lossy().to_string())
                .unwrap_or_default();
            let name_b = std::path::Path::new(&b.file_path)
                .file_name()
                .map(|n| n.to_string_lossy().to_string())
                .unwrap_or_default();
            name_a.to_lowercase().cmp(&name_b.to_lowercase())
        }),
    }

    let page_files: Vec<FileInfo> = sorted
        .into_iter()
        .skip(skip)
        .take(per_page as usize)
        .map(|f| {
            let name = std::path::Path::new(&f.file_path)
                .file_name()
                .map(|n| n.to_string_lossy().to_string())
                .unwrap_or_default();
            let ext = std::path::Path::new(&f.file_path)
                .extension()
                .and_then(|e| e.to_str())
                .unwrap_or("")
                .to_lowercase();
            let media_type = if matches!(
                ext.as_str(),
                "jpg" | "jpeg" | "png" | "gif" | "bmp" | "tiff" | "tif" | "webp" | "heic"
                    | "heif" | "avif"
            ) {
                "image"
            } else if matches!(
                ext.as_str(),
                "mp4" | "mov" | "avi" | "mkv" | "wmv" | "flv" | "webm" | "m4v" | "mpg" | "mpeg"
            ) {
                "video"
            } else {
                "unknown"
            };

            FileInfo {
                hash: f.file_hash,
                path: f.file_path,
                name,
                size: f.file_size as u64,
                media_type: media_type.to_string(),
                created: f.first_seen,
                modified: f.file_modified,
                has_thumbnail: false,
            }
        })
        .collect();

    Ok(Json(FilesResponse {
        files: page_files,
        total,
        page,
        per_page,
    }))
}

pub async fn scan_handler(
    State(state): State<Arc<AppState>>,
    Json(req): Json<ScanRequest>,
) -> Result<Json<ActionResponse>, (StatusCode, String)> {
    let scan_path = req.path.clone();
    let data_dir = state.data_dir.clone();

    {
        let mut progress = state.progress.lock().unwrap();
        progress.op_type = "scan".to_string();
        progress.stage = None;
        progress.processed = 0;
        progress.total = 0;
        progress.message = format!("Scanning {}", scan_path);
    }

    let progress_ref = Arc::clone(&state);

    tokio::task::spawn_blocking(move || {
        let db_path = data_dir.join("state.db");
        let db = match catalogy_queue::StateDb::open(&db_path) {
            Ok(db) => db,
            Err(e) => {
                let mut progress = progress_ref.progress.lock().unwrap();
                progress.op_type = "idle".to_string();
                progress.message = format!("Scan failed: {}", e);
                return;
            }
        };

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

        let root = std::path::Path::new(&scan_path);
        match catalogy_scanner::scan_directory(root, &image_exts, &video_exts) {
            Ok(scanned) => {
                {
                    let mut progress = progress_ref.progress.lock().unwrap();
                    progress.total = scanned.len() as u64;
                    progress.message =
                        format!("Found {} files, detecting changes...", scanned.len());
                }

                let changes = catalogy_queue::detect_changes(&db, &scanned);
                if let Ok(changes) = changes {
                    let _ = catalogy_queue::apply_changes_and_enqueue(&db, &changes);
                    let _ = db.set_config("last_scan_time", &chrono::Utc::now().to_rfc3339());
                }

                let mut progress = progress_ref.progress.lock().unwrap();
                progress.op_type = "idle".to_string();
                progress.message = "Scan complete".to_string();
            }
            Err(e) => {
                let mut progress = progress_ref.progress.lock().unwrap();
                progress.op_type = "idle".to_string();
                progress.message = format!("Scan failed: {}", e);
            }
        }
    });

    Ok(Json(ActionResponse {
        ok: true,
        message: "Scan started".to_string(),
    }))
}

pub async fn ingest_handler(
    State(state): State<Arc<AppState>>,
    Json(req): Json<IngestRequest>,
) -> Result<Json<ActionResponse>, (StatusCode, String)> {
    let data_dir = state.data_dir.clone();
    let model_dir = state.model_dir.clone();
    let stages = req.stages.clone();

    {
        let mut progress = state.progress.lock().unwrap();
        progress.op_type = "ingest".to_string();
        progress.stage = None;
        progress.processed = 0;
        progress.total = 0;
        progress.message = "Starting ingest...".to_string();
    }

    let progress_ref = Arc::clone(&state);

    tokio::task::spawn_blocking(move || {
        let db_path = data_dir.join("state.db");
        let db = match catalogy_queue::StateDb::open(&db_path) {
            Ok(db) => db,
            Err(e) => {
                let mut progress = progress_ref.progress.lock().unwrap();
                progress.op_type = "idle".to_string();
                progress.message = format!("Ingest failed: {}", e);
                return;
            }
        };

        let _ = db.reset_running_to_pending();

        let should_run = |stage_name: &str| -> bool {
            match &stages {
                None => true,
                Some(s) => s.split(',').any(|s| s.trim() == stage_name),
            }
        };

        if should_run("metadata") || should_run("extract_metadata") {
            {
                let mut progress = progress_ref.progress.lock().unwrap();
                progress.stage = Some("metadata".to_string());
                progress.message = "Extracting metadata...".to_string();
            }
            let ffprobe = catalogy_metadata::find_ffprobe(None);
            let _ = catalogy_metadata::run_metadata_worker(&db, ffprobe.as_deref(), true);
        }

        if should_run("frames") || should_run("extract_frames") {
            {
                let mut progress = progress_ref.progress.lock().unwrap();
                progress.stage = Some("frames".to_string());
                progress.message = "Extracting frames...".to_string();
            }
            let config = catalogy_core::ExtractionConfig {
                frame_strategy: "adaptive".to_string(),
                scene_threshold: 0.3,
                max_interval_seconds: 60,
                frame_interval_seconds: 30,
                frame_max_dimension: 512,
                dedup_similarity_threshold: 0.95,
                ffprobe_path: None,
                thumbnail_dir: "~/.local/share/catalogy/thumbs".to_string(),
            };
            let _ = catalogy_extract::run_extract_frames_worker(&db, &config, "worker-web");
        }

        if should_run("embed") {
            {
                let mut progress = progress_ref.progress.lock().unwrap();
                progress.stage = Some("embed".to_string());
                progress.message = "Embedding...".to_string();
            }
            let visual_model = model_dir.join("visual.onnx");
            let text_model = model_dir.join("text.onnx");
            let tokenizer = model_dir.join("tokenizer.json");

            if visual_model.exists() && text_model.exists() && tokenizer.exists() {
                if let Ok(session) =
                    catalogy_embed::EmbedSession::new(&visual_model, &text_model, &tokenizer)
                {
                    let catalog_path = data_dir.join("catalog.lance");
                    if let Ok(catalog) =
                        catalogy_catalog::Catalog::open(&catalog_path.to_string_lossy())
                    {
                        let _ = catalogy_embed::run_embed_worker(
                            &db,
                            &session,
                            &catalog,
                            "clip-vit-h-14",
                            "1",
                            "worker-web",
                        );
                    }
                }
            }
        }

        let mut progress = progress_ref.progress.lock().unwrap();
        progress.op_type = "idle".to_string();
        progress.stage = None;
        progress.message = "Ingest complete".to_string();
    });

    Ok(Json(ActionResponse {
        ok: true,
        message: "Ingest started".to_string(),
    }))
}

pub async fn progress_handler(State(state): State<Arc<AppState>>) -> Response {
    let state_clone = Arc::clone(&state);

    let stream = async_stream::stream! {
        loop {
            let event = {
                let progress = state_clone.progress.lock().unwrap();
                ProgressEvent {
                    op_type: if progress.op_type.is_empty() { "idle".to_string() } else { progress.op_type.clone() },
                    stage: progress.stage.clone(),
                    processed: progress.processed,
                    total: progress.total,
                    message: progress.message.clone(),
                }
            };

            // Also fetch live queue stats if db is available
            let (processed, total) = if let Some(ref db_path) = state_clone.state_db_path {
                if let Ok(db) = catalogy_queue::StateDb::open(db_path) {
                    let stats = db.stats().unwrap_or_default();
                    let done = stats.completed + stats.skipped + stats.failed;
                    (done, done + stats.pending + stats.running)
                } else {
                    (event.processed, event.total)
                }
            } else {
                (event.processed, event.total)
            };

            let final_event = ProgressEvent {
                op_type: event.op_type,
                stage: event.stage,
                processed,
                total,
                message: event.message,
            };

            let json = serde_json::to_string(&final_event).unwrap_or_default();
            let sse = format!("data: {}\n\n", json);
            yield Ok::<_, std::convert::Infallible>(sse);

            tokio::time::sleep(std::time::Duration::from_secs(2)).await;
        }
    };

    Response::builder()
        .header(header::CONTENT_TYPE, "text/event-stream")
        .header(header::CACHE_CONTROL, "no-cache")
        .header("X-Accel-Buffering", "no")
        .body(Body::from_stream(stream))
        .unwrap()
}

pub async fn browse_handler(
    State(state): State<Arc<AppState>>,
    Query(params): Query<BrowseQuery>,
) -> Result<Json<BrowseResponse>, (StatusCode, String)> {
    let all_records = state.catalog.list_all().map_err(|e| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("Catalog error: {}", e),
        )
    })?;

    // Filter by media type
    let filtered: Vec<_> = all_records
        .into_iter()
        .filter(|r| {
            if r.tombstone {
                return false;
            }
            if params.media_type == "all" {
                true
            } else if params.media_type == "image" {
                r.media_type == "image"
            } else if params.media_type == "video" {
                r.media_type == "video" || r.media_type == "video_frame"
            } else {
                true
            }
        })
        .collect();

    let total = filtered.len() as u64;
    let page = params.page.max(1);
    let per_page = params.per_page.min(200).max(1);
    let skip = ((page - 1) * per_page) as usize;

    let mut sorted = filtered;
    match params.sort.as_str() {
        "name" => sorted.sort_by(|a, b| {
            a.file_name
                .to_lowercase()
                .cmp(&b.file_name.to_lowercase())
        }),
        "size" => sorted.sort_by(|a, b| b.file_size.cmp(&a.file_size)),
        _ => sorted.sort_by(|a, b| b.indexed_at.cmp(&a.indexed_at)),
    }

    let items: Vec<BrowseItem> = sorted
        .into_iter()
        .skip(skip)
        .take(per_page as usize)
        .map(|r| BrowseItem {
            id: r.id,
            file_path: r.file_path,
            file_name: r.file_name,
            media_type: r.media_type,
            width: r.width.map(|w| w as u32),
            height: r.height.map(|h| h as u32),
            duration_ms: r.duration_ms.map(|d| d as u64),
            score: None,
        })
        .collect();

    Ok(Json(BrowseResponse {
        items,
        total,
        page,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_search_request_deserialize() {
        let json = r#"{"query": "sunset", "limit": 10}"#;
        let req: SearchRequest = serde_json::from_str(json).unwrap();
        assert_eq!(req.query, "sunset");
        assert_eq!(req.limit, 10);
    }

    #[test]
    fn test_search_request_default_limit() {
        let json = r#"{"query": "ocean"}"#;
        let req: SearchRequest = serde_json::from_str(json).unwrap();
        assert_eq!(req.limit, 20);
    }

    #[test]
    fn test_stats_response_serialize() {
        let stats = StatsResponse { total_items: 42 };
        let json = serde_json::to_string(&stats).unwrap();
        assert!(json.contains("42"));
    }

    #[test]
    fn test_search_result_item_serialize() {
        let item = SearchResultItem {
            id: "abc".to_string(),
            score: 0.95,
            file_path: "/photos/sunset.jpg".to_string(),
            file_name: "sunset.jpg".to_string(),
            media_type: "image".to_string(),
            width: Some(1920),
            height: Some(1080),
            duration_ms: None,
        };
        let json = serde_json::to_string(&item).unwrap();
        assert!(json.contains("sunset.jpg"));
        assert!(json.contains("0.95"));
    }

    #[test]
    fn test_setup_status_response_serialize() {
        let resp = SetupStatusResponse {
            ffmpeg: true,
            ffprobe: true,
            models: false,
            database: true,
            catalog: CatalogStatus {
                exists: true,
                count: 100,
            },
        };
        let json = serde_json::to_string(&resp).unwrap();
        let v: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert_eq!(v["ffmpeg"], true);
        assert_eq!(v["ffprobe"], true);
        assert_eq!(v["models"], false);
        assert_eq!(v["database"], true);
        assert_eq!(v["catalog"]["exists"], true);
        assert_eq!(v["catalog"]["count"], 100);
    }

    #[test]
    fn test_full_stats_response_serialize() {
        let resp = FullStatsResponse {
            total_items: 500,
            files_tracked: 200,
            queue: QueueInfo {
                pending: 10,
                running: 2,
                completed: 180,
                failed: 5,
                skipped: 3,
                by_stage: vec![StageInfo {
                    stage: "embed".to_string(),
                    pending: 10,
                    running: 2,
                    completed: 180,
                    failed: 5,
                    skipped: 3,
                }],
            },
            last_scan: Some("2024-01-15T10:30:00Z".to_string()),
            models: vec![ModelInfo {
                model_id: "clip-vit-h-14".to_string(),
                version: "1".to_string(),
                dimensions: 1024,
                is_current: true,
            }],
        };
        let json = serde_json::to_string(&resp).unwrap();
        let v: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert_eq!(v["total_items"], 500);
        assert_eq!(v["files_tracked"], 200);
        assert_eq!(v["queue"]["pending"], 10);
        assert_eq!(v["queue"]["by_stage"][0]["stage"], "embed");
        assert_eq!(v["last_scan"], "2024-01-15T10:30:00Z");
        assert_eq!(v["models"][0]["model_id"], "clip-vit-h-14");
    }

    #[test]
    fn test_files_query_defaults() {
        let json = r#"{}"#;
        let query: FilesQuery = serde_json::from_str(json).unwrap();
        assert_eq!(query.page, 1);
        assert_eq!(query.per_page, 50);
        assert_eq!(query.sort, "name");
        assert!(query.media_type.is_none());
    }

    #[test]
    fn test_browse_query_defaults() {
        let json = r#"{}"#;
        let query: BrowseQuery = serde_json::from_str(json).unwrap();
        assert_eq!(query.page, 1);
        assert_eq!(query.per_page, 50);
        assert_eq!(query.media_type, "all");
        assert_eq!(query.sort, "date");
    }

    #[test]
    fn test_files_response_serialize() {
        let resp = FilesResponse {
            files: vec![FileInfo {
                hash: "abc123".to_string(),
                path: "/photos/test.jpg".to_string(),
                name: "test.jpg".to_string(),
                size: 1024,
                media_type: "image".to_string(),
                created: "2024-01-01".to_string(),
                modified: "2024-01-01".to_string(),
                has_thumbnail: false,
            }],
            total: 1,
            page: 1,
            per_page: 50,
        };
        let json = serde_json::to_string(&resp).unwrap();
        let v: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert_eq!(v["total"], 1);
        assert_eq!(v["files"][0]["hash"], "abc123");
        assert_eq!(v["files"][0]["name"], "test.jpg");
    }

    #[test]
    fn test_browse_response_serialize() {
        let resp = BrowseResponse {
            items: vec![BrowseItem {
                id: "item-1".to_string(),
                file_path: "/photos/sunset.jpg".to_string(),
                file_name: "sunset.jpg".to_string(),
                media_type: "image".to_string(),
                width: Some(1920),
                height: Some(1080),
                duration_ms: None,
                score: None,
            }],
            total: 1,
            page: 1,
        };
        let json = serde_json::to_string(&resp).unwrap();
        let v: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert_eq!(v["total"], 1);
        assert_eq!(v["items"][0]["id"], "item-1");
        assert_eq!(v["items"][0]["width"], 1920);
        assert!(v["items"][0]["score"].is_null());
    }

    #[test]
    fn test_action_response_serialize() {
        let resp = ActionResponse {
            ok: true,
            message: "Scan started".to_string(),
        };
        let json = serde_json::to_string(&resp).unwrap();
        let v: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert_eq!(v["ok"], true);
        assert_eq!(v["message"], "Scan started");
    }

    #[test]
    fn test_progress_event_serialize() {
        let evt = ProgressEvent {
            op_type: "ingest".to_string(),
            stage: Some("embed".to_string()),
            processed: 50,
            total: 100,
            message: "Embedding files...".to_string(),
        };
        let json = serde_json::to_string(&evt).unwrap();
        let v: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert_eq!(v["type"], "ingest");
        assert_eq!(v["stage"], "embed");
        assert_eq!(v["processed"], 50);
        assert_eq!(v["total"], 100);
    }

    #[test]
    fn test_scan_request_deserialize() {
        let json = r#"{"path": "/home/user/photos"}"#;
        let req: ScanRequest = serde_json::from_str(json).unwrap();
        assert_eq!(req.path, "/home/user/photos");
    }

    #[test]
    fn test_ingest_request_deserialize() {
        let json = r#"{"stages": "metadata,embed"}"#;
        let req: IngestRequest = serde_json::from_str(json).unwrap();
        assert_eq!(req.stages, Some("metadata,embed".to_string()));

        let json2 = r#"{"stages": null}"#;
        let req2: IngestRequest = serde_json::from_str(json2).unwrap();
        assert!(req2.stages.is_none());
    }

    #[test]
    fn test_check_command_does_not_panic() {
        // Should not panic regardless of whether the command exists
        let _ = check_command("echo");
        let _ = check_command("nonexistent_command_12345");
    }
}
