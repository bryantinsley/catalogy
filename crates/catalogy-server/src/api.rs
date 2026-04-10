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
    // For thumbnails, serve the original file for now.
    // A real implementation would serve pre-generated thumbnails.
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
            format!("Unknown tier: {}. Use: exact, visual, cross-video, or all", tier),
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
}
