use std::path::PathBuf;
use std::sync::Arc;

use catalogy_catalog::{Catalog, CatalogRecord};
use catalogy_core::{
    CatalogyError, FrameInfo, MediaMetadata, MediaType, Result, SearchQuery, SearchResult,
};
use catalogy_embed::EmbedSession;

/// Hybrid search engine combining vector similarity with scalar filters.
#[derive(Clone)]
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

    /// Returns a clone of the shared embed session.
    pub fn embed_session(&self) -> Arc<EmbedSession> {
        Arc::clone(&self.embed_session)
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

        // Step 2: Vector search in catalog. Fetch generously: a single video can
        // occupy many of the top rows (its aggregated row + each kept frame), and
        // they all collapse to one result below, so we need headroom to still
        // return `limit` distinct media items.
        let fetch_limit = (query.limit * 8).max(80);
        let raw_results = self.catalog.search_vector(&query_vector, fetch_limit)?;

        // Step 3: Post-filter and map to SearchResult (one row per catalog record;
        // collapsed to one per media item in step 4).
        let mut results: Vec<SearchResult> = raw_results
            .into_iter()
            .filter_map(|(record, distance)| {
                // search_vector uses cosine distance, so _distance = 1 - cosine
                // similarity. Report the cosine similarity directly.
                let score = 1.0 - distance;

                // Apply media_type filter. A `video` filter matches both the
                // aggregated video row and its individual frame rows, so the
                // collapsed result keeps its best-matching frame timestamp.
                if let Some(ref filter_type) = query.filters.media_type {
                    let record_type = parse_media_type(&record.media_type);
                    if !media_type_matches(filter_type, &record_type) {
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
            .collect();

        // Step 4: Collapse to one result per media item. A video and all of its
        // frame rows become a single result whose score is the best across them
        // and whose frame_info points at the best-matching moment. Then sort by
        // score and trim to the requested limit.
        results = collapse_by_media_item(results);
        results.sort_by(|a, b| {
            b.score
                .partial_cmp(&a.score)
                .unwrap_or(std::cmp::Ordering::Equal)
        });
        results.truncate(query.limit);

        Ok(results)
    }
}

/// Whether a catalog row of `record_type` satisfies a `filter_type` request.
///
/// A `Video` filter intentionally also matches `VideoFrame` rows: frames are how
/// a video earns its rank, and they collapse into the single video result. All
/// other types match exactly. `VideoFrame` is an internal storage type, never a
/// user-facing filter, so it isn't widened in reverse.
fn media_type_matches(filter_type: &MediaType, record_type: &MediaType) -> bool {
    record_type == filter_type
        || (*filter_type == MediaType::Video && *record_type == MediaType::VideoFrame)
}

/// Collapse multiple catalog rows for the same media item into one result.
///
/// Videos produce one aggregated `video` row plus one `video_frame` row per kept
/// frame, all sharing the same `file_path`. Returning them separately floods the
/// results with near-duplicates, so we keep a single result per `file_path`: the
/// highest-scoring row as the representative, with `frame_info` set to the
/// best-scoring frame (the moment to jump to). Images have a unique `file_path`
/// and pass through unchanged.
fn collapse_by_media_item(results: Vec<SearchResult>) -> Vec<SearchResult> {
    use std::collections::HashMap;

    let mut order: Vec<String> = Vec::new();
    let mut best: HashMap<String, SearchResult> = HashMap::new();
    let mut best_frame: HashMap<String, (f32, FrameInfo)> = HashMap::new();

    for r in results {
        let key = r.file_path.to_string_lossy().to_string();

        // Track the best-scoring frame for this item (the "moment").
        if let Some(fi) = &r.frame_info {
            let is_better = best_frame.get(&key).is_none_or(|(s, _)| r.score > *s);
            if is_better {
                best_frame.insert(key.clone(), (r.score, fi.clone()));
            }
        }

        // Track the representative (highest-scoring) row for this item.
        match best.get(&key) {
            Some(existing) if existing.score >= r.score => {}
            _ => {
                if !best.contains_key(&key) {
                    order.push(key.clone());
                }
                best.insert(key, r);
            }
        }
    }

    order
        .into_iter()
        .filter_map(|k| {
            let mut rep = best.remove(&k)?;
            // If this item has any frames, present it as a single video result
            // anchored to its best-matching moment.
            if let Some((_, fi)) = best_frame.remove(&k) {
                rep.media_type = MediaType::Video;
                rep.frame_info = Some(fi);
            }
            Some(rep)
        })
        .collect()
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

#[cfg(test)]
mod tests {
    use super::*;

    fn sr(path: &str, mt: MediaType, score: f32, ts: Option<u64>) -> SearchResult {
        SearchResult {
            id: uuid::Uuid::now_v7(),
            score,
            file_path: PathBuf::from(path),
            file_name: path.rsplit('/').next().unwrap_or(path).to_string(),
            media_type: mt,
            metadata: MediaMetadata {
                width: None,
                height: None,
                duration_ms: None,
                fps: None,
                codec: None,
                bitrate_kbps: None,
                exif: None,
            },
            frame_info: ts.map(|t| FrameInfo {
                source_video: PathBuf::from(path),
                frame_index: 0,
                timestamp_ms: t,
            }),
        }
    }

    #[test]
    fn test_collapse_keeps_one_per_video_with_best_moment() {
        // A video: aggregated row + two frame rows; plus a standalone image.
        let input = vec![
            sr("/v/a.mp4", MediaType::Video, 0.20, None),
            sr("/v/a.mp4", MediaType::VideoFrame, 0.50, Some(3000)), // best frame
            sr("/v/a.mp4", MediaType::VideoFrame, 0.40, Some(9000)),
            sr("/i/b.jpg", MediaType::Image, 0.30, None),
        ];
        let out = collapse_by_media_item(input);

        assert_eq!(out.len(), 2, "one result per distinct media item");

        let vid = out
            .iter()
            .find(|r| r.file_path == PathBuf::from("/v/a.mp4"))
            .unwrap();
        // Score is the best across all rows, presented as a single video result
        // anchored to the best-matching moment.
        assert!((vid.score - 0.50).abs() < 1e-6);
        assert_eq!(vid.media_type, MediaType::Video);
        assert_eq!(vid.frame_info.as_ref().unwrap().timestamp_ms, 3000);

        // Image passes through unchanged, with no moment.
        let img = out
            .iter()
            .find(|r| r.file_path == PathBuf::from("/i/b.jpg"))
            .unwrap();
        assert_eq!(img.media_type, MediaType::Image);
        assert!(img.frame_info.is_none());
    }

    #[test]
    fn test_collapse_empty() {
        assert!(collapse_by_media_item(vec![]).is_empty());
    }

    #[test]
    fn test_media_type_matches() {
        // Exact matches.
        assert!(media_type_matches(&MediaType::Image, &MediaType::Image));
        assert!(media_type_matches(&MediaType::Video, &MediaType::Video));
        // A video filter also matches the internal frame rows...
        assert!(media_type_matches(&MediaType::Video, &MediaType::VideoFrame));
        // ...but an image filter never does, and the widening is one-way.
        assert!(!media_type_matches(&MediaType::Image, &MediaType::VideoFrame));
        assert!(!media_type_matches(&MediaType::Image, &MediaType::Video));
        assert!(!media_type_matches(&MediaType::VideoFrame, &MediaType::Video));
    }

    #[test]
    fn test_collapse_multiple_videos_independent() {
        // Two videos interleaved; each must collapse to its own best moment.
        let input = vec![
            sr("/v/a.mp4", MediaType::VideoFrame, 0.30, Some(1000)),
            sr("/v/b.mp4", MediaType::VideoFrame, 0.70, Some(2000)), // b best
            sr("/v/a.mp4", MediaType::VideoFrame, 0.55, Some(4000)), // a best
            sr("/v/b.mp4", MediaType::Video, 0.10, None),
        ];
        let out = collapse_by_media_item(input);
        assert_eq!(out.len(), 2);

        let a = out.iter().find(|r| r.file_name == "a.mp4").unwrap();
        assert!((a.score - 0.55).abs() < 1e-6);
        assert_eq!(a.frame_info.as_ref().unwrap().timestamp_ms, 4000);

        let b = out.iter().find(|r| r.file_name == "b.mp4").unwrap();
        assert!((b.score - 0.70).abs() < 1e-6);
        assert_eq!(b.frame_info.as_ref().unwrap().timestamp_ms, 2000);
    }

    #[test]
    fn test_collapse_aggregated_row_outscores_frames() {
        // The aggregated `video` row is the highest scorer, but the result must
        // still be anchored to the best *frame* moment (the aggregated row has
        // no timestamp to seek to).
        let input = vec![
            sr("/v/a.mp4", MediaType::Video, 0.90, None), // top score, no moment
            sr("/v/a.mp4", MediaType::VideoFrame, 0.40, Some(5000)),
            sr("/v/a.mp4", MediaType::VideoFrame, 0.60, Some(7000)), // best frame
        ];
        let out = collapse_by_media_item(input);
        assert_eq!(out.len(), 1);
        assert!((out[0].score - 0.90).abs() < 1e-6, "keeps best overall score");
        assert_eq!(out[0].media_type, MediaType::Video);
        assert_eq!(
            out[0].frame_info.as_ref().unwrap().timestamp_ms,
            7000,
            "anchored to best frame, not the score-less aggregated row"
        );
    }
}
