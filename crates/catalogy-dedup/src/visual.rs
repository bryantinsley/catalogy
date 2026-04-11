use catalogy_catalog::Catalog;
use catalogy_core::Result;
use serde::Serialize;
use std::collections::HashMap;

/// A cluster of visually similar media items.
#[derive(Clone, Debug, Serialize)]
pub struct VisualDuplicateCluster {
    pub items: Vec<VisualDuplicateItem>,
}

/// An item within a visual duplicate cluster.
#[derive(Clone, Debug, Serialize)]
pub struct VisualDuplicateItem {
    pub id: String,
    pub file_path: String,
    pub media_type: String,
    pub similarity: f32,
}

/// Find near-visual duplicates by searching for catalog items with
/// cosine similarity above the given threshold.
///
/// Uses union-find to build transitive clusters: if A~B and B~C, then {A,B,C}.
pub fn find_visual_duplicates(
    catalog: &Catalog,
    threshold: f32,
) -> Result<Vec<VisualDuplicateCluster>> {
    let records = catalog.list_all()?;
    if records.is_empty() {
        return Ok(Vec::new());
    }

    // Map id -> index for union-find
    let id_to_idx: HashMap<String, usize> = records
        .iter()
        .enumerate()
        .map(|(i, r)| (r.id.clone(), i))
        .collect();

    let mut uf = UnionFind::new(records.len());

    // For each record, find neighbors above threshold
    // LanceDB returns _distance (L2 distance). For cosine similarity with normalized vectors,
    // similarity = 1 - distance/2. We search with a reasonable limit.
    let search_limit = 20;

    // Track similarity scores per pair for reporting
    let mut pair_similarities: HashMap<(usize, usize), f32> = HashMap::new();

    for (i, record) in records.iter().enumerate() {
        // Skip video_frame -> parent_video matches
        if record.media_type == "video_frame" {
            continue;
        }

        let results = catalog.search_vector(&record.embedding, search_limit)?;

        for (neighbor, distance) in &results {
            // Skip self-matches
            if neighbor.id == record.id {
                continue;
            }

            // Skip video_frame entries
            if neighbor.media_type == "video_frame" {
                continue;
            }

            // Convert L2 distance to cosine similarity for normalized vectors:
            // cos_sim = 1 - (distance^2 / 2) for unit vectors
            // LanceDB returns squared L2 distance by default
            let similarity = 1.0 - distance / 2.0;

            if similarity >= threshold {
                if let Some(&j) = id_to_idx.get(&neighbor.id) {
                    uf.union(i, j);
                    let key = (i.min(j), i.max(j));
                    pair_similarities
                        .entry(key)
                        .and_modify(|s| {
                            if similarity > *s {
                                *s = similarity;
                            }
                        })
                        .or_insert(similarity);
                }
            }
        }
    }

    // Build clusters from union-find
    let mut clusters_map: HashMap<usize, Vec<usize>> = HashMap::new();
    for (i, record) in records.iter().enumerate() {
        if record.media_type == "video_frame" {
            continue;
        }
        let root = uf.find(i);
        clusters_map.entry(root).or_default().push(i);
    }

    // Only keep clusters with more than one item
    let mut clusters = Vec::new();
    for (_root, indices) in clusters_map {
        if indices.len() < 2 {
            continue;
        }

        let items: Vec<VisualDuplicateItem> = indices
            .iter()
            .map(|&idx| {
                // Find max similarity this item has with any other in the cluster
                let max_sim = indices
                    .iter()
                    .filter(|&&other| other != idx)
                    .filter_map(|&other| {
                        let key = (idx.min(other), idx.max(other));
                        pair_similarities.get(&key).copied()
                    })
                    .fold(0.0_f32, f32::max);

                VisualDuplicateItem {
                    id: records[idx].id.clone(),
                    file_path: records[idx].file_path.clone(),
                    media_type: records[idx].media_type.clone(),
                    similarity: max_sim,
                }
            })
            .collect();

        clusters.push(VisualDuplicateCluster { items });
    }

    Ok(clusters)
}

/// Simple union-find (disjoint set) data structure.
struct UnionFind {
    parent: Vec<usize>,
    rank: Vec<usize>,
}

impl UnionFind {
    fn new(n: usize) -> Self {
        Self {
            parent: (0..n).collect(),
            rank: vec![0; n],
        }
    }

    fn find(&mut self, x: usize) -> usize {
        if self.parent[x] != x {
            self.parent[x] = self.find(self.parent[x]);
        }
        self.parent[x]
    }

    fn union(&mut self, x: usize, y: usize) {
        let rx = self.find(x);
        let ry = self.find(y);
        if rx == ry {
            return;
        }
        if self.rank[rx] < self.rank[ry] {
            self.parent[rx] = ry;
        } else if self.rank[rx] > self.rank[ry] {
            self.parent[ry] = rx;
        } else {
            self.parent[ry] = rx;
            self.rank[rx] += 1;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use catalogy_catalog::CatalogRecord;

    fn make_record(id: &str, media_type: &str, embedding: Vec<f32>) -> CatalogRecord {
        CatalogRecord {
            id: id.to_string(),
            file_hash: format!("hash_{}", id),
            file_path: format!("/test/{}.jpg", id),
            file_name: format!("{}.jpg", id),
            file_size: 1024,
            file_ext: "jpg".to_string(),
            media_type: media_type.to_string(),
            embedding,
            model_id: "clip-vit-h-14".to_string(),
            model_version: "1".to_string(),
            width: Some(1920),
            height: Some(1080),
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
            source_video_path: None,
            frame_index: None,
            frame_timestamp_ms: None,
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
    fn test_union_find() {
        let mut uf = UnionFind::new(5);
        uf.union(0, 1);
        uf.union(2, 3);
        uf.union(1, 3);

        assert_eq!(uf.find(0), uf.find(3));
        assert_ne!(uf.find(0), uf.find(4));
    }

    #[test]
    fn test_visual_duplicates_empty_catalog() {
        let dir = tempfile::tempdir().unwrap();
        let catalog = Catalog::open(dir.path().join("catalog").to_str().unwrap()).unwrap();

        let clusters = find_visual_duplicates(&catalog, 0.92).unwrap();
        assert!(clusters.is_empty());
    }

    #[test]
    fn test_visual_duplicates_with_similar_items() {
        let dir = tempfile::tempdir().unwrap();
        let catalog = Catalog::open(dir.path().join("catalog").to_str().unwrap()).unwrap();

        // Create two nearly identical embeddings (high cosine similarity)
        let mut base = vec![0.0_f32; 1024];
        base[0] = 1.0;
        base[1] = 0.5;
        let emb1 = normalized(&base);

        base[1] = 0.51; // Very slight change
        let emb2 = normalized(&base);

        // Create a very different embedding
        let mut different = vec![0.0_f32; 1024];
        different[500] = 1.0;
        different[501] = 0.5;
        let emb3 = normalized(&different);

        let r1 = make_record("sim-1", "image", emb1);
        let r2 = make_record("sim-2", "image", emb2);
        let r3 = make_record("diff-1", "image", emb3);

        catalog.batch_upsert(&[r1, r2, r3]).unwrap();

        let clusters = find_visual_duplicates(&catalog, 0.90).unwrap();

        // sim-1 and sim-2 should be clustered together
        // diff-1 should not be in any cluster
        let has_similar_cluster = clusters.iter().any(|c| {
            c.items.len() == 2
                && c.items.iter().any(|i| i.id == "sim-1")
                && c.items.iter().any(|i| i.id == "sim-2")
        });
        assert!(
            has_similar_cluster,
            "Expected a cluster containing sim-1 and sim-2, got {:?}",
            clusters
        );
    }

    #[test]
    fn test_visual_duplicates_no_duplicates() {
        let dir = tempfile::tempdir().unwrap();
        let catalog = Catalog::open(dir.path().join("catalog").to_str().unwrap()).unwrap();

        // Create completely different embeddings
        let mut emb1 = vec![0.0_f32; 1024];
        emb1[0] = 1.0;
        let emb1 = normalized(&emb1);

        let mut emb2 = vec![0.0_f32; 1024];
        emb2[500] = 1.0;
        let emb2 = normalized(&emb2);

        let r1 = make_record("a", "image", emb1);
        let r2 = make_record("b", "image", emb2);
        catalog.batch_upsert(&[r1, r2]).unwrap();

        let clusters = find_visual_duplicates(&catalog, 0.92).unwrap();
        assert!(
            clusters.is_empty(),
            "Should find no clusters for very different embeddings"
        );
    }

    #[test]
    fn test_visual_duplicate_item_serialization() {
        let item = VisualDuplicateItem {
            id: "test-id".to_string(),
            file_path: "/test.jpg".to_string(),
            media_type: "image".to_string(),
            similarity: 0.95,
        };
        let json = serde_json::to_string(&item).unwrap();
        assert!(json.contains("test-id"));
        assert!(json.contains("0.95"));
    }
}
