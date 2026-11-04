//! Native LanceDB vector storage. Content and authorization remain in SQLite.
//! Writes are complete per-document commits; SDK write futures are not dropped
//! to pretend that an in-flight commit was rolled back.
use crate::content_store::{
    ContentChunk, ContentSearchFilter, ContentStore, ContentStoreError, ContentStoreErrorCode,
};
use crate::document_extract::SourceFingerprint;
use arrow_array::{
    types::Float32Type, Array, FixedSizeListArray, Float32Array, RecordBatch, RecordBatchIterator,
    StringArray,
};
use arrow_schema::{DataType, Field, Schema};
use futures::{future::Either, Future, TryStreamExt};
use lancedb::{
    expr::{col, is_in, lit},
    query::{ExecutableQuery, QueryBase, QueryExecutionOptions, Select},
    DistanceType, Table,
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    collections::{HashMap, HashSet},
    fs,
    path::{Path, PathBuf},
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
    time::Duration,
};

const MAX_DIMENSIONS: usize = 4096;
const MAX_CANDIDATES: usize = 8192;
const MAX_TOP_K: usize = 128;
const MAX_DOCUMENT_CHUNKS: usize = 100_000;
const MAX_VECTOR_BYTES: usize = 256 * 1024 * 1024;
const MAX_PURGE_ROWS: usize = 1_000_000;
const MAX_PURGE_FILES: usize = 4096;
const SPEC_KEY: &str = "prism.vector.spec.v1";
const FORMAT_KEY: &str = "prism.vector.format";
const FORMAT_VERSION: &str = "lance-f32-cosine-l2-v1";

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct VectorModelSpec {
    pub model_fingerprint: String,
    pub dimensions: usize,
    pub chunking_version: String,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ChunkEmbedding {
    pub chunk_id: String,
    pub vector: Vec<f32>,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct VectorDocumentBatch {
    pub file_path: String,
    pub fingerprint: SourceFingerprint,
    pub model_fingerprint: String,
    pub embeddings: Vec<ChunkEmbedding>,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct VectorCommitReceipt {
    pub namespace: String,
    pub version: u64,
    pub inserted: u64,
    pub updated: u64,
    pub deleted: u64,
    pub committed_after_cancel: bool,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct VectorSearchHit {
    pub chunk: ContentChunk,
    pub cosine_distance: f32,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct VectorSearchResponse {
    pub namespace: String,
    pub total_vectors: usize,
    pub hits: Vec<VectorSearchHit>,
    pub candidate_count: usize,
    pub candidate_limit: usize,
    pub filtered_out: usize,
    pub exhausted: bool,
    pub hard_limit_reached: bool,
}
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum VectorStoreErrorCode {
    InvalidInput,
    DimensionMismatch,
    ModelMismatch,
    SchemaMismatch,
    IncompleteDocument,
    OutsideScope,
    SourceChanged,
    Cancelled,
    LimitExceeded,
    Storage,
    CommitUnknown,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct VectorStoreError {
    pub code: VectorStoreErrorCode,
    pub message: String,
}
impl std::fmt::Display for VectorStoreError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message)
    }
}
impl std::error::Error for VectorStoreError {}
type Result<T> = std::result::Result<T, VectorStoreError>;
fn error(code: VectorStoreErrorCode, message: &str) -> VectorStoreError {
    VectorStoreError {
        code,
        message: message.into(),
    }
}
fn storage(_: impl std::fmt::Debug) -> VectorStoreError {
    error(
        VectorStoreErrorCode::Storage,
        "Lance vector repository operation failed",
    )
}
fn content_error(value: ContentStoreError) -> VectorStoreError {
    match value.code {
        ContentStoreErrorCode::Cancelled => error(
            VectorStoreErrorCode::Cancelled,
            "Vector repository operation cancelled",
        ),
        ContentStoreErrorCode::LimitExceeded => error(
            VectorStoreErrorCode::LimitExceeded,
            "Source chunk lookup exceeds the worker budget",
        ),
        ContentStoreErrorCode::SourceChanged => error(
            VectorStoreErrorCode::SourceChanged,
            "Source document changed during vector lookup",
        ),
        ContentStoreErrorCode::OutsideScope | ContentStoreErrorCode::UnsafePath => error(
            VectorStoreErrorCode::OutsideScope,
            "Source document is outside authorized content roots",
        ),
        _ => storage(value),
    }
}
fn commit_unknown(_: impl std::fmt::Debug) -> VectorStoreError {
    error(
        VectorStoreErrorCode::CommitUnknown,
        "Lance vector commit requires reconciliation before retry",
    )
}
fn schema_error() -> VectorStoreError {
    error(
        VectorStoreErrorCode::SchemaMismatch,
        "Lance vector repository schema does not match its model namespace",
    )
}
fn check_cancel(cancel: &AtomicBool) -> Result<()> {
    if cancel.load(Ordering::Relaxed) {
        Err(error(
            VectorStoreErrorCode::Cancelled,
            "Vector repository operation cancelled",
        ))
    } else {
        Ok(())
    }
}
async fn cancellable<F: Future>(future: F, cancel: &AtomicBool) -> Result<F::Output> {
    check_cancel(cancel)?;
    let cancelled = async {
        while !cancel.load(Ordering::Relaxed) {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    };
    futures::pin_mut!(future, cancelled);
    match futures::future::select(future, cancelled).await {
        Either::Left((value, _)) => {
            check_cancel(cancel)?;
            Ok(value)
        }
        Either::Right(_) => Err(error(
            VectorStoreErrorCode::Cancelled,
            "Vector repository operation cancelled",
        )),
    }
}

pub struct VectorStore {
    table: Table,
    namespace: String,
    model: VectorModelSpec,
    schema: Arc<Schema>,
    operation: tokio::sync::Mutex<()>,
}

fn normal_path(path: &Path) -> PathBuf {
    #[cfg(windows)]
    {
        let value = path.to_string_lossy();
        if let Some(rest) = value.strip_prefix(r"\\?\UNC\") {
            return PathBuf::from(format!(r"\\{rest}"));
        }
        if let Some(rest) = value.strip_prefix(r"\\?\") {
            return PathBuf::from(rest);
        }
    }
    path.to_owned()
}
fn key(path: &str) -> String {
    let path = normal_path(Path::new(path)).to_string_lossy().into_owned();
    #[cfg(windows)]
    {
        path.to_lowercase()
    }
    #[cfg(not(windows))]
    {
        path
    }
}
fn within(path: &Path, root: &Path) -> bool {
    let mut actual = path.components();
    for expected in root.components() {
        let Some(part) = actual.next() else {
            return false;
        };
        #[cfg(windows)]
        if part.as_os_str().to_string_lossy().to_lowercase()
            != expected.as_os_str().to_string_lossy().to_lowercase()
        {
            return false;
        }
        #[cfg(not(windows))]
        if part != expected {
            return false;
        }
    }
    true
}
fn no_links(path: &Path) -> bool {
    path.ancestors()
        .filter(|p| !p.as_os_str().is_empty())
        .all(|ancestor| {
            let Ok(meta) = fs::symlink_metadata(ancestor) else {
                return false;
            };
            if meta.file_type().is_symlink() {
                return false;
            }
            #[cfg(windows)]
            {
                use std::os::windows::fs::MetadataExt;
                if meta.file_attributes() & 0x400 != 0 {
                    return false;
                }
            }
            true
        })
}
fn current_path(path: &str, directory: bool) -> Option<PathBuf> {
    let absolute = std::path::absolute(path).ok()?;
    if !no_links(&absolute) {
        return None;
    }
    let canonical = absolute.canonicalize().ok()?;
    let meta = fs::metadata(&canonical).ok()?;
    if if directory {
        !meta.is_dir()
    } else {
        !meta.is_file()
    } {
        return None;
    }
    Some(normal_path(&canonical))
}
struct CurrentFilter {
    documents: Option<HashSet<String>>,
    scopes: Option<Vec<PathBuf>>,
    extensions: Vec<String>,
}
impl CurrentFilter {
    fn new(filter: &ContentSearchFilter) -> Result<Self> {
        if filter.document_paths.len() > 256 || filter.scope_paths.len() > 256 {
            return Err(error(
                VectorStoreErrorCode::InvalidInput,
                "Vector search filter exceeds allowed limits",
            ));
        }
        Ok(Self {
            extensions: crate::content_store::normalized_extensions(&filter.extensions).map_err(
                |_| {
                    error(
                        VectorStoreErrorCode::InvalidInput,
                        "Vector extension filter is invalid",
                    )
                },
            )?,
            documents: (!filter.document_paths.is_empty()).then(|| {
                filter
                    .document_paths
                    .iter()
                    .filter_map(|p| current_path(p, false))
                    .map(|p| key(&p.to_string_lossy()))
                    .collect()
            }),
            scopes: (!filter.scope_paths.is_empty()).then(|| {
                filter
                    .scope_paths
                    .iter()
                    .filter_map(|p| current_path(p, true))
                    .collect()
            }),
        })
    }
    fn matches(&self, chunk: &ContentChunk) -> bool {
        (self.extensions.is_empty()
            || Path::new(&chunk.file_path)
                .extension()
                .is_some_and(|extension| {
                    self.extensions
                        .iter()
                        .any(|allowed| extension.to_string_lossy().eq_ignore_ascii_case(allowed))
                }))
            && self
                .documents
                .as_ref()
                .is_none_or(|paths| paths.contains(&key(&chunk.file_path)))
            && self.scopes.as_ref().is_none_or(|scopes| {
                scopes
                    .iter()
                    .any(|scope| within(Path::new(&chunk.file_path), scope))
            })
    }
}

fn vector_schema(model: &VectorModelSpec) -> Result<Arc<Schema>> {
    if model.dimensions == 0
        || model.dimensions > MAX_DIMENSIONS
        || model.model_fingerprint.is_empty()
        || model.model_fingerprint.len() > 4096
        || model.model_fingerprint.chars().any(char::is_control)
        || model.chunking_version.is_empty()
        || model.chunking_version.len() > 128
    {
        return Err(error(
            VectorStoreErrorCode::InvalidInput,
            "Invalid vector model namespace specification",
        ));
    }
    let fields = vec![
        Field::new("chunkId", DataType::Utf8, false),
        Field::new("filePath", DataType::Utf8, false),
        Field::new("fingerprint", DataType::Utf8, false),
        Field::new("modelFingerprint", DataType::Utf8, false),
        Field::new(
            "vector",
            DataType::FixedSizeList(
                Arc::new(Field::new("item", DataType::Float32, true)),
                model.dimensions as i32,
            ),
            false,
        ),
    ];
    Ok(Arc::new(Schema::new_with_metadata(
        fields,
        HashMap::from([
            (
                SPEC_KEY.to_owned(),
                serde_json::to_string(model).map_err(storage)?,
            ),
            (FORMAT_KEY.to_owned(), FORMAT_VERSION.to_owned()),
        ]),
    )))
}
fn normalized_vector(vector: &[f32], dimensions: usize) -> Result<Vec<f32>> {
    if vector.len() != dimensions {
        return Err(error(
            VectorStoreErrorCode::DimensionMismatch,
            "Embedding vector dimensions do not match the model namespace",
        ));
    }
    if vector.iter().any(|v| !v.is_finite()) {
        return Err(error(
            VectorStoreErrorCode::InvalidInput,
            "Embedding vectors must contain only finite numbers",
        ));
    }
    let norm = vector
        .iter()
        .map(|v| (*v as f64) * (*v as f64))
        .sum::<f64>()
        .sqrt();
    if !norm.is_finite() || norm <= f64::MIN_POSITIVE {
        return Err(error(
            VectorStoreErrorCode::InvalidInput,
            "Embedding vectors must have a nonzero finite norm",
        ));
    }
    Ok(vector
        .iter()
        .map(|value| (*value as f64 / norm) as f32)
        .collect())
}
#[derive(Clone)]
struct Candidate {
    id: String,
    file_path: String,
    fingerprint: String,
    model_fingerprint: String,
    distance: f32,
}
fn candidates(batch: &RecordBatch, distance: bool) -> Result<Vec<Candidate>> {
    fn strings<'a>(batch: &'a RecordBatch, name: &str) -> Result<&'a StringArray> {
        batch
            .column_by_name(name)
            .and_then(|column| column.as_any().downcast_ref::<StringArray>())
            .ok_or_else(schema_error)
    }
    let ids = strings(batch, "chunkId")?;
    let paths = strings(batch, "filePath")?;
    let fingerprints = strings(batch, "fingerprint")?;
    let models = strings(batch, "modelFingerprint")?;
    let distances = if distance {
        Some(
            batch
                .column_by_name("_distance")
                .and_then(|column| column.as_any().downcast_ref::<Float32Array>())
                .ok_or_else(schema_error)?,
        )
    } else {
        None
    };
    let mut output = Vec::with_capacity(batch.num_rows());
    for index in 0..batch.num_rows() {
        if ids.is_null(index)
            || paths.is_null(index)
            || fingerprints.is_null(index)
            || models.is_null(index)
            || distances
                .is_some_and(|values| values.is_null(index) || !values.value(index).is_finite())
        {
            return Err(schema_error());
        }
        output.push(Candidate {
            id: ids.value(index).into(),
            file_path: paths.value(index).into(),
            fingerprint: fingerprints.value(index).into(),
            model_fingerprint: models.value(index).into(),
            distance: distances.map_or(0.0, |values| values.value(index)),
        });
    }
    Ok(output)
}

impl VectorStore {
    pub async fn open(local_directory: &Path, model: VectorModelSpec) -> Result<Self> {
        let schema = vector_schema(&model)?;
        fs::create_dir_all(local_directory).map_err(storage)?;
        let absolute = std::path::absolute(local_directory).map_err(storage)?;
        if !no_links(&absolute) {
            return Err(error(
                VectorStoreErrorCode::OutsideScope,
                "Lance storage directory is unsafe",
            ));
        }
        let canonical = normal_path(&absolute.canonicalize().map_err(storage)?);
        let uri = canonical.to_str().ok_or_else(|| {
            error(
                VectorStoreErrorCode::InvalidInput,
                "Lance storage directory is not valid Unicode",
            )
        })?;
        let mut namespace_hash = Sha256::new();
        namespace_hash.update(FORMAT_VERSION.as_bytes());
        namespace_hash.update([0]);
        namespace_hash.update(serde_json::to_vec(&model).map_err(storage)?);
        let namespace = format!("prism_vectors_{:x}", namespace_hash.finalize());
        let connection = lancedb::connect(uri)
            .read_consistency_interval(Duration::ZERO)
            .execute()
            .await
            .map_err(storage)?;
        let table = match connection.open_table(&namespace).execute().await {
            Ok(table) => table,
            Err(lancedb::Error::TableNotFound { .. }) => match connection
                .create_empty_table(&namespace, schema.clone())
                .execute()
                .await
            {
                Ok(table) => table,
                Err(lancedb::Error::TableAlreadyExists { .. }) => connection
                    .open_table(&namespace)
                    .execute()
                    .await
                    .map_err(storage)?,
                Err(error) => return Err(storage(error)),
            },
            Err(error) => return Err(storage(error)),
        };
        let stored = table.schema().await.map_err(storage)?;
        if stored.fields() != schema.fields()
            || stored.metadata().get(SPEC_KEY) != schema.metadata().get(SPEC_KEY)
            || stored.metadata().get(FORMAT_KEY) != schema.metadata().get(FORMAT_KEY)
        {
            return Err(schema_error());
        }
        Ok(Self {
            table,
            namespace,
            model,
            schema,
            operation: tokio::sync::Mutex::new(()),
        })
    }

    pub fn namespace(&self) -> &str {
        &self.namespace
    }

    /// Check persisted completeness without reading vectors or rerunning the
    /// embedding model. Caller IDs are cross-checked with current SQLite state.
    pub async fn has_current_document(
        &self,
        path: &Path,
        fingerprint: &SourceFingerprint,
        full_chunk_ids: &[String],
        content: &ContentStore,
        allowed_roots: &[PathBuf],
        cancel: &AtomicBool,
    ) -> Result<bool> {
        check_cancel(cancel)?;
        if full_chunk_ids.len() > MAX_DOCUMENT_CHUNKS
            || full_chunk_ids.iter().any(|id| id.len() > 128)
        {
            return Err(error(
                VectorStoreErrorCode::InvalidInput,
                "Document vector completeness request exceeds allowed limits",
            ));
        }
        let _operation = cancellable(self.operation.lock(), cancel).await?;
        let Some(summary) = content
            .document_summary(path, allowed_roots, cancel)
            .map_err(content_error)?
        else {
            return Ok(false);
        };
        if summary.fingerprint != *fingerprint || summary.chunk_count != full_chunk_ids.len() {
            return Ok(false);
        }
        let expected: HashSet<&str> = full_chunk_ids.iter().map(String::as_str).collect();
        if expected.len() != full_chunk_ids.len() {
            return Ok(false);
        }
        for ids in full_chunk_ids.chunks(256) {
            let current = content
                .chunks_by_ids(ids, allowed_roots, cancel)
                .map_err(content_error)?;
            if current.len() != ids.len()
                || current.iter().any(|chunk| {
                    chunk.fingerprint != *fingerprint
                        || key(&chunk.file_path) != key(&summary.file_path)
                })
            {
                return Ok(false);
            }
        }
        let query = self
            .table
            .query()
            .only_if_expr(col("filePath").eq(lit(summary.file_path.clone())))
            .select(Select::columns(&[
                "chunkId",
                "filePath",
                "fingerprint",
                "modelFingerprint",
            ]))
            .limit(MAX_DOCUMENT_CHUNKS + 1);
        let mut stream = cancellable(query.execute(), cancel)
            .await?
            .map_err(storage)?;
        let mut found = HashSet::new();
        let mut count = 0usize;
        while let Some(batch) = cancellable(stream.try_next(), cancel)
            .await?
            .map_err(storage)?
        {
            for row in candidates(&batch, false)? {
                count += 1;
                if count > expected.len()
                    || row.fingerprint != fingerprint.sha256
                    || row.model_fingerprint != self.model.model_fingerprint
                    || key(&row.file_path) != key(&summary.file_path)
                    || !expected.contains(row.id.as_str())
                    || !found.insert(row.id)
                {
                    return Ok(false);
                }
            }
        }
        if found.len() != expected.len() {
            return Ok(false);
        }
        Ok(content
            .document_summary(path, allowed_roots, cancel)
            .map_err(content_error)?
            .is_some_and(|current| {
                current.fingerprint == *fingerprint && current.chunk_count == expected.len()
            }))
    }

    pub async fn replace_document(
        &self,
        batch: VectorDocumentBatch,
        content: &ContentStore,
        allowed_roots: &[PathBuf],
        cancel: &AtomicBool,
    ) -> Result<VectorCommitReceipt> {
        check_cancel(cancel)?;
        if batch.model_fingerprint != self.model.model_fingerprint {
            return Err(error(
                VectorStoreErrorCode::ModelMismatch,
                "Embedding batch uses a different model fingerprint",
            ));
        }
        if batch.embeddings.len() > MAX_DOCUMENT_CHUNKS
            || batch
                .embeddings
                .len()
                .checked_mul(self.model.dimensions)
                .and_then(|v| v.checked_mul(4))
                .is_none_or(|bytes| bytes > MAX_VECTOR_BYTES)
        {
            return Err(error(
                VectorStoreErrorCode::LimitExceeded,
                "Embedding batch exceeds the vector worker budget",
            ));
        }
        let _operation = cancellable(self.operation.lock(), cancel).await?;
        let chunks = content
            .chunks_for_document(Path::new(&batch.file_path), allowed_roots, cancel)
            .map_err(content_error)?;
        if chunks.is_empty() {
            return Err(error(VectorStoreErrorCode::OutsideScope, "Source has no current authorized chunks; use explicit document purge for empty sources"));
        }
        if chunks
            .iter()
            .any(|chunk| chunk.fingerprint != batch.fingerprint)
        {
            return Err(error(
                VectorStoreErrorCode::SourceChanged,
                "Source fingerprint changed before vector indexing",
            ));
        }
        if chunks.len() != batch.embeddings.len() {
            return Err(error(
                VectorStoreErrorCode::IncompleteDocument,
                "Embedding batch must contain the complete source chunk set",
            ));
        }
        let expected: HashMap<&str, &ContentChunk> = chunks
            .iter()
            .map(|chunk| (chunk.id.as_str(), chunk))
            .collect();
        let mut seen = HashSet::new();
        let mut ids = Vec::new();
        let mut paths = Vec::new();
        let mut fingerprints = Vec::new();
        let mut models = Vec::new();
        let mut vectors = Vec::new();
        for embedding in &batch.embeddings {
            check_cancel(cancel)?;
            let chunk = expected
                .get(embedding.chunk_id.as_str())
                .filter(|_| seen.insert(embedding.chunk_id.as_str()))
                .ok_or_else(|| {
                    error(
                        VectorStoreErrorCode::IncompleteDocument,
                        "Embedding batch contains duplicate or unrelated chunk IDs",
                    )
                })?;
            ids.push(chunk.id.clone());
            paths.push(chunk.file_path.clone());
            fingerprints.push(chunk.fingerprint.sha256.clone());
            models.push(self.model.model_fingerprint.clone());
            vectors.push(normalized_vector(&embedding.vector, self.model.dimensions)?);
        }
        let data = RecordBatch::try_new(
            self.schema.clone(),
            vec![
                Arc::new(StringArray::from(ids)),
                Arc::new(StringArray::from(paths)),
                Arc::new(StringArray::from(fingerprints)),
                Arc::new(StringArray::from(models)),
                Arc::new(
                    FixedSizeListArray::from_iter_primitive::<Float32Type, _, _>(
                        vectors
                            .into_iter()
                            .map(|vector| Some(vector.into_iter().map(Some).collect::<Vec<_>>())),
                        self.model.dimensions as i32,
                    ),
                ),
            ],
        )
        .map_err(storage)?;
        let current = content
            .chunks_for_document(Path::new(&batch.file_path), allowed_roots, cancel)
            .map_err(content_error)?;
        if current.len() != chunks.len()
            || current
                .iter()
                .zip(&chunks)
                .any(|(a, b)| a.id != b.id || a.fingerprint != b.fingerprint)
        {
            return Err(error(
                VectorStoreErrorCode::SourceChanged,
                "Source changed while preparing a complete vector batch",
            ));
        }
        check_cancel(cancel)?;
        let source_path = &chunks[0].file_path;
        let mut merge = self.table.merge_insert(&["chunkId"]);
        merge
            .when_matched_update_all(None)
            .when_not_matched_insert_all()
            .when_not_matched_by_source_delete_expr(col("filePath").eq(lit(source_path.as_str())))
            .timeout(Duration::from_secs(120))
            .use_rte(false);
        // This is the commit boundary. Do not cancel/drop the SDK write future.
        let result = merge
            .execute(Box::new(RecordBatchIterator::new(
                [Ok(data)],
                self.schema.clone(),
            )))
            .await
            .map_err(commit_unknown)?;
        Ok(VectorCommitReceipt {
            namespace: self.namespace.clone(),
            version: result.version,
            inserted: result.num_inserted_rows,
            updated: result.num_updated_rows,
            deleted: result.num_deleted_rows,
            committed_after_cancel: cancel.load(Ordering::Relaxed),
        })
    }

    pub async fn search(
        &self,
        query_vector: &[f32],
        model_fingerprint: &str,
        filter: &ContentSearchFilter,
        content: &ContentStore,
        allowed_roots: &[PathBuf],
        top_k: usize,
        max_candidates: usize,
        cancel: &AtomicBool,
    ) -> Result<VectorSearchResponse> {
        check_cancel(cancel)?;
        if model_fingerprint != self.model.model_fingerprint {
            return Err(error(
                VectorStoreErrorCode::ModelMismatch,
                "Query embedding uses a different model fingerprint",
            ));
        }
        if !(1..=MAX_TOP_K).contains(&top_k)
            || max_candidates < top_k
            || max_candidates > MAX_CANDIDATES
        {
            return Err(error(
                VectorStoreErrorCode::InvalidInput,
                "Vector search result or candidate limits are invalid",
            ));
        }
        let query_vector = normalized_vector(query_vector, self.model.dimensions)?;
        let filter = CurrentFilter::new(filter)?;
        let _operation = cancellable(self.operation.lock(), cancel).await?;
        cancellable(self.table.checkout_latest(), cancel)
            .await?
            .map_err(storage)?;
        let total = cancellable(self.table.count_rows(None), cancel)
            .await?
            .map_err(storage)?;
        let mut response = VectorSearchResponse {
            namespace: self.namespace.clone(),
            total_vectors: total,
            hits: Vec::new(),
            candidate_count: 0,
            candidate_limit: max_candidates,
            filtered_out: 0,
            exhausted: total == 0,
            hard_limit_reached: false,
        };
        if total == 0 || allowed_roots.is_empty() {
            return Ok(response);
        }
        let cap = max_candidates.min(total);
        let mut requested = (top_k.saturating_mul(4)).max(32).min(cap);
        loop {
            check_cancel(cancel)?;
            let query = self
                .table
                .vector_search(query_vector.as_slice())
                .map_err(storage)?
                .distance_type(DistanceType::Cosine)
                .bypass_vector_index()
                .limit(requested)
                .select(Select::columns(&[
                    "chunkId",
                    "filePath",
                    "fingerprint",
                    "modelFingerprint",
                ]));
            let mut options = QueryExecutionOptions::default();
            options.timeout = Some(Duration::from_secs(30));
            let stream = cancellable(query.execute_with_options(options), cancel)
                .await?
                .map_err(storage)?;
            let batches = cancellable(stream.try_collect::<Vec<_>>(), cancel)
                .await?
                .map_err(storage)?;
            let mut rows = Vec::new();
            for batch in batches {
                check_cancel(cancel)?;
                rows.extend(candidates(&batch, true)?);
            }
            response.candidate_count = rows.len();
            response.exhausted = rows.len() < requested || requested >= total;
            let mut authorized = HashMap::new();
            for batch in rows.chunks(256) {
                check_cancel(cancel)?;
                let ids: Vec<String> = batch.iter().map(|row| row.id.clone()).collect();
                for chunk in content
                    .chunks_by_ids(&ids, allowed_roots, cancel)
                    .map_err(content_error)?
                {
                    authorized.insert(chunk.id.clone(), chunk);
                }
            }
            response.hits.clear();
            response.filtered_out = 0;
            let mut seen = HashSet::new();
            for row in rows {
                let chunk = authorized.get(&row.id);
                if row.model_fingerprint != self.model.model_fingerprint
                    || !seen.insert(row.id.clone())
                    || chunk.is_none_or(|chunk| {
                        chunk.fingerprint.sha256 != row.fingerprint
                            || key(&chunk.file_path) != key(&row.file_path)
                            || !filter.matches(chunk)
                    })
                {
                    response.filtered_out += 1;
                    continue;
                }
                response.hits.push(VectorSearchHit {
                    chunk: chunk.expect("validated chunk").clone(),
                    cosine_distance: row.distance,
                });
            }
            response.hits.sort_by(|a, b| {
                a.cosine_distance
                    .total_cmp(&b.cosine_distance)
                    .then_with(|| a.chunk.id.cmp(&b.chunk.id))
            });
            response.hits.truncate(top_k);
            if response.hits.len() >= top_k || response.exhausted || requested == cap {
                response.hard_limit_reached = !response.exhausted && requested == cap;
                check_cancel(cancel)?;
                return Ok(response);
            }
            requested = requested.saturating_mul(2).min(cap);
        }
    }

    pub async fn purge_document(
        &self,
        file_path: &Path,
        cancel: &AtomicBool,
    ) -> Result<VectorCommitReceipt> {
        check_cancel(cancel)?;
        let path = normal_path(&std::path::absolute(normal_path(file_path)).map_err(storage)?);
        let path = path.to_str().ok_or_else(|| {
            error(
                VectorStoreErrorCode::InvalidInput,
                "Source path is not valid Unicode",
            )
        })?;
        let _operation = cancellable(self.operation.lock(), cancel).await?;
        #[cfg(windows)]
        let expression = lancedb::expr::lower(col("filePath")).eq(lit(key(path)));
        #[cfg(not(windows))]
        let expression = col("filePath").eq(lit(path));
        check_cancel(cancel)?;
        let result = self
            .table
            .delete(&expression)
            .await
            .map_err(commit_unknown)?;
        Ok(VectorCommitReceipt {
            namespace: self.namespace.clone(),
            version: result.version,
            inserted: 0,
            updated: 0,
            deleted: result.num_deleted_rows,
            committed_after_cancel: cancel.load(Ordering::Relaxed),
        })
    }

    pub async fn purge_outside_roots(
        &self,
        content: &ContentStore,
        allowed_roots: &[PathBuf],
        cancel: &AtomicBool,
    ) -> Result<VectorCommitReceipt> {
        check_cancel(cancel)?;
        let _operation = cancellable(self.operation.lock(), cancel).await?;
        let query = self
            .table
            .query()
            .select(Select::columns(&[
                "chunkId",
                "filePath",
                "fingerprint",
                "modelFingerprint",
            ]))
            .limit(MAX_PURGE_ROWS + 1);
        let mut stream = cancellable(query.execute(), cancel)
            .await?
            .map_err(storage)?;
        let mut count = 0usize;
        let mut remove_files = HashSet::new();
        while let Some(batch) = cancellable(stream.try_next(), cancel)
            .await?
            .map_err(storage)?
        {
            count += batch.num_rows();
            if count > MAX_PURGE_ROWS {
                return Err(error(
                    VectorStoreErrorCode::LimitExceeded,
                    "Vector purge scan exceeds the worker budget",
                ));
            }
            for batch in candidates(&batch, false)?.chunks(256) {
                let ids: Vec<_> = batch.iter().map(|row| row.id.clone()).collect();
                let authorized: HashMap<_, _> = content
                    .chunks_by_ids(&ids, allowed_roots, cancel)
                    .map_err(content_error)?
                    .into_iter()
                    .map(|chunk| (chunk.id.clone(), chunk))
                    .collect();
                for row in batch {
                    if row.model_fingerprint != self.model.model_fingerprint
                        || authorized.get(&row.id).is_none_or(|chunk| {
                            chunk.fingerprint.sha256 != row.fingerprint
                                || key(&chunk.file_path) != key(&row.file_path)
                        })
                    {
                        remove_files.insert(row.file_path.clone());
                    }
                    if remove_files.len() > MAX_PURGE_FILES {
                        return Err(error(
                            VectorStoreErrorCode::LimitExceeded,
                            "Vector purge file set exceeds the worker budget",
                        ));
                    }
                }
            }
        }
        check_cancel(cancel)?;
        if remove_files.is_empty() {
            return Ok(VectorCommitReceipt {
                namespace: self.namespace.clone(),
                version: self.table.version().await.map_err(storage)?,
                inserted: 0,
                updated: 0,
                deleted: 0,
                committed_after_cancel: false,
            });
        }
        let expression = is_in(col("filePath"), remove_files.into_iter().map(lit).collect());
        let result = self
            .table
            .delete(&expression)
            .await
            .map_err(commit_unknown)?;
        Ok(VectorCommitReceipt {
            namespace: self.namespace.clone(),
            version: result.version,
            inserted: 0,
            updated: 0,
            deleted: result.num_deleted_rows,
            committed_after_cancel: cancel.load(Ordering::Relaxed),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::document_extract::{extract_document, ExtractOptions};

    fn cancel() -> AtomicBool {
        AtomicBool::new(false)
    }
    fn model(dimensions: usize) -> VectorModelSpec {
        VectorModelSpec {
            model_fingerprint: format!("synthetic-storage-fixture:{dimensions}"),
            dimensions,
            chunking_version: "content-chunk-v1:1200:150".into(),
        }
    }
    fn runtime<F: Future>(future: F) -> F::Output {
        tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .enable_all()
            .build()
            .unwrap()
            .block_on(future)
    }
    fn source(
        directory: &Path,
        name: &str,
        text: &str,
        content: &ContentStore,
        roots: &[PathBuf],
    ) -> (ExtractedDocument, Vec<ContentChunk>) {
        let path = directory.join(name);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).unwrap();
        }
        fs::write(&path, text).unwrap();
        let document = extract_document(&path, &ExtractOptions::default(), &cancel()).unwrap();
        content
            .replace_document(&document, roots, &cancel())
            .unwrap();
        let chunks = content
            .chunks_for_document(&path, roots, &cancel())
            .unwrap();
        (document, chunks)
    }
    use crate::document_extract::ExtractedDocument;
    fn batch(
        document: &ExtractedDocument,
        chunks: &[ContentChunk],
        fingerprint: &str,
        vector: Vec<f32>,
    ) -> VectorDocumentBatch {
        VectorDocumentBatch {
            file_path: document.file_path.clone(),
            fingerprint: document.fingerprint.clone(),
            model_fingerprint: fingerprint.into(),
            embeddings: chunks
                .iter()
                .map(|chunk| ChunkEmbedding {
                    chunk_id: chunk.id.clone(),
                    vector: vector.clone(),
                })
                .collect(),
        }
    }

    #[test]
    fn extension_filters_validate_and_intersect_with_vector_results() {
        let filter = CurrentFilter::new(&ContentSearchFilter {
            extensions: vec![".MD".into()],
            ..Default::default()
        })
        .unwrap();
        let mut chunk = ContentChunk {
            id: "fixture".into(),
            file_path: "Guide.md".into(),
            fingerprint: SourceFingerprint {
                sha256: "0".repeat(64),
                size_bytes: 0,
                modified_unix_ms: None,
            },
            section_index: 0,
            chunk_index: 0,
            text: String::new(),
            page: None,
            paragraph: None,
            heading: None,
            char_start: 0,
            char_end: 0,
        };
        assert!(filter.matches(&chunk));
        chunk.file_path = "Guide.pdf".into();
        assert!(!filter.matches(&chunk));
        assert!(CurrentFilter::new(&ContentSearchFilter {
            extensions: vec!["%".into()],
            ..Default::default()
        })
        .is_err());
    }

    #[test]
    fn vector_validation_rejects_dimensions_nonfinite_and_zero() {
        assert_eq!(
            normalized_vector(&[1.0], 2).unwrap_err().code,
            VectorStoreErrorCode::DimensionMismatch
        );
        for vector in [[0.0, 0.0], [f32::NAN, 1.0], [f32::INFINITY, 1.0]] {
            assert_eq!(
                normalized_vector(&vector, 2).unwrap_err().code,
                VectorStoreErrorCode::InvalidInput
            );
        }
        assert_eq!(normalized_vector(&[3.0, 4.0], 2).unwrap(), vec![0.6, 0.8]);
    }

    #[test]
    fn native_lance_reopen_replace_purge_and_namespace_isolation() {
        runtime(async {
            let dir = tempfile::tempdir().unwrap();
            let root = dir.path().join("sources");
            let roots = vec![root.clone()];
            let content = ContentStore::open(&dir.path().join("content.sqlite")).unwrap();
            let (first, first_chunks) = source(
                &root,
                "첫째's.txt",
                "first document\nsecond paragraph",
                &content,
                &roots,
            );
            let (second, second_chunks) =
                source(&root, "second.txt", "other document", &content, &roots);
            let spec = model(2);
            let vector_dir = dir.path().join("vectors");
            let store = VectorStore::open(&vector_dir, spec.clone()).await.unwrap();
            let first_ids: Vec<String> =
                first_chunks.iter().map(|chunk| chunk.id.clone()).collect();
            store
                .replace_document(
                    batch(
                        &first,
                        &first_chunks,
                        &spec.model_fingerprint,
                        vec![1.0, 0.0],
                    ),
                    &content,
                    &roots,
                    &cancel(),
                )
                .await
                .unwrap();
            store
                .replace_document(
                    batch(
                        &second,
                        &second_chunks,
                        &spec.model_fingerprint,
                        vec![0.0, 1.0],
                    ),
                    &content,
                    &roots,
                    &cancel(),
                )
                .await
                .unwrap();
            let response = store
                .search(
                    &[1.0, 0.0],
                    &spec.model_fingerprint,
                    &ContentSearchFilter::default(),
                    &content,
                    &roots,
                    1,
                    32,
                    &cancel(),
                )
                .await
                .unwrap();
            assert_eq!(response.hits[0].chunk.fingerprint, first.fingerprint);
            assert!(response.hits[0].cosine_distance.abs() < 0.0001);
            assert!(store
                .has_current_document(
                    Path::new(&first.file_path),
                    &first.fingerprint,
                    &first_ids,
                    &content,
                    &roots,
                    &cancel()
                )
                .await
                .unwrap());
            assert!(!store
                .has_current_document(
                    Path::new(&first.file_path),
                    &first.fingerprint,
                    &first_ids[..1],
                    &content,
                    &roots,
                    &cancel()
                )
                .await
                .unwrap());
            let namespace = store.namespace().to_owned();
            drop(store);
            let store = VectorStore::open(&vector_dir, spec.clone()).await.unwrap();
            assert_eq!(store.namespace(), namespace);
            assert!(store
                .has_current_document(
                    Path::new(&first.file_path),
                    &first.fingerprint,
                    &first_ids,
                    &content,
                    &roots,
                    &cancel()
                )
                .await
                .unwrap());
            let (updated, updated_chunks) =
                source(&root, "첫째's.txt", "replacement content", &content, &roots);
            let updated_ids: Vec<String> = updated_chunks
                .iter()
                .map(|chunk| chunk.id.clone())
                .collect();
            assert!(!store
                .has_current_document(
                    Path::new(&updated.file_path),
                    &updated.fingerprint,
                    &updated_ids,
                    &content,
                    &roots,
                    &cancel()
                )
                .await
                .unwrap());
            store
                .replace_document(
                    batch(
                        &updated,
                        &updated_chunks,
                        &spec.model_fingerprint,
                        vec![1.0, 0.0],
                    ),
                    &content,
                    &roots,
                    &cancel(),
                )
                .await
                .unwrap();
            assert_eq!(store.table.count_rows(None).await.unwrap(), 2);
            assert!(store
                .has_current_document(
                    Path::new(&updated.file_path),
                    &updated.fingerprint,
                    &updated_ids,
                    &content,
                    &roots,
                    &cancel()
                )
                .await
                .unwrap());
            let mut changed = spec.clone();
            changed.model_fingerprint.push_str(":prefix-change");
            let separate = VectorStore::open(&vector_dir, changed).await.unwrap();
            assert_ne!(separate.namespace(), store.namespace());
            assert_eq!(separate.table.count_rows(None).await.unwrap(), 0);
            let receipt = store
                .purge_document(Path::new(&first.file_path), &cancel())
                .await
                .unwrap();
            assert_eq!(receipt.deleted, 1);
            assert!(!store
                .has_current_document(
                    Path::new(&updated.file_path),
                    &updated.fingerprint,
                    &updated_ids,
                    &content,
                    &roots,
                    &cancel()
                )
                .await
                .unwrap());
            assert_eq!(store.table.count_rows(None).await.unwrap(), 1);
            drop(store);
            let store = VectorStore::open(&vector_dir, spec).await.unwrap();
            assert_eq!(store.table.count_rows(None).await.unwrap(), 1);
        });
    }

    #[test]
    fn incomplete_batches_and_invalid_queries_never_publish_partial_vectors() {
        runtime(async {
            let dir = tempfile::tempdir().unwrap();
            let root = dir.path().join("sources");
            let roots = vec![root.clone()];
            let content = ContentStore::open(&dir.path().join("content.sqlite")).unwrap();
            let (document, chunks) = source(
                &root,
                "source.txt",
                "first line\nsecond line",
                &content,
                &roots,
            );
            let spec = model(2);
            let store = VectorStore::open(&dir.path().join("vectors"), spec.clone())
                .await
                .unwrap();
            let mut partial = batch(&document, &chunks, &spec.model_fingerprint, vec![1.0, 0.0]);
            partial.embeddings.pop();
            assert_eq!(
                store
                    .replace_document(partial, &content, &roots, &cancel())
                    .await
                    .unwrap_err()
                    .code,
                VectorStoreErrorCode::IncompleteDocument
            );
            let mut duplicate = batch(&document, &chunks, &spec.model_fingerprint, vec![1.0, 0.0]);
            duplicate.embeddings[1].chunk_id = duplicate.embeddings[0].chunk_id.clone();
            assert_eq!(
                store
                    .replace_document(duplicate, &content, &roots, &cancel())
                    .await
                    .unwrap_err()
                    .code,
                VectorStoreErrorCode::IncompleteDocument
            );
            assert_eq!(store.table.count_rows(None).await.unwrap(), 0);
            assert_eq!(
                store
                    .search(
                        &[1.0, 0.0, 0.0],
                        &spec.model_fingerprint,
                        &ContentSearchFilter::default(),
                        &content,
                        &roots,
                        1,
                        32,
                        &cancel()
                    )
                    .await
                    .unwrap_err()
                    .code,
                VectorStoreErrorCode::DimensionMismatch
            );
            assert_eq!(
                store
                    .search(
                        &[1.0, 0.0],
                        "wrong-model",
                        &ContentSearchFilter::default(),
                        &content,
                        &roots,
                        1,
                        32,
                        &cancel()
                    )
                    .await
                    .unwrap_err()
                    .code,
                VectorStoreErrorCode::ModelMismatch
            );
            let cancelled = AtomicBool::new(true);
            assert_eq!(
                store
                    .replace_document(
                        batch(&document, &chunks, &spec.model_fingerprint, vec![1.0, 0.0]),
                        &content,
                        &roots,
                        &cancelled
                    )
                    .await
                    .unwrap_err()
                    .code,
                VectorStoreErrorCode::Cancelled
            );
            assert_eq!(store.table.count_rows(None).await.unwrap(), 0);
        });
    }

    #[test]
    fn authorization_overfetch_diagnostics_and_orphan_purge() {
        runtime(async {
            let dir = tempfile::tempdir().unwrap();
            let allowed = dir.path().join("allowed");
            let outside = dir.path().join("outside");
            let all = vec![dir.path().to_owned()];
            let roots = vec![allowed.clone()];
            let content = ContentStore::open(&dir.path().join("content.sqlite")).unwrap();
            let outside_text = (0..40)
                .map(|index| format!("outside fixture {index}"))
                .collect::<Vec<_>>()
                .join("\n");
            let (denied, denied_chunks) =
                source(&outside, "outside.txt", &outside_text, &content, &all);
            let (permitted, permitted_chunks) =
                source(&allowed, "allowed.txt", "allowed paragraph", &content, &all);
            let spec = model(2);
            let store = VectorStore::open(&dir.path().join("vectors"), spec.clone())
                .await
                .unwrap();
            store
                .replace_document(
                    batch(
                        &denied,
                        &denied_chunks,
                        &spec.model_fingerprint,
                        vec![1.0, 0.0],
                    ),
                    &content,
                    &all,
                    &cancel(),
                )
                .await
                .unwrap();
            store
                .replace_document(
                    batch(
                        &permitted,
                        &permitted_chunks,
                        &spec.model_fingerprint,
                        vec![0.8, 0.6],
                    ),
                    &content,
                    &all,
                    &cancel(),
                )
                .await
                .unwrap();
            let limited = store
                .search(
                    &[1.0, 0.0],
                    &spec.model_fingerprint,
                    &ContentSearchFilter::default(),
                    &content,
                    &roots,
                    1,
                    1,
                    &cancel(),
                )
                .await
                .unwrap();
            assert!(limited.hits.is_empty());
            assert!(limited.hard_limit_reached);
            assert!(!limited.exhausted);
            assert_eq!(limited.filtered_out, 1);
            let expanded = store
                .search(
                    &[1.0, 0.0],
                    &spec.model_fingerprint,
                    &ContentSearchFilter::default(),
                    &content,
                    &roots,
                    1,
                    64,
                    &cancel(),
                )
                .await
                .unwrap();
            assert_eq!(expanded.hits.len(), 1);
            assert_eq!(expanded.candidate_count, 41);
            assert_eq!(expanded.filtered_out, 40);
            assert!(expanded.exhausted);
            assert!(!expanded.hard_limit_reached);
            assert_eq!(
                store
                    .purge_outside_roots(&content, &roots, &cancel())
                    .await
                    .unwrap()
                    .deleted,
                40
            );
            fs::remove_file(&permitted.file_path).unwrap();
            assert!(store
                .search(
                    &[1.0, 0.0],
                    &spec.model_fingerprint,
                    &ContentSearchFilter::default(),
                    &content,
                    &roots,
                    1,
                    3,
                    &cancel()
                )
                .await
                .unwrap()
                .hits
                .is_empty());
            assert_eq!(
                store
                    .purge_outside_roots(&content, &roots, &cancel())
                    .await
                    .unwrap()
                    .deleted,
                1
            );
        });
    }

    #[test]
    #[ignore = "Requires PRISM_TEST_MODEL and PRISM_TEST_RUNTIME pinned actual EmbeddingGemma 2 assets"]
    fn actual_korean_embedding_persists_and_retrieves_from_lance() {
        use crate::embedding_runtime::{EmbeddingDocument, EmbeddingRuntime};
        let model_asset =
            PathBuf::from(std::env::var_os("PRISM_TEST_MODEL").expect("PRISM_TEST_MODEL"));
        let runtime_asset =
            PathBuf::from(std::env::var_os("PRISM_TEST_RUNTIME").expect("PRISM_TEST_RUNTIME"));
        let dir = tempfile::tempdir().unwrap();
        let service = EmbeddingRuntime::new(dir.path().join("runtime")).unwrap();
        service.import_model(&model_asset).unwrap();
        service.import_runtime(&runtime_asset).unwrap();
        service.start().unwrap();
        let documents = [
            EmbeddingDocument { title: Some("제주도 여름휴가 계획".into()), text: "8월 가족 여행으로 제주도에 갑니다. 항공권은 금요일 아침이고 숙소는 서귀포입니다.".into() },
            EmbeddingDocument { title: Some("회의록".into()), text: "다음 분기 데이터베이스 마이그레이션과 매출 보고서 자동화 작업을 검토했습니다.".into() },
            EmbeddingDocument { title: Some("김치찌개 조리법".into()), text: "돼지고기와 잘 익은 김치를 볶은 뒤 물과 두부를 넣고 끓입니다.".into() },
        ];
        let vectors = service.embed_documents(&documents, 768).unwrap();
        let query = service
            .embed_query("여름휴가 때 제주도에 가는 일정은 어디에 있어?", 768)
            .unwrap();
        let fingerprint = service.fingerprint(768).unwrap();
        runtime(async {
            let root = dir.path().join("sources");
            let roots = vec![root.clone()];
            let content = ContentStore::open(&dir.path().join("content.sqlite")).unwrap();
            let spec = VectorModelSpec {
                model_fingerprint: fingerprint.clone(),
                dimensions: 768,
                chunking_version: "content-chunk-v1:1200:150".into(),
            };
            let vector_dir = dir.path().join("vectors");
            let store = VectorStore::open(&vector_dir, spec.clone()).await.unwrap();
            let mut expected = String::new();
            for (index, (document, vector)) in documents.iter().zip(vectors).enumerate() {
                let (source, chunks) = source(
                    &root,
                    &format!("fixture-{index}.txt"),
                    &document.text,
                    &content,
                    &roots,
                );
                if index == 0 {
                    expected = chunks[0].id.clone();
                }
                store
                    .replace_document(
                        batch(&source, &chunks, &fingerprint, vector),
                        &content,
                        &roots,
                        &cancel(),
                    )
                    .await
                    .unwrap();
            }
            drop(store);
            let store = VectorStore::open(&vector_dir, spec).await.unwrap();
            let hits = store
                .search(
                    &query,
                    &fingerprint,
                    &ContentSearchFilter::default(),
                    &content,
                    &roots,
                    3,
                    3,
                    &cancel(),
                )
                .await
                .unwrap();
            assert_eq!(hits.hits.len(), 3);
            assert_eq!(hits.hits[0].chunk.id, expected);
        });
        service.stop().unwrap();
    }
}
