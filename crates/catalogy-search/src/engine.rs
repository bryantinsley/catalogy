use std::path::PathBuf;
use std::sync::Arc;

use catalogy_catalog::{Catalog, CatalogRecord};
use catalogy_core::{
    CatalogyError, FrameInfo, MediaMetadata, MediaType, Result, SearchQuery, SearchResult,
};
use catalogy_embed::EmbedSession;

/// Hybrid search engine combining vector similarity with scalar filters.
pub struct SearchEngine {
    embed_session: Arc<EmbedSession>,
    catalog: Arc<Catalog>,
}

impl SearchEngine {
    pub fn new(embed_session: Arc<EmbedSession>, catalog: Arc<Catalog>) -> Self {
        Self {
            embed_session,
            catalog,
        }
    }

    /// Execute a search query and return ranked results.
    pub fn search(&self, query: &SearchQuery) -> Result<Vec<SearchResult>> {
        if query.text.is_empty() && query.filters.media_type.is_none() {
            return Ok(Vec::new());
        }

        // Step 1: Encode text query via CLIP text encoder
        let query_vector = if query.text.is_empty() {
            // No text query — use a zero vector (will rely on filters only)
            return Err(CatalogyError::Embedding(
                "Search requires a text query".to_string(),
            ));
        } else {
            self.embed_session.embed_text(&query.text)?
        };

        // Step 2: Vector search in catalog (fetch extra to allow for post-filtering)
        let fetch_limit = query.limit * 4;
        let raw_results = self.catalog.search_vector(&query_vector, fetch_limit)?;

        // Step 3: Post-filter and map to SearchResult
        let mut results: Vec<SearchResult> = raw_results
            .into_iter()
            .filter_map(|(record, distance)| {
                // Convert distance to similarity score (LanceDB returns L2 distance)
                let score = 1.0 / (1.0 + distance);

                // Apply media_type filter
                if let Some(ref filter_type) = query.filters.media_type {
                    let record_type = parse_media_type(&record.media_type);
                    if record_type != *filter_type {
                        return None;
                    }
                }

                // Apply date filters (using file_modified or exif_date_taken)
                let record_date = record
                    .exif_date_taken
                    .as_deref()
                    .or(record.file_modified.as_deref())
                    .and_then(|d| {
                        chrono::NaiveDateTime::parse_from_str(d, "%Y-%m-%dT%H:%M:%S%.fZ")
                            .or_else(|_| {
                                chrono::NaiveDateTime::parse_from_str(d, "%Y-%m-%dT%H:%M:%S")
                            })
                            .or_else(|_| {
                                chrono::NaiveDateTime::parse_from_str(d, "%Y-%m-%d %H:%M:%S")
                            })
                            .ok()
                    });

                if let Some(after) = &query.filters.after {
                    if let Some(date) = &record_date {
                        if date < after {
                            return None;
                        }
                    }
                }

                if let Some(before) = &query.filters.before {
                    if let Some(date) = &record_date {
                        if date > before {
                            return None;
                        }
                    }
                }

                Some(record_to_search_result(record, score))
            })
            .take(query.limit)
            .collect();

        // Sort by score descending
        results.sort_by(|a, b| {
            b.score
                .partial_cmp(&a.score)
                .unwrap_or(std::cmp::Ordering::Equal)
        });

        Ok(results)
    }
}

fn parse_media_type(s: &str) -> MediaType {
    match s.to_lowercase().as_str() {
        "image" => MediaType::Image,
        "video" => MediaType::Video,
        "video_frame" | "videoframe" => MediaType::VideoFrame,
        _ => MediaType::Image,
    }
}

fn record_to_search_result(record: CatalogRecord, score: f32) -> SearchResult {
    let media_type = parse_media_type(&record.media_type);

    let metadata = MediaMetadata {
        width: record.width.map(|w| w as u32),
        height: record.height.map(|h| h as u32),
        duration_ms: record.duration_ms.map(|d| d as u64),
        fps: record.fps,
        codec: record.codec,
        bitrate_kbps: record.bitrate_kbps.map(|b| b as u32),
        exif: None,
    };

    let frame_info = record.source_video_path.as_ref().map(|svp| FrameInfo {
        source_video: PathBuf::from(svp),
        frame_index: record.frame_index.unwrap_or(0) as u32,
        timestamp_ms: record.frame_timestamp_ms.unwrap_or(0) as u64,
    });

    let id = uuid::Uuid::parse_str(&record.id).unwrap_or_else(|_| uuid::Uuid::now_v7());

    SearchResult {
        id,
        score,
        file_path: PathBuf::from(&record.file_path),
        file_name: record.file_name,
        media_type,
        metadata,
        frame_info,
    }
}
