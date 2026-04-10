use crate::cross_video::CrossVideoDuplicate;
use crate::exact::DuplicateSet;
use crate::visual::VisualDuplicateCluster;

/// Format exact duplicate results as human-readable text.
pub fn format_exact_report(sets: &[DuplicateSet]) -> String {
    if sets.is_empty() {
        return "No exact duplicates found.\n".to_string();
    }

    let mut out = format!("=== Exact Duplicates: {} set(s) ===\n\n", sets.len());

    for (i, set) in sets.iter().enumerate() {
        out.push_str(&format!(
            "Set {} (hash: {}...)\n",
            i + 1,
            &set.file_hash[..set.file_hash.len().min(12)]
        ));

        for file in &set.files {
            out.push_str(&format!(
                "  {} ({} bytes, modified: {})\n",
                file.path, file.size, file.modified
            ));
        }
        out.push('\n');
    }

    let total_files: usize = sets.iter().map(|s| s.files.len()).sum();
    let wasted = total_files - sets.len(); // extra copies beyond one per set
    out.push_str(&format!(
        "Total: {} duplicate set(s), {} extra file(s) that could be removed.\n",
        sets.len(),
        wasted
    ));

    out
}

/// Format visual duplicate results as human-readable text.
pub fn format_visual_report(clusters: &[VisualDuplicateCluster]) -> String {
    if clusters.is_empty() {
        return "No visual duplicates found.\n".to_string();
    }

    let mut out = format!(
        "=== Visual Duplicates: {} cluster(s) ===\n\n",
        clusters.len()
    );

    for (i, cluster) in clusters.iter().enumerate() {
        out.push_str(&format!(
            "Cluster {} ({} items)\n",
            i + 1,
            cluster.items.len()
        ));

        for item in &cluster.items {
            out.push_str(&format!(
                "  [{}] {} (similarity: {:.3})\n",
                item.media_type, item.file_path, item.similarity
            ));
        }
        out.push('\n');
    }

    let total_items: usize = clusters.iter().map(|c| c.items.len()).sum();
    out.push_str(&format!(
        "Total: {} cluster(s), {} items involved.\n",
        clusters.len(),
        total_items
    ));

    out
}

/// Format cross-video duplicate results as human-readable text.
pub fn format_cross_video_report(duplicates: &[CrossVideoDuplicate]) -> String {
    if duplicates.is_empty() {
        return "No cross-video duplicates found.\n".to_string();
    }

    let mut out = format!(
        "=== Cross-Video Duplicates: {} pair(s) ===\n\n",
        duplicates.len()
    );

    for (i, dup) in duplicates.iter().enumerate() {
        out.push_str(&format!("Pair {}\n", i + 1));
        out.push_str(&format!(
            "  Video A: {} ({} frames)\n",
            dup.video_a_path, dup.total_frames_a
        ));
        out.push_str(&format!(
            "  Video B: {} ({} frames)\n",
            dup.video_b_path, dup.total_frames_b
        ));
        out.push_str(&format!(
            "  Shared frames: {} (overlap: {:.1}%)\n",
            dup.shared_frame_count,
            dup.overlap_ratio * 100.0
        ));
        out.push('\n');
    }

    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::exact::DuplicateFile;
    use crate::visual::VisualDuplicateItem;

    #[test]
    fn test_format_exact_empty() {
        let report = format_exact_report(&[]);
        assert!(report.contains("No exact duplicates"));
    }

    #[test]
    fn test_format_exact_with_sets() {
        let sets = vec![DuplicateSet {
            file_hash: "abcdef123456".to_string(),
            files: vec![
                DuplicateFile {
                    path: "/photos/a.jpg".to_string(),
                    size: 1024,
                    modified: "2024-01-01".to_string(),
                },
                DuplicateFile {
                    path: "/backup/a.jpg".to_string(),
                    size: 1024,
                    modified: "2024-01-01".to_string(),
                },
            ],
        }];

        let report = format_exact_report(&sets);
        assert!(report.contains("1 set(s)"));
        assert!(report.contains("/photos/a.jpg"));
        assert!(report.contains("/backup/a.jpg"));
        assert!(report.contains("1024 bytes"));
        assert!(report.contains("1 extra file(s)"));
    }

    #[test]
    fn test_format_visual_empty() {
        let report = format_visual_report(&[]);
        assert!(report.contains("No visual duplicates"));
    }

    #[test]
    fn test_format_visual_with_clusters() {
        let clusters = vec![VisualDuplicateCluster {
            items: vec![
                VisualDuplicateItem {
                    id: "id1".to_string(),
                    file_path: "/a.jpg".to_string(),
                    media_type: "image".to_string(),
                    similarity: 0.95,
                },
                VisualDuplicateItem {
                    id: "id2".to_string(),
                    file_path: "/b.jpg".to_string(),
                    media_type: "image".to_string(),
                    similarity: 0.95,
                },
            ],
        }];

        let report = format_visual_report(&clusters);
        assert!(report.contains("1 cluster(s)"));
        assert!(report.contains("/a.jpg"));
        assert!(report.contains("0.950"));
    }

    #[test]
    fn test_format_cross_video_empty() {
        let report = format_cross_video_report(&[]);
        assert!(report.contains("No cross-video duplicates"));
    }

    #[test]
    fn test_format_cross_video_with_pairs() {
        let dups = vec![CrossVideoDuplicate {
            video_a_path: "/videos/a.mp4".to_string(),
            video_b_path: "/videos/b.mp4".to_string(),
            shared_frame_count: 5,
            total_frames_a: 10,
            total_frames_b: 8,
            overlap_ratio: 0.625,
        }];

        let report = format_cross_video_report(&dups);
        assert!(report.contains("1 pair(s)"));
        assert!(report.contains("a.mp4"));
        assert!(report.contains("b.mp4"));
        assert!(report.contains("62.5%"));
    }
}
