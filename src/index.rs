use anyhow::Context;
use std::path::Path;
use std::sync::Arc;
use std::sync::Mutex;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, Instant};
use tantivy::IndexSettings;
use tantivy::ReloadPolicy;
use tantivy::collector::TopDocs;
use tantivy::query::QueryParser;
use tantivy::schema::{FAST, Field, STORED, STRING, Schema, TEXT, Value};
use tantivy::{Order, Term};

#[derive(Clone)]
pub struct SearchIndex {
    inner: Arc<SearchIndexInner>,
}

struct SearchIndexInner {
    index: tantivy::Index,
    reader: tantivy::IndexReader,
    schema: Schema,
    info_hash: Field,
    title: Field,
    magnet: Field,
    seeders: Field,
    writer: Mutex<tantivy::IndexWriter>,
    pending_ops: AtomicUsize,
    last_commit_at: Mutex<Instant>,
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct SearchHit {
    pub info_hash: Option<String>,
    pub title: Option<String>,
    pub magnet: Option<String>,
    pub seeders: i64,
}

impl SearchIndex {
    pub fn open_or_create(path: impl AsRef<Path>) -> anyhow::Result<Self> {
        let mut expected_schema_builder = Schema::builder();
        expected_schema_builder.add_text_field("info_hash", STRING | STORED);
        expected_schema_builder.add_text_field("title", TEXT | STORED);
        expected_schema_builder.add_text_field("magnet", STORED);
        expected_schema_builder.add_i64_field("seeders", FAST | STORED);
        let expected_schema = expected_schema_builder.build();

        std::fs::create_dir_all(path.as_ref()).context("create index directory")?;
        let mut dir = tantivy::directory::MmapDirectory::open(path.as_ref())
            .context("open index directory")?;

        // IMPORTANT: When opening an existing Tantivy index, always use the schema
        // stored in that index for field IDs. Mixing field IDs from a newly built
        // schema with an on-disk schema can panic inside Tantivy.
        let (index, schema, info_hash, title, magnet, seeders) = match tantivy::Index::open(dir.clone()) {
            Ok(index) => {
                let schema = index.schema();
                let info_hash = schema.get_field("info_hash").ok();
                let title = schema.get_field("title").ok();
                let magnet = schema.get_field("magnet").ok();
                let seeders = schema.get_field("seeders").ok();

                if let (Some(info_hash), Some(title), Some(magnet), Some(seeders)) =
                    (info_hash, title, magnet, seeders)
                {
                    (index, schema.clone(), info_hash, title, magnet, seeders)
                } else {
                    tracing::warn!(
                        path = %path.as_ref().display(),
                        "tantivy schema mismatch; recreating index directory"
                    );
                    drop(index);

                    // Tantivy does not support in-place schema migrations.
                    // Recreate the index directory so the schema matches the binary.
                    std::fs::remove_dir_all(path.as_ref()).ok();
                    std::fs::create_dir_all(path.as_ref())
                        .context("recreate index directory")?;
                    dir = tantivy::directory::MmapDirectory::open(path.as_ref())
                        .context("reopen index directory")?;
                    let index = tantivy::Index::create(dir, expected_schema.clone(), IndexSettings::default())
                        .context("create index")?;
                    let schema = index.schema();
                    let info_hash = schema
                        .get_field("info_hash")
                        .context("missing info_hash field")?;
                    let title = schema.get_field("title").context("missing title field")?;
                    let magnet = schema
                        .get_field("magnet")
                        .context("missing magnet field")?;
                    let seeders = schema
                        .get_field("seeders")
                        .context("missing seeders field")?;
                    (index, schema.clone(), info_hash, title, magnet, seeders)
                }
            }
            Err(_) => {
                let index = tantivy::Index::create(dir, expected_schema.clone(), IndexSettings::default())
                    .context("create index")?;
                let schema = index.schema();
                let info_hash = schema
                    .get_field("info_hash")
                    .context("missing info_hash field")?;
                let title = schema.get_field("title").context("missing title field")?;
                let magnet = schema
                    .get_field("magnet")
                    .context("missing magnet field")?;
                let seeders = schema
                    .get_field("seeders")
                    .context("missing seeders field")?;
                (index, schema.clone(), info_hash, title, magnet, seeders)
            }
        };

        let reader = index
            .reader_builder()
            .reload_policy(ReloadPolicy::OnCommitWithDelay)
            .try_into()
            .context("build index reader")?;

        let writer = index.writer(200_000_000)?;

        Ok(Self {
            inner: Arc::new(SearchIndexInner {
                index,
                reader,
                schema,
                info_hash,
                title,
                magnet,
                seeders,
                writer: Mutex::new(writer),
                pending_ops: AtomicUsize::new(0),
                // Ensure the very first maybe_commit() can commit immediately.
                // Otherwise, a single ingested hash can remain uncommitted and therefore unsearchable.
                last_commit_at: Mutex::new(Instant::now() - Duration::from_secs(3600)),
            }),
        })
    }

    pub fn upsert(
        &self,
        info_hash_hex: &str,
        title: &str,
        magnet: &str,
        seeders: i64,
    ) -> anyhow::Result<()> {
        let mut writer = self
            .inner
            .writer
            .lock()
            .map_err(|_| anyhow::anyhow!("tantivy writer lock poisoned"))?;

        // Delete old documents for this info_hash (stable upsert key).
        let term = Term::from_field_text(self.inner.info_hash, info_hash_hex);
        writer.delete_term(term);

        let mut doc = tantivy::schema::TantivyDocument::default();
        doc.add_text(self.inner.info_hash, info_hash_hex);
        doc.add_text(self.inner.title, title);
        if !magnet.trim().is_empty() {
            doc.add_text(self.inner.magnet, magnet);
        }
        doc.add_i64(self.inner.seeders, seeders);

        writer.add_document(doc)?;

        let pending = self.inner.pending_ops.fetch_add(1, Ordering::Relaxed) + 1;
        if pending >= 100 {
            self.commit_locked(&mut writer)?;
        }

        Ok(())
    }

    pub fn delete(&self, info_hash_hex: &str) -> anyhow::Result<()> {
        let writer = self
            .inner
            .writer
            .lock()
            .map_err(|_| anyhow::anyhow!("tantivy writer lock poisoned"))?;

        let term = Term::from_field_text(self.inner.info_hash, info_hash_hex);
        writer.delete_term(term);

        self.inner.pending_ops.fetch_add(1, Ordering::Relaxed);
        Ok(())
    }

    pub fn maybe_commit(&self) -> anyhow::Result<()> {
        let mut writer = self
            .inner
            .writer
            .lock()
            .map_err(|_| anyhow::anyhow!("tantivy writer lock poisoned"))?;
        let pending = self.inner.pending_ops.load(Ordering::Relaxed);
        if pending == 0 {
            return Ok(());
        }

        let mut last_commit_at = self
            .inner
            .last_commit_at
            .lock()
            .map_err(|_| anyhow::anyhow!("tantivy commit lock poisoned"))?;
        if last_commit_at.elapsed() < Duration::from_secs(2) {
            return Ok(());
        }

        self.commit_locked(&mut writer)?;
        *last_commit_at = Instant::now();
        Ok(())
    }

    fn commit_locked(&self, writer: &mut tantivy::IndexWriter) -> anyhow::Result<()> {
        writer.commit()?;
        self.inner.pending_ops.store(0, Ordering::Relaxed);
        Ok(())
    }

    pub fn search(&self, q: &str, limit: usize) -> anyhow::Result<Vec<SearchHit>> {
        // Keep it simple for MVP: ensure we see recent commits.
        self.inner.reader.reload().ok();
        let searcher = self.inner.reader.searcher();

        let query_parser = QueryParser::for_index(
            &self.inner.index,
            vec![self.inner.title, self.inner.info_hash],
        );
        let query = query_parser.parse_query(q)?;

        let top_docs = searcher.search(
            &query,
            &TopDocs::with_limit(limit).order_by_fast_field::<i64>("seeders", Order::Desc),
        )?;
        let mut hits = Vec::with_capacity(top_docs.len());

        for (_score, addr) in top_docs {
            let retrieved: tantivy::schema::TantivyDocument = searcher.doc(addr)?;
            let info_hash = retrieved
                .get_first(self.inner.info_hash)
                .and_then(|v| v.as_str())
                .map(|s| s.to_string());
            let title = retrieved
                .get_first(self.inner.title)
                .and_then(|v| v.as_str())
                .map(|s| s.to_string());
            let magnet = retrieved
                .get_first(self.inner.magnet)
                .and_then(|v| v.as_str())
                .map(|s| s.to_string());
            let seeders = retrieved
                .get_first(self.inner.seeders)
                .and_then(|v| v.as_i64())
                .unwrap_or(0);

            hits.push(SearchHit {
                info_hash,
                title,
                magnet,
                seeders,
            });
        }

        Ok(hits)
    }
}
