use catalogy_catalog::Catalog;
use catalogy_core::Result;
use serde::Serialize;
use std::collections::{HashMap, HashSet};

/// A pair of videos that share visually similar frames.
#[derive(Clone, Debug, Serialize)]
pub struct CrossVideoDuplicate {
    pub video_a_path: String,
    pub video_b_path: String,
    pub shared_frame_count: usize,
    pub total_frames_a: usize,
    pub total_frames_b: usize,
    pub overlap_ratio: f32,
}

/// Find videos that share visually similar frames.
///
/// For each video frame, search for similar frames belonging to different videos.
/// Report pairs of videos with significant frame overlap.
pub fn find_cross_video_duplicates(
    catalog: &Catalog,
    threshold: f32,
) -> Result<Vec<CrossVideoDuplicate>> {
    let records = catalog.list_all()?;
    if records.is_empty() {
        return Ok(Vec::new());
    }

    // Collect video frames grouped by source video path
    let mut frames_by_video: HashMap<String, Vec<usize>> = HashMap::new();
    let mut frame_indices: Vec<usize> = Vec::new();

    for (i, record) in records.iter().enumerate() {
        if record.media_type == "video_frame" {
            let source = record
                .source_video_path
                .as_deref()
                .unwrap_or(&record.file_path);
            frames_by_video
                .entry(source.to_string())
                .or_default()
                .push(i);
            frame_indices.push(i);
        }
    }

    if frames_by_video.len() < 2 {
        return Ok(Vec::new());
    }

    // Build reverse map: record index -> source video path
    let mut idx_to_video: HashMap<usize, String> = HashMap::new();
    for (video, indices) in &frames_by_video {
        for &idx in indices {
            idx_to_video.insert(idx, video.clone());
        }
    }

    // For each frame, search for similar frames from other videos
    // Track shared frame counts between video pairs
    let mut shared_frames: HashMap<(String, String), HashSet<(usize, usize)>> = HashMap::new();
    let search_limit = 10;

    for &frame_idx in &frame_indices {
        let record = &records[frame_idx];
        let source_video = idx_to_video.get(&frame_idx).unwrap();

        let results = catalog.search_vector(&record.embedding, search_limit)?;

        for (neighbor, distance) in &results {
            if neighbor.id == record.id {
                continue;
            }
            if neighbor.media_type != "video_frame" {
                continue;
            }

            let similarity = 1.0 - distance / 2.0;
            if similarity < threshold {
                continue;
            }

            // Find the neighbor's source video
            let neighbor_video = neighbor
                .source_video_path
                .as_deref()
                .unwrap_or(&neighbor.file_path);

            if neighbor_video == source_video {
                continue; // Same video, skip
            }

            // Order the pair consistently
            let pair = if source_video.as_str() < neighbor_video {
                (source_video.clone(), neighbor_video.to_string())
            } else {
                (neighbor_video.to_string(), source_video.clone())
            };

            // Find neighbor index
            if let Some(neighbor_idx) = records
                .iter()
                .enumerate()
                .find(|(_, r)| r.id == neighbor.id)
                .map(|(i, _)| i)
            {
                shared_frames
                    .entry(pair)
                    .or_default()
                    .insert((frame_idx.min(neighbor_idx), frame_idx.max(neighbor_idx)));
            }
        }
    }

    // Build results
    let mut duplicates = Vec::new();
    for ((video_a, video_b), frame_pairs) in shared_frames {
        let total_a = frames_by_video.get(&video_a).map_or(0, |v| v.len());
        let total_b = frames_by_video.get(&video_b).map_or(0, |v| v.len());
        let shared = frame_pairs.len();

        let min_total = total_a.min(total_b).max(1);
        let overlap_ratio = shared as f32 / min_total as f32;

        duplicates.push(CrossVideoDuplicate {
            video_a_path: video_a,
            video_b_path: video_b,
            shared_frame_count: shared,
            total_frames_a: total_a,
            total_frames_b: total_b,
            overlap_ratio,
        });
    }

    // Sort by overlap ratio descending
    duplicates.sort_by(|a, b| b.overlap_ratio.partial_cmp(&a.overlap_ratio).unwrap());

    Ok(duplicates)
}

#[cfg(test)]
mod tests {
    use super::*;
    use catalogy_catalog::CatalogRecord;

    fn make_frame_record(
        id: &str,
        source_video: &str,
        frame_index: i32,
        embedding: Vec<f32>,
    ) -> CatalogRecord {
        CatalogRecord {
            id: id.to_string(),
            file_hash: format!("hash_{}", source_video.replace('/', "_")),
            file_path: source_video.to_string(),
            file_name: format!("frame_{}.jpg", frame_index),
            file_size: 1024,
            file_ext: "jpg".to_string(),
            media_type: "video_frame".to_string(),
            embedding,
            model_id: "clip-vit-h-14".to_string(),
            model_version: "1".to_string(),
            width: Some(512),
            height: Some(512),
            duration_ms: None,
            fps: None,
            codec: None,
            bitrate_kbps: None,
            exif_camera_make: None,
            exif_camera_model: None,
            exif_date_taken: None,
            exif_gps_lat: None,
            exif_gps_lon: None,
            exif_focal_length_mm: None,
            exif_iso: None,
            exif_orientation: None,
            source_video_path: Some(source_video.to_string()),
            frame_index: Some(frame_index),
            frame_timestamp_ms: Some(frame_index as i64 * 1000),
            file_created: None,
            file_modified: None,
            indexed_at: "2024-01-01T00:00:00Z".to_string(),
            updated_at: "2024-01-01T00:00:00Z".to_string(),
            tombstone: false,
        }
    }

    fn normalized(v: &[f32]) -> Vec<f32> {
        let norm: f32 = v.iter().map(|x| x * x).sum::<f32>().sqrt();
        if norm == 0.0 {
            return v.to_vec();
        }
        v.iter().map(|x| x / norm).collect()
    }

    #[test]
    fn test_cross_video_empty() {
        let dir = tempfile::tempdir().unwrap();
        let catalog = Catalog::open(dir.path().join("catalog").to_str().unwrap()).unwrap();

        let results = find_cross_video_duplicates(&catalog, 0.90).unwrap();
        assert!(results.is_empty());
    }

    #[test]
    fn test_cross_video_shared_frames() {
        let dir = tempfile::tempdir().unwrap();
        let catalog = Catalog::open(dir.path().join("catalog").to_str().unwrap()).unwrap();

        // Create a shared embedding (similar frame in both videos)
        let mut shared_base = vec![0.0_f32; 1024];
        shared_base[0] = 1.0;
        shared_base[1] = 0.5;
        let shared_emb = normalized(&shared_base);

        // Slightly different version of the shared frame
        shared_base[1] = 0.51;
        let shared_emb2 = normalized(&shared_base);

        // Unique frame for video A
        let mut unique_a = vec![0.0_f32; 1024];
        unique_a[100] = 1.0;
        let unique_emb_a = normalized(&unique_a);

        // Unique frame for video B
        let mut unique_b = vec![0.0_f32; 1024];
        unique_b[500] = 1.0;
        let unique_emb_b = normalized(&unique_b);

        let records = vec![
            make_frame_record("va-f0", "/videos/a.mp4", 0, shared_emb.clone()),
            make_frame_record("va-f1", "/videos/a.mp4", 1, unique_emb_a),
            make_frame_record("vb-f0", "/videos/b.mp4", 0, shared_emb2),
            make_frame_record("vb-f1", "/videos/b.mp4", 1, unique_emb_b),
        ];

        catalog.batch_upsert(&records).unwrap();

        let results = find_cross_video_duplicates(&catalog, 0.90).unwrap();

        // Should detect that video A and B share at least one similar frame
        assert!(
            !results.is_empty(),
            "Expected cross-video duplicates, got none"
        );

        let pair = &results[0];
        assert!(pair.shared_frame_count >= 1);
        assert_eq!(pair.total_frames_a, 2);
        assert_eq!(pair.total_frames_b, 2);
        assert!(pair.overlap_ratio > 0.0);
    }

    #[test]
    fn test_cross_video_no_overlap() {
        let dir = tempfile::tempdir().unwrap();
        let catalog = Catalog::open(dir.path().join("catalog").to_str().unwrap()).unwrap();

        // Video A frames - completely different from video B
        let mut emb_a1 = vec![0.0_f32; 1024];
        emb_a1[0] = 1.0;
        let mut emb_a2 = vec![0.0_f32; 1024];
        emb_a2[1] = 1.0;

        // Video B frames - completely different from video A
        let mut emb_b1 = vec![0.0_f32; 1024];
        emb_b1[500] = 1.0;
        let mut emb_b2 = vec![0.0_f32; 1024];
        emb_b2[501] = 1.0;

        let records = vec![
            make_frame_record("va-0", "/videos/a.mp4", 0, normalized(&emb_a1)),
            make_frame_record("va-1", "/videos/a.mp4", 1, normalized(&emb_a2)),
            make_frame_record("vb-0", "/videos/b.mp4", 0, normalized(&emb_b1)),
            make_frame_record("vb-1", "/videos/b.mp4", 1, normalized(&emb_b2)),
        ];

        catalog.batch_upsert(&records).unwrap();

        let results = find_cross_video_duplicates(&catalog, 0.92).unwrap();
        assert!(
            results.is_empty(),
            "Expected no cross-video duplicates for completely different videos"
        );
    }

    #[test]
    fn test_cross_video_duplicate_serialization() {
        let dup = CrossVideoDuplicate {
            video_a_path: "/videos/a.mp4".to_string(),
            video_b_path: "/videos/b.mp4".to_string(),
            shared_frame_count: 5,
            total_frames_a: 10,
            total_frames_b: 8,
            overlap_ratio: 0.625,
        };
        let json = serde_json::to_string(&dup).unwrap();
        assert!(json.contains("a.mp4"));
        assert!(json.contains("0.625"));
    }
}
