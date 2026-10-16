use crate::db::{Database, SearchHit};
use crate::embeddings::EmbeddingGenerator;
use std::collections::HashMap;
use std::path::Path;

#[derive(Debug, Clone)]
pub struct DocumentSearchResult {
    pub document_id: i64,
    pub filename: String,
    pub file_path: String,
    pub matching_pages: Vec<i32>,
    pub snippets: Vec<String>,
    pub max_score: f32,
    pub has_semantic_matches: bool,
}

pub struct SearchEngine {
    db: Database,
    embedding_gen: Box<dyn EmbeddingGenerator>,
}

impl SearchEngine {
    pub fn new(db: Database, embedding_gen: Box<dyn EmbeddingGenerator>) -> Self {
        Self { db, embedding_gen }
    }

    pub fn search(
        &self,
        library_id: Option<i64>,
        query: &str,
        limit: usize,
    ) -> Result<Vec<DocumentSearchResult>, String> {
        let trimmed = query.trim();
        if trimmed.is_empty() {
            return Ok(Vec::new());
        }

        // 1. Text Search (FTS / Keyword)
        let text_hits = self.db.search_text(library_id, trimmed, limit * 2)?;

        // 2. Semantic Search (Embeddings)
        let semantic_hits = if let Ok(query_emb) = self.embedding_gen.generate_embedding(trimmed) {
            self.db.search_semantic(library_id, &query_emb, limit * 2).unwrap_or_default()
        } else {
            Vec::new()
        };

        // 3. Merge & Aggregate by Document
        let mut doc_map: HashMap<i64, DocumentSearchResult> = HashMap::new();

        for hit in text_hits {
            let entry = doc_map.entry(hit.document_id).or_insert_with(|| DocumentSearchResult {
                document_id: hit.document_id,
                filename: hit.filename.clone(),
                file_path: hit.file_path.clone(),
                matching_pages: Vec::new(),
                snippets: Vec::new(),
                max_score: 1.0,
                has_semantic_matches: false,
            });

            if !entry.matching_pages.contains(&hit.page_number) {
                entry.matching_pages.push(hit.page_number);
            }
            if entry.snippets.len() < 3 && !entry.snippets.contains(&hit.snippet) {
                entry.snippets.push(hit.snippet);
            }
        }

        for hit in semantic_hits {
            let entry = doc_map.entry(hit.document_id).or_insert_with(|| DocumentSearchResult {
                document_id: hit.document_id,
                filename: hit.filename.clone(),
                file_path: hit.file_path.clone(),
                matching_pages: Vec::new(),
                snippets: Vec::new(),
                max_score: hit.score,
                has_semantic_matches: true,
            });

            if hit.score > entry.max_score {
                entry.max_score = hit.score;
            }
            entry.has_semantic_matches = true;

            if !entry.matching_pages.contains(&hit.page_number) {
                entry.matching_pages.push(hit.page_number);
            }
            if entry.snippets.len() < 3 && !entry.snippets.contains(&hit.snippet) {
                entry.snippets.push(hit.snippet);
            }
        }

        let mut results: Vec<DocumentSearchResult> = doc_map.into_values().collect();
        for res in &mut results {
            res.matching_pages.sort();
        }

        results.sort_by(|a, b| {
            b.max_score
                .partial_cmp(&a.max_score)
                .unwrap_or(std::cmp::Ordering::Equal)
                .then_with(|| a.filename.cmp(&b.filename))
        });

        results.truncate(limit);
        Ok(results)
    }

    pub fn search_raw_hits(
        &self,
        library_id: Option<i64>,
        query: &str,
        limit: usize,
    ) -> Result<Vec<SearchHit>, String> {
        let trimmed = query.trim();
        if trimmed.is_empty() {
            return Ok(Vec::new());
        }

        let mut hits = self.db.search_text(library_id, trimmed, limit)?;

        if let Ok(query_emb) = self.embedding_gen.generate_embedding(trimmed) {
            if let Ok(sem_hits) = self.db.search_semantic(library_id, &query_emb, limit) {
                for sem in sem_hits {
                    if !hits.iter().any(|h| h.document_id == sem.document_id && h.page_number == sem.page_number) {
                        hits.push(sem);
                    }
                }
            }
        }

        hits.sort_by(|a, b| b.score.partial_cmp(&a.score).unwrap_or(std::cmp::Ordering::Equal));
        hits.truncate(limit);
        Ok(hits)
    }
}

pub fn open_document_at_page<P: AsRef<Path>>(file_path: P, _page_number: i32) -> Result<(), String> {
    let path = file_path.as_ref();
    if !path.exists() {
        return Err(format!("File does not exist: {}", path.display()));
    }

    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .map(|e| e.to_lowercase())
        .unwrap_or_default();

    if ext == "pdf" {
        // Try platform-specific PDF readers or browser page parameters
        #[cfg(target_os = "windows")]
        {
            // Try default opener first
            if opener::open(path).is_ok() {
                return Ok(());
            }
        }

        #[cfg(target_os = "macos")]
        {
            if std::process::Command::new("open")
                .arg(path)
                .status()
                .is_ok()
            {
                return Ok(());
            }
        }

        #[cfg(target_os = "linux")]
        {
            if std::process::Command::new("xdg-open")
                .arg(path)
                .status()
                .is_ok()
            {
                return Ok(());
            }
        }
    }

    opener::open(path).map_err(|e| format!("Failed to open file: {e}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::embeddings::DeterministicEmbeddingGenerator;

    #[test]
    fn test_search_engine_hybrid() {
        let db = Database::open_in_memory().unwrap();
        let lib = db.add_or_get_library("TestLib", "/lib").unwrap();

        let doc1 = db
            .upsert_document(lib.id, "RustGuide.pdf", "/lib/RustGuide.pdf", "pdf", 100, 1000, false)
            .unwrap();
        let doc2 = db
            .upsert_document(lib.id, "Cooking.epub", "/lib/Cooking.epub", "epub", 100, 2000, false)
            .unwrap();

        let emb_gen = DeterministicEmbeddingGenerator::new(64);
        let emb1 = emb_gen.generate_embedding("Rust memory safety and concurrency").unwrap();
        let emb2 = emb_gen.generate_embedding("Italian pasta sauce recipe with tomatoes").unwrap();

        db.insert_page(doc1, 1, "Rust is a systems programming language focusing on safety.", Some(&emb1))
            .unwrap();
        db.insert_page(doc1, 2, "Concurrency without data races in Rust.", Some(&emb1))
            .unwrap();
        db.insert_page(doc2, 1, "Boil water and add fresh tomatoes for the sauce.", Some(&emb2))
            .unwrap();

        let engine = SearchEngine::new(db, Box::new(emb_gen));

        // Exact text match for "Rust"
        let results = engine.search(Some(lib.id), "Rust", 10).unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].filename, "RustGuide.pdf");
        assert_eq!(results[0].matching_pages, vec![1, 2]);

        // Semantic query for "systems programming"
        let sem_results = engine.search(Some(lib.id), "systems programming", 10).unwrap();
        assert!(!sem_results.is_empty());
        assert_eq!(sem_results[0].filename, "RustGuide.pdf");

        // Empty query
        let empty_results = engine.search(Some(lib.id), "   ", 10).unwrap();
        assert!(empty_results.is_empty());

        // Raw hits test
        let raw_hits = engine.search_raw_hits(Some(lib.id), "tomatoes", 5).unwrap();
        assert!(!raw_hits.is_empty());
        assert!(raw_hits.iter().any(|h| h.filename == "Cooking.epub"));
    }

    #[test]
    fn test_open_nonexistent_file() {
        let res = open_document_at_page("/nonexistent/file.pdf", 1);
        assert!(res.is_err());
    }
}
