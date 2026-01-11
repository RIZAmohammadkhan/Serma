use anyhow::Context;
use std::cmp::Ordering as CmpOrdering;
use std::path::Path;
use std::sync::Arc;
use std::sync::Mutex;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, Instant};
use tantivy::IndexSettings;
use tantivy::ReloadPolicy;
use tantivy::collector::TopDocs;
use tantivy::query::{BooleanQuery, FuzzyTermQuery, Occur, Query, QueryParser, RegexQuery, TermQuery};
use tantivy::schema::{FAST, Field, IndexRecordOption, STORED, STRING, Schema, TEXT, Value};
use tantivy::{Score, Term};

#[derive(Clone)]
pub struct SearchIndex {
    inner: Arc<SearchIndexInner>,
}

struct SearchIndexInner {
    index: tantivy::Index,
    reader: tantivy::IndexReader,
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
        let (index, info_hash, title, magnet, seeders) = match tantivy::Index::open(dir.clone()) {
            Ok(index) => {
                let schema = index.schema();
                let info_hash = schema.get_field("info_hash").ok();
                let title = schema.get_field("title").ok();
                let magnet = schema.get_field("magnet").ok();
                let seeders = schema.get_field("seeders").ok();

                if let (Some(info_hash), Some(title), Some(magnet), Some(seeders)) =
                    (info_hash, title, magnet, seeders)
                {
                    (index, info_hash, title, magnet, seeders)
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
                    (index, info_hash, title, magnet, seeders)
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
                (index, info_hash, title, magnet, seeders)
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
        self.search_page(q, 0, limit)
    }

    pub fn search_page(&self, q: &str, offset: usize, limit: usize) -> anyhow::Result<Vec<SearchHit>> {
        let q = q.trim();
        if q.is_empty() || limit == 0 {
            return Ok(Vec::new());
        }

        let requested = offset.saturating_add(limit);
        if requested == 0 {
            return Ok(Vec::new());
        }

        // Ensure we see recent commits.
        self.inner.reader.reload().ok();
        let searcher = self.inner.reader.searcher();

        let strict_query = self.build_query(q, QueryMode::Strict)?;
        let mut scored_docs = self.search_and_score(&searcher, strict_query.as_ref(), requested)?;

        // If the strict parse yields nothing, fall back to a typo-tolerant query.
        if scored_docs.is_empty() {
            let fuzzy_query = self.build_query(q, QueryMode::FuzzyFallback)?;
            scored_docs = self.search_and_score(&searcher, fuzzy_query.as_ref(), requested)?;
        }

        Ok(scored_docs.into_iter().skip(offset).take(limit).collect())
    }

    fn search_and_score(
        &self,
        searcher: &tantivy::Searcher,
        query: &dyn Query,
        limit: usize,
    ) -> anyhow::Result<Vec<SearchHit>> {
        // Pull more candidates than we ultimately return, so we can re-rank
        // by a combination of textual relevance and seeders.
        let candidate_limit = (limit.saturating_mul(10)).clamp(limit, 2000);
        let top_docs = searcher.search(query, &TopDocs::with_limit(candidate_limit))?;

        let mut candidates = Vec::with_capacity(top_docs.len());
        for (bm25_score, addr) in top_docs {
            let retrieved: tantivy::schema::TantivyDocument = searcher.doc(addr)?;
            let seeders = retrieved
                .get_first(self.inner.seeders)
                .and_then(|v| v.as_i64())
                .unwrap_or(0);

            let adjusted = adjust_score(bm25_score, seeders);
            candidates.push((adjusted, seeders, retrieved));
        }

        candidates.sort_by(|(score_a, seeders_a, _), (score_b, seeders_b, _)| {
            score_b
                .partial_cmp(score_a)
                .unwrap_or(CmpOrdering::Equal)
                .then_with(|| seeders_b.cmp(seeders_a))
        });

        let mut hits = Vec::with_capacity(limit.min(candidates.len()));
        for (_score, seeders, retrieved) in candidates.into_iter().take(limit) {
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

            hits.push(SearchHit {
                info_hash,
                title,
                magnet,
                seeders,
            });
        }

        Ok(hits)
    }

    fn build_query(&self, q: &str, mode: QueryMode) -> anyhow::Result<Box<dyn Query>> {
        let q = q.trim();

        // Special-case: if the user pasted a full hash (or a long hex prefix), do the right thing.
        if let Some(hex) = normalize_hex_query(q) {
            if hex.len() == 40 {
                let term = Term::from_field_text(self.inner.info_hash, &hex);
                return Ok(Box::new(TermQuery::new(term, IndexRecordOption::Basic)));
            }
            // A shorter hex string is treated as a prefix match on the hash.
            if hex.len() >= 8 {
                let pattern = format!("^{}.*", hex);
                return Ok(Box::new(
                    RegexQuery::from_pattern(&pattern, self.inner.info_hash)
                        .context("build hash prefix query")?,
                ));
            }
        }

        match mode {
            QueryMode::Strict => self.build_strict_query(q),
            QueryMode::FuzzyFallback => self.build_fuzzy_query(q),
        }
    }

    fn build_strict_query(&self, q: &str) -> anyhow::Result<Box<dyn Query>> {
        let mut query_parser = QueryParser::for_index(
            &self.inner.index,
            vec![self.inner.title, self.inner.info_hash],
        );
        // Better default for search UX: space-separated terms behave like AND.
        query_parser.set_conjunction_by_default();
        // Prefer title matches to hash matches.
        query_parser.set_field_boost(self.inner.title, 2.0);

        if let Ok(query) = query_parser.parse_query(q) {
            return Ok(query);
        }

        // Fallback: sanitize the query (some users paste magnet params, colons, etc.).
        let sanitized = sanitize_query(q);
        if let Ok(query) = query_parser.parse_query(&sanitized) {
            return Ok(query);
        }

        // Last resort: token-based MUST queries on title.
        let tokens = self.tokenize_for_title(&sanitized);
        let mut clauses: Vec<(Occur, Box<dyn Query>)> = Vec::new();
        for token in tokens {
            let term = Term::from_field_text(self.inner.title, &token);
            clauses.push((Occur::Must, Box::new(TermQuery::new(term, IndexRecordOption::Basic))));
        }
        if clauses.is_empty() {
            anyhow::bail!("empty query")
        }
        Ok(Box::new(BooleanQuery::new(clauses)))
    }

    fn build_fuzzy_query(&self, q: &str) -> anyhow::Result<Box<dyn Query>> {
        let sanitized = sanitize_query(q);
        let tokens = self.tokenize_for_title(&sanitized);
        let mut clauses: Vec<(Occur, Box<dyn Query>)> = Vec::new();

        for token in tokens {
            // Also allow searching by hash prefixes when the query contains hex-like chunks.
            if let Some(hex) = normalize_hex_query(&token) {
                if hex.len() >= 8 {
                    let pattern = format!("^{}.*", hex);
                    let query = RegexQuery::from_pattern(&pattern, self.inner.info_hash)
                        .context("build hash prefix query")?;
                    clauses.push((Occur::Should, Box::new(query)));
                }
            }

            // Fuzzy title matching for typos.
            let term = Term::from_field_text(self.inner.title, &token);
            if token.len() <= 3 {
                clauses.push((Occur::Must, Box::new(TermQuery::new(term, IndexRecordOption::Basic))));
            } else {
                // Distance=1 keeps it reasonably precise while fixing common typos.
                clauses.push((Occur::Must, Box::new(FuzzyTermQuery::new(term, 1, true))));
            }
        }

        if clauses.is_empty() {
            anyhow::bail!("empty query")
        }
        Ok(Box::new(BooleanQuery::new(clauses)))
    }

    fn tokenize_for_title(&self, text: &str) -> Vec<String> {
        let Some(mut tokenizer) = self.inner.index.tokenizers().get("default") else {
            return text
                .split_whitespace()
                .map(|s| s.to_ascii_lowercase())
                .filter(|s| !s.is_empty())
                .collect();
        };

        let mut stream = tokenizer.token_stream(text);
        let mut out = Vec::new();
        while stream.advance() {
            let token = stream.token();
            if !token.text.is_empty() {
                out.push(token.text.to_string());
            }
        }
        out
    }
}

#[derive(Debug, Clone, Copy)]
enum QueryMode {
    Strict,
    FuzzyFallback,
}

fn adjust_score(bm25: Score, seeders: i64) -> f32 {
    // Relevance is primary; seeders is a gentle boost.
    // Using ln(1+s) avoids huge domination by very large seed counts.
    let seed_boost = ((seeders.max(0) as f32) + 1.0).ln() / 4.0;
    bm25 + seed_boost
}

fn sanitize_query(input: &str) -> String {
    // Keep quotes so users can still do phrase searches.
    // Replace common query-parser special chars with spaces.
    input
        .chars()
        .map(|c| match c {
            // Tantivy query parser operators / syntax.
            ':' | '^' | '~' | '*' | '?' | '\\' | '(' | ')' | '[' | ']' | '{' | '}' | '!' | '+'
            | '-' | '|' => ' ',
            _ => c,
        })
        .collect::<String>()
}

fn normalize_hex_query(input: &str) -> Option<String> {
    let s = input.trim();
    if s.len() < 8 || s.len() > 40 {
        return None;
    }
    if s.chars().all(|c| c.is_ascii_hexdigit()) {
        Some(s.to_ascii_lowercase())
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn temp_index_dir() -> std::path::PathBuf {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos();
        std::env::temp_dir().join(format!("serma-index-test-{}-{}", std::process::id(), nanos))
    }

    #[test]
    fn relevance_beats_seeders_sorting() {
        let dir = temp_index_dir();
        let index = SearchIndex::open_or_create(&dir).unwrap();

        // Doc with massive seeders but missing a key term.
        index
            .upsert(
                "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
                "Matrix 1080p",
                "",
                10_000,
            )
            .unwrap();

        // Doc with fewer seeders but full match.
        index
            .upsert(
                "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb",
                "The Matrix 1999 1080p",
                "",
                5,
            )
            .unwrap();

        index.maybe_commit().unwrap();

        let hits = index.search("matrix 1999", 10).unwrap();
        assert!(!hits.is_empty());
        let top_title = hits[0].title.clone().unwrap_or_default().to_ascii_lowercase();
        assert!(top_title.contains("1999"));
    }

    #[test]
    fn fuzzy_fallback_finds_typos() {
        let dir = temp_index_dir();
        let index = SearchIndex::open_or_create(&dir).unwrap();
        index
            .upsert(
                "cccccccccccccccccccccccccccccccccccccccc",
                "The Matrix 1999",
                "",
                1,
            )
            .unwrap();
        index.maybe_commit().unwrap();

        // Missing the second 'i'.
        let hits = index.search("matrx 1999", 10).unwrap();
        assert!(!hits.is_empty());
        let title = hits[0].title.clone().unwrap_or_default().to_ascii_lowercase();
        assert!(title.contains("matrix"));
    }
}
