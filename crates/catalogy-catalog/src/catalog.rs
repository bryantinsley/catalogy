use arrow_array::{
    Array, ArrayRef, BooleanArray, FixedSizeListArray, Float32Array, Float64Array, Int32Array,
    Int64Array, RecordBatch, RecordBatchIterator, StringArray,
};
use arrow_schema::{DataType, Field, SchemaRef};
use catalogy_core::{CatalogyError, Result};
use lancedb::connection::Connection;
use lancedb::query::{ExecutableQuery, QueryBase};
use lancedb::table::Table;
use std::sync::Arc;

use crate::record::CatalogRecord;
use crate::schema::media_schema;

const TABLE_NAME: &str = "media";

/// LanceDB-based catalog storage.
pub struct Catalog {
    connection: Connection,
    rt: tokio::runtime::Runtime,
}

impl Catalog {
    /// Open (or create) a catalog at the given path.
    pub fn open(path: &str) -> Result<Self> {
        let rt = tokio::runtime::Runtime::new()
            .map_err(|e| CatalogyError::Database(format!("Failed to create runtime: {}", e)))?;

        let connection = rt.block_on(async {
            lancedb::connect(path)
                .execute()
                .await
                .map_err(|e| {
                    CatalogyError::Database(format!("Failed to connect to LanceDB: {}", e))
                })
        })?;

        Ok(Self { connection, rt })
    }

    /// Insert or update a single record.
    pub fn upsert(&self, record: &CatalogRecord) -> Result<()> {
        self.batch_upsert(&[record.clone()])
    }

    /// Insert or update a batch of records.
    pub fn batch_upsert(&self, records: &[CatalogRecord]) -> Result<()> {
        if records.is_empty() {
            return Ok(());
        }

        let batch = records_to_batch(records)?;
        let schema = batch.schema();

        self.rt.block_on(async {
            let table = self.get_or_create_table(&batch).await?;

            let reader = make_reader(batch, schema);

            // Use merge_insert for upsert behavior
            let mut builder = table.merge_insert(&["id"]);
            builder.when_matched_update_all(None);
            builder.when_not_matched_insert_all();
            builder
                .execute(Box::new(reader))
                .await
                .map_err(|e| CatalogyError::Database(format!("Upsert failed: {}", e)))?;

            Ok(())
        })
    }

    /// Get a record by ID.
    pub fn get_by_id(&self, id: &str) -> Result<Option<CatalogRecord>> {
        self.rt.block_on(async {
            let table = match self.open_table().await {
                Ok(t) => t,
                Err(_) => return Ok(None),
            };

            let filter = format!("id = '{}'", id.replace('\'', "''"));
            let results = table
                .query()
                .only_if(filter)
                .limit(1)
                .execute()
                .await
                .map_err(|e| CatalogyError::Database(format!("Query failed: {}", e)))?;

            let batches = collect_batches(results).await?;
            if batches.is_empty() || batches[0].num_rows() == 0 {
                return Ok(None);
            }

            let records = batch_to_records(&batches[0])?;
            Ok(records.into_iter().next())
        })
    }

    /// Get records by file hash.
    pub fn get_by_hash(&self, hash: &str) -> Result<Vec<CatalogRecord>> {
        self.rt.block_on(async {
            let table = match self.open_table().await {
                Ok(t) => t,
                Err(_) => return Ok(Vec::new()),
            };

            let filter = format!("file_hash = '{}'", hash.replace('\'', "''"));
            let results = table
                .query()
                .only_if(filter)
                .execute()
                .await
                .map_err(|e| CatalogyError::Database(format!("Query failed: {}", e)))?;

            let batches = collect_batches(results).await?;
            let mut all_records = Vec::new();
            for batch in &batches {
                all_records.extend(batch_to_records(batch)?);
            }
            Ok(all_records)
        })
    }

    /// Search by vector similarity.
    pub fn search_vector(
        &self,
        query_vector: &[f32],
        limit: usize,
    ) -> Result<Vec<(CatalogRecord, f32)>> {
        self.rt.block_on(async {
            let table = match self.open_table().await {
                Ok(t) => t,
                Err(_) => return Ok(Vec::new()),
            };

            let results = table
                .vector_search(query_vector)
                .map_err(|e| {
                    CatalogyError::Database(format!("Vector search setup failed: {}", e))
                })?
                .limit(limit)
                .execute()
                .await
                .map_err(|e| {
                    CatalogyError::Database(format!("Vector search failed: {}", e))
                })?;

            let batches = collect_batches(results).await?;
            let mut results_with_scores = Vec::new();

            for batch in &batches {
                let records = batch_to_records(batch)?;

                // Extract distance scores if present
                let distances = batch
                    .column_by_name("_distance")
                    .and_then(|col| col.as_any().downcast_ref::<Float32Array>());

                for (i, record) in records.into_iter().enumerate() {
                    let score = distances.map(|d| d.value(i)).unwrap_or(0.0);
                    results_with_scores.push((record, score));
                }
            }

            Ok(results_with_scores)
        })
    }

    /// List all records in the catalog.
    pub fn list_all(&self) -> Result<Vec<CatalogRecord>> {
        self.rt.block_on(async {
            let table = match self.open_table().await {
                Ok(t) => t,
                Err(_) => return Ok(Vec::new()),
            };

            let results = table
                .query()
                .execute()
                .await
                .map_err(|e| CatalogyError::Database(format!("Query failed: {}", e)))?;

            let batches = collect_batches(results).await?;
            let mut all_records = Vec::new();
            for batch in &batches {
                all_records.extend(batch_to_records(batch)?);
            }
            Ok(all_records)
        })
    }

    /// Count total records.
    pub fn count(&self) -> Result<u64> {
        self.rt.block_on(async {
            let table = match self.open_table().await {
                Ok(t) => t,
                Err(_) => return Ok(0),
            };

            let count = table
                .count_rows(None)
                .await
                .map_err(|e| CatalogyError::Database(format!("Count failed: {}", e)))?;

            Ok(count as u64)
        })
    }

    /// Build an IVF-PQ index on the embedding column.
    pub fn build_index(&self, num_partitions: u32) -> Result<()> {
        self.rt.block_on(async {
            let table = self.open_table().await?;

            table
                .create_index(
                    &["embedding"],
                    lancedb::index::Index::IvfPq(
                        lancedb::index::vector::IvfPqIndexBuilder::default()
                            .num_partitions(num_partitions),
                    ),
                )
                .execute()
                .await
                .map_err(|e| CatalogyError::Database(format!("Index build failed: {}", e)))?;

            Ok(())
        })
    }

    async fn open_table(&self) -> Result<Table> {
        self.connection
            .open_table(TABLE_NAME)
            .execute()
            .await
            .map_err(|e| CatalogyError::Database(format!("Failed to open table: {}", e)))
    }

    async fn get_or_create_table(&self, batch: &RecordBatch) -> Result<Table> {
        match self.connection.open_table(TABLE_NAME).execute().await {
            Ok(table) => Ok(table),
            Err(_) => {
                let schema = batch.schema();
                let reader = make_reader(batch.clone(), schema);
                self.connection
                    .create_table(TABLE_NAME, Box::new(reader))
                    .execute()
                    .await
                    .map_err(|e| {
                        CatalogyError::Database(format!("Failed to create table: {}", e))
                    })
            }
        }
    }
}

/// Wrap a RecordBatch into a RecordBatchIterator (implements RecordBatchReader).
fn make_reader(
    batch: RecordBatch,
    schema: SchemaRef,
) -> RecordBatchIterator<std::vec::IntoIter<std::result::Result<RecordBatch, arrow::error::ArrowError>>>
{
    RecordBatchIterator::new(vec![Ok(batch)].into_iter(), schema)
}

/// Convert CatalogRecords to an Arrow RecordBatch.
fn records_to_batch(records: &[CatalogRecord]) -> Result<RecordBatch> {
    let schema = Arc::new(media_schema());

    let id_arr = StringArray::from(
        records.iter().map(|r| r.id.as_str()).collect::<Vec<_>>(),
    );
    let file_hash_arr = StringArray::from(
        records
            .iter()
            .map(|r| r.file_hash.as_str())
            .collect::<Vec<_>>(),
    );
    let file_path_arr = StringArray::from(
        records
            .iter()
            .map(|r| r.file_path.as_str())
            .collect::<Vec<_>>(),
    );
    let file_name_arr = StringArray::from(
        records
            .iter()
            .map(|r| r.file_name.as_str())
            .collect::<Vec<_>>(),
    );
    let file_size_arr =
        Int64Array::from(records.iter().map(|r| r.file_size).collect::<Vec<_>>());
    let file_ext_arr = StringArray::from(
        records
            .iter()
            .map(|r| r.file_ext.as_str())
            .collect::<Vec<_>>(),
    );
    let media_type_arr = StringArray::from(
        records
            .iter()
            .map(|r| r.media_type.as_str())
            .collect::<Vec<_>>(),
    );

    // Build FixedSizeList for embeddings
    let embedding_values: Vec<f32> = records
        .iter()
        .flat_map(|r| r.embedding.iter().copied())
        .collect();
    let values_arr = Float32Array::from(embedding_values);
    let field = Arc::new(Field::new("item", DataType::Float32, true));
    let embedding_arr =
        FixedSizeListArray::new(field, 1024, Arc::new(values_arr) as ArrayRef, None);

    let model_id_arr = StringArray::from(
        records
            .iter()
            .map(|r| r.model_id.as_str())
            .collect::<Vec<_>>(),
    );
    let model_version_arr = StringArray::from(
        records
            .iter()
            .map(|r| r.model_version.as_str())
            .collect::<Vec<_>>(),
    );

    // Optional fields
    let width_arr = Int32Array::from(records.iter().map(|r| r.width).collect::<Vec<_>>());
    let height_arr = Int32Array::from(records.iter().map(|r| r.height).collect::<Vec<_>>());
    let duration_arr =
        Int64Array::from(records.iter().map(|r| r.duration_ms).collect::<Vec<_>>());
    let fps_arr = Float32Array::from(records.iter().map(|r| r.fps).collect::<Vec<_>>());
    let codec_arr = StringArray::from(
        records
            .iter()
            .map(|r| r.codec.as_deref())
            .collect::<Vec<_>>(),
    );
    let bitrate_arr =
        Int32Array::from(records.iter().map(|r| r.bitrate_kbps).collect::<Vec<_>>());

    // EXIF
    let exif_make_arr = StringArray::from(
        records
            .iter()
            .map(|r| r.exif_camera_make.as_deref())
            .collect::<Vec<_>>(),
    );
    let exif_model_arr = StringArray::from(
        records
            .iter()
            .map(|r| r.exif_camera_model.as_deref())
            .collect::<Vec<_>>(),
    );
    let exif_date_arr = StringArray::from(
        records
            .iter()
            .map(|r| r.exif_date_taken.as_deref())
            .collect::<Vec<_>>(),
    );
    let exif_lat_arr =
        Float64Array::from(records.iter().map(|r| r.exif_gps_lat).collect::<Vec<_>>());
    let exif_lon_arr =
        Float64Array::from(records.iter().map(|r| r.exif_gps_lon).collect::<Vec<_>>());
    let exif_fl_arr = Float32Array::from(
        records
            .iter()
            .map(|r| r.exif_focal_length_mm)
            .collect::<Vec<_>>(),
    );
    let exif_iso_arr =
        Int32Array::from(records.iter().map(|r| r.exif_iso).collect::<Vec<_>>());
    let exif_orient_arr =
        Int32Array::from(records.iter().map(|r| r.exif_orientation).collect::<Vec<_>>());

    // Video frame
    let source_video_arr = StringArray::from(
        records
            .iter()
            .map(|r| r.source_video_path.as_deref())
            .collect::<Vec<_>>(),
    );
    let frame_idx_arr =
        Int32Array::from(records.iter().map(|r| r.frame_index).collect::<Vec<_>>());
    let frame_ts_arr =
        Int64Array::from(records.iter().map(|r| r.frame_timestamp_ms).collect::<Vec<_>>());

    // Timestamps
    let file_created_arr = StringArray::from(
        records
            .iter()
            .map(|r| r.file_created.as_deref())
            .collect::<Vec<_>>(),
    );
    let file_modified_arr = StringArray::from(
        records
            .iter()
            .map(|r| r.file_modified.as_deref())
            .collect::<Vec<_>>(),
    );
    let indexed_at_arr = StringArray::from(
        records
            .iter()
            .map(|r| r.indexed_at.as_str())
            .collect::<Vec<_>>(),
    );
    let updated_at_arr = StringArray::from(
        records
            .iter()
            .map(|r| r.updated_at.as_str())
            .collect::<Vec<_>>(),
    );
    let tombstone_arr =
        BooleanArray::from(records.iter().map(|r| r.tombstone).collect::<Vec<_>>());

    let batch = RecordBatch::try_new(
        schema,
        vec![
            Arc::new(id_arr),
            Arc::new(file_hash_arr),
            Arc::new(file_path_arr),
            Arc::new(file_name_arr),
            Arc::new(file_size_arr),
            Arc::new(file_ext_arr),
            Arc::new(media_type_arr),
            Arc::new(embedding_arr),
            Arc::new(model_id_arr),
            Arc::new(model_version_arr),
            Arc::new(width_arr),
            Arc::new(height_arr),
            Arc::new(duration_arr),
            Arc::new(fps_arr),
            Arc::new(codec_arr),
            Arc::new(bitrate_arr),
            Arc::new(exif_make_arr),
            Arc::new(exif_model_arr),
            Arc::new(exif_date_arr),
            Arc::new(exif_lat_arr),
            Arc::new(exif_lon_arr),
            Arc::new(exif_fl_arr),
            Arc::new(exif_iso_arr),
            Arc::new(exif_orient_arr),
            Arc::new(source_video_arr),
            Arc::new(frame_idx_arr),
            Arc::new(frame_ts_arr),
            Arc::new(file_created_arr),
            Arc::new(file_modified_arr),
            Arc::new(indexed_at_arr),
            Arc::new(updated_at_arr),
            Arc::new(tombstone_arr),
        ],
    )
    .map_err(|e| CatalogyError::Database(format!("Failed to create RecordBatch: {}", e)))?;

    Ok(batch)
}

/// Convert an Arrow RecordBatch back to CatalogRecords.
fn batch_to_records(batch: &RecordBatch) -> Result<Vec<CatalogRecord>> {
    let n = batch.num_rows();
    let mut records = Vec::with_capacity(n);

    let id_col = get_string_col(batch, "id")?;
    let file_hash_col = get_string_col(batch, "file_hash")?;
    let file_path_col = get_string_col(batch, "file_path")?;
    let file_name_col = get_string_col(batch, "file_name")?;
    let file_size_col = get_i64_col(batch, "file_size")?;
    let file_ext_col = get_string_col(batch, "file_ext")?;
    let media_type_col = get_string_col(batch, "media_type")?;
    let model_id_col = get_string_col(batch, "model_id")?;
    let model_version_col = get_string_col(batch, "model_version")?;

    // Embedding column
    let embedding_col = batch
        .column_by_name("embedding")
        .ok_or_else(|| CatalogyError::Database("Missing embedding column".to_string()))?;
    let embedding_list = embedding_col
        .as_any()
        .downcast_ref::<FixedSizeListArray>()
        .ok_or_else(|| {
            CatalogyError::Database("embedding is not FixedSizeList".to_string())
        })?;

    // Optional columns
    let width_col = get_opt_i32_col(batch, "width");
    let height_col = get_opt_i32_col(batch, "height");
    let duration_col = get_opt_i64_col(batch, "duration_ms");
    let fps_col = get_opt_f32_col(batch, "fps");
    let codec_col = get_opt_string_col(batch, "codec");
    let bitrate_col = get_opt_i32_col(batch, "bitrate_kbps");

    let exif_make_col = get_opt_string_col(batch, "exif_camera_make");
    let exif_model_col = get_opt_string_col(batch, "exif_camera_model");
    let exif_date_col = get_opt_string_col(batch, "exif_date_taken");
    let exif_lat_col = get_opt_f64_col(batch, "exif_gps_lat");
    let exif_lon_col = get_opt_f64_col(batch, "exif_gps_lon");
    let exif_fl_col = get_opt_f32_col(batch, "exif_focal_length_mm");
    let exif_iso_col = get_opt_i32_col(batch, "exif_iso");
    let exif_orient_col = get_opt_i32_col(batch, "exif_orientation");

    let source_video_col = get_opt_string_col(batch, "source_video_path");
    let frame_idx_col = get_opt_i32_col(batch, "frame_index");
    let frame_ts_col = get_opt_i64_col(batch, "frame_timestamp_ms");

    let file_created_col = get_opt_string_col(batch, "file_created");
    let file_modified_col = get_opt_string_col(batch, "file_modified");
    let indexed_at_col = get_string_col(batch, "indexed_at")?;
    let updated_at_col = get_string_col(batch, "updated_at")?;
    let tombstone_col = batch
        .column_by_name("tombstone")
        .and_then(|c| c.as_any().downcast_ref::<BooleanArray>());

    for i in 0..n {
        // Extract embedding vector
        let emb_values = embedding_list.value(i);
        let emb_f32 = emb_values
            .as_any()
            .downcast_ref::<Float32Array>()
            .ok_or_else(|| {
                CatalogyError::Database("embedding values are not Float32".to_string())
            })?;
        let embedding: Vec<f32> = (0..emb_f32.len()).map(|j| emb_f32.value(j)).collect();

        records.push(CatalogRecord {
            id: id_col.value(i).to_string(),
            file_hash: file_hash_col.value(i).to_string(),
            file_path: file_path_col.value(i).to_string(),
            file_name: file_name_col.value(i).to_string(),
            file_size: file_size_col.value(i),
            file_ext: file_ext_col.value(i).to_string(),
            media_type: media_type_col.value(i).to_string(),
            embedding,
            model_id: model_id_col.value(i).to_string(),
            model_version: model_version_col.value(i).to_string(),
            width: width_col
                .and_then(|c| if c.is_null(i) { None } else { Some(c.value(i)) }),
            height: height_col
                .and_then(|c| if c.is_null(i) { None } else { Some(c.value(i)) }),
            duration_ms: duration_col
                .and_then(|c| if c.is_null(i) { None } else { Some(c.value(i)) }),
            fps: fps_col
                .and_then(|c| if c.is_null(i) { None } else { Some(c.value(i)) }),
            codec: codec_col.and_then(|c| {
                if c.is_null(i) {
                    None
                } else {
                    Some(c.value(i).to_string())
                }
            }),
            bitrate_kbps: bitrate_col
                .and_then(|c| if c.is_null(i) { None } else { Some(c.value(i)) }),
            exif_camera_make: exif_make_col.and_then(|c| {
                if c.is_null(i) {
                    None
                } else {
                    Some(c.value(i).to_string())
                }
            }),
            exif_camera_model: exif_model_col.and_then(|c| {
                if c.is_null(i) {
                    None
                } else {
                    Some(c.value(i).to_string())
                }
            }),
            exif_date_taken: exif_date_col.and_then(|c| {
                if c.is_null(i) {
                    None
                } else {
                    Some(c.value(i).to_string())
                }
            }),
            exif_gps_lat: exif_lat_col
                .and_then(|c| if c.is_null(i) { None } else { Some(c.value(i)) }),
            exif_gps_lon: exif_lon_col
                .and_then(|c| if c.is_null(i) { None } else { Some(c.value(i)) }),
            exif_focal_length_mm: exif_fl_col
                .and_then(|c| if c.is_null(i) { None } else { Some(c.value(i)) }),
            exif_iso: exif_iso_col
                .and_then(|c| if c.is_null(i) { None } else { Some(c.value(i)) }),
            exif_orientation: exif_orient_col
                .and_then(|c| if c.is_null(i) { None } else { Some(c.value(i)) }),
            source_video_path: source_video_col.and_then(|c| {
                if c.is_null(i) {
                    None
                } else {
                    Some(c.value(i).to_string())
                }
            }),
            frame_index: frame_idx_col
                .and_then(|c| if c.is_null(i) { None } else { Some(c.value(i)) }),
            frame_timestamp_ms: frame_ts_col
                .and_then(|c| if c.is_null(i) { None } else { Some(c.value(i)) }),
            file_created: file_created_col.and_then(|c| {
                if c.is_null(i) {
                    None
                } else {
                    Some(c.value(i).to_string())
                }
            }),
            file_modified: file_modified_col.and_then(|c| {
                if c.is_null(i) {
                    None
                } else {
                    Some(c.value(i).to_string())
                }
            }),
            indexed_at: indexed_at_col.value(i).to_string(),
            updated_at: updated_at_col.value(i).to_string(),
            tombstone: tombstone_col.map(|c| c.value(i)).unwrap_or(false),
        });
    }

    Ok(records)
}

// Helper functions for extracting typed columns
fn get_string_col<'a>(batch: &'a RecordBatch, name: &str) -> Result<&'a StringArray> {
    batch
        .column_by_name(name)
        .and_then(|c| c.as_any().downcast_ref::<StringArray>())
        .ok_or_else(|| {
            CatalogyError::Database(format!("Missing or invalid column: {}", name))
        })
}

fn get_i64_col<'a>(batch: &'a RecordBatch, name: &str) -> Result<&'a Int64Array> {
    batch
        .column_by_name(name)
        .and_then(|c| c.as_any().downcast_ref::<Int64Array>())
        .ok_or_else(|| {
            CatalogyError::Database(format!("Missing or invalid column: {}", name))
        })
}

fn get_opt_string_col<'a>(
    batch: &'a RecordBatch,
    name: &str,
) -> Option<&'a StringArray> {
    batch
        .column_by_name(name)
        .and_then(|c| c.as_any().downcast_ref::<StringArray>())
}

fn get_opt_i32_col<'a>(batch: &'a RecordBatch, name: &str) -> Option<&'a Int32Array> {
    batch
        .column_by_name(name)
        .and_then(|c| c.as_any().downcast_ref::<Int32Array>())
}

fn get_opt_i64_col<'a>(batch: &'a RecordBatch, name: &str) -> Option<&'a Int64Array> {
    batch
        .column_by_name(name)
        .and_then(|c| c.as_any().downcast_ref::<Int64Array>())
}

fn get_opt_f32_col<'a>(
    batch: &'a RecordBatch,
    name: &str,
) -> Option<&'a Float32Array> {
    batch
        .column_by_name(name)
        .and_then(|c| c.as_any().downcast_ref::<Float32Array>())
}

fn get_opt_f64_col<'a>(
    batch: &'a RecordBatch,
    name: &str,
) -> Option<&'a Float64Array> {
    batch
        .column_by_name(name)
        .and_then(|c| c.as_any().downcast_ref::<Float64Array>())
}

async fn collect_batches(
    mut stream: impl futures::Stream<
            Item = std::result::Result<RecordBatch, lancedb::error::Error>,
        > + Unpin,
) -> Result<Vec<RecordBatch>> {
    use futures::StreamExt;
    let mut batches = Vec::new();
    while let Some(result) = stream.next().await {
        let batch =
            result.map_err(|e| CatalogyError::Database(format!("Stream error: {}", e)))?;
        batches.push(batch);
    }
    Ok(batches)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_record(id: &str, hash: &str) -> CatalogRecord {
        CatalogRecord {
            id: id.to_string(),
            file_hash: hash.to_string(),
            file_path: format!("/test/{}.jpg", id),
            file_name: format!("{}.jpg", id),
            file_size: 1024,
            file_ext: "jpg".to_string(),
            media_type: "image".to_string(),
            embedding: vec![0.1; 1024],
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

    #[test]
    fn test_records_to_batch_roundtrip() {
        let records = vec![test_record("id1", "hash1"), test_record("id2", "hash2")];

        let batch = records_to_batch(&records).unwrap();
        assert_eq!(batch.num_rows(), 2);

        let recovered = batch_to_records(&batch).unwrap();
        assert_eq!(recovered.len(), 2);
        assert_eq!(recovered[0].id, "id1");
        assert_eq!(recovered[1].id, "id2");
        assert_eq!(recovered[0].file_hash, "hash1");
        assert_eq!(recovered[0].embedding.len(), 1024);
        assert_eq!(recovered[0].width, Some(1920));
        assert_eq!(recovered[0].duration_ms, None);
    }

    #[test]
    fn test_catalog_open_and_upsert() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("test_catalog");

        let catalog = Catalog::open(path.to_str().unwrap()).unwrap();
        let record = test_record("test-1", "hash-1");
        catalog.upsert(&record).unwrap();

        let count = catalog.count().unwrap();
        assert_eq!(count, 1);
    }

    #[test]
    fn test_catalog_get_by_id() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("test_catalog");

        let catalog = Catalog::open(path.to_str().unwrap()).unwrap();
        let record = test_record("find-me", "hash-find");
        catalog.upsert(&record).unwrap();

        let found = catalog.get_by_id("find-me").unwrap().unwrap();
        assert_eq!(found.file_hash, "hash-find");
        assert_eq!(found.file_name, "find-me.jpg");
    }

    #[test]
    fn test_catalog_get_by_hash() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("test_catalog");

        let catalog = Catalog::open(path.to_str().unwrap()).unwrap();
        catalog.upsert(&test_record("r1", "shared-hash")).unwrap();
        catalog.upsert(&test_record("r2", "shared-hash")).unwrap();

        let found = catalog.get_by_hash("shared-hash").unwrap();
        assert_eq!(found.len(), 2);
    }

    #[test]
    fn test_catalog_batch_upsert() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("test_catalog");

        let catalog = Catalog::open(path.to_str().unwrap()).unwrap();
        let records: Vec<CatalogRecord> = (0..10)
            .map(|i| test_record(&format!("batch-{}", i), &format!("hash-{}", i)))
            .collect();

        catalog.batch_upsert(&records).unwrap();
        assert_eq!(catalog.count().unwrap(), 10);
    }

    #[test]
    fn test_catalog_upsert_updates_existing() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("test_catalog");

        let catalog = Catalog::open(path.to_str().unwrap()).unwrap();

        let mut record = test_record("update-me", "hash-u");
        record.width = Some(800);
        catalog.upsert(&record).unwrap();

        record.width = Some(1600);
        catalog.upsert(&record).unwrap();

        assert_eq!(catalog.count().unwrap(), 1);
        let found = catalog.get_by_id("update-me").unwrap().unwrap();
        assert_eq!(found.width, Some(1600));
    }

    #[test]
    fn test_catalog_vector_search() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("test_catalog");

        let catalog = Catalog::open(path.to_str().unwrap()).unwrap();

        let mut r1 = test_record("vs-1", "h-1");
        r1.embedding = vec![1.0; 1024];
        let mut r2 = test_record("vs-2", "h-2");
        r2.embedding = vec![0.0; 1024];
        r2.embedding[0] = 1.0;

        catalog.batch_upsert(&[r1, r2]).unwrap();

        let query = vec![1.0; 1024];
        let results = catalog.search_vector(&query, 10).unwrap();
        assert!(!results.is_empty());
    }

    #[test]
    fn test_catalog_empty_operations() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("test_catalog");

        let catalog = Catalog::open(path.to_str().unwrap()).unwrap();

        assert_eq!(catalog.count().unwrap(), 0);
        assert!(catalog.get_by_id("nonexistent").unwrap().is_none());
        assert!(catalog.get_by_hash("nonexistent").unwrap().is_empty());
    }

    #[test]
    fn test_catalog_batch_upsert_empty() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("test_catalog");

        let catalog = Catalog::open(path.to_str().unwrap()).unwrap();
        catalog.batch_upsert(&[]).unwrap();
        assert_eq!(catalog.count().unwrap(), 0);
    }
}
