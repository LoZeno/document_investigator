use duckdb::{params, Connection};
use serde::{Deserialize, Serialize};
use std::path::Path;
use std::sync::{Arc, Mutex};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Library {
    pub id: i64,
    pub name: String,
    pub path: String,
    pub created_at: String,
    pub last_indexed_at: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct DocumentRecord {
    pub id: i64,
    pub library_id: i64,
    pub filename: String,
    pub file_path: String,
    pub file_extension: String,
    pub modified_time: i64,
    pub file_size: i64,
    pub is_encrypted: bool,
    pub indexed_at: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct PageRecord {
    pub id: i64,
    pub document_id: i64,
    pub page_number: i32,
    pub content: String,
    pub embedding: Option<Vec<f32>>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct SavedPasswordRecord {
    pub file_path: String,
    pub encrypted_password: String,
    pub nonce: String,
    pub updated_at: String,
}

#[derive(Debug, Clone)]
pub struct SearchHit {
    pub document_id: i64,
    pub filename: String,
    pub file_path: String,
    pub page_number: i32,
    pub snippet: String,
    pub score: f32,
    pub is_semantic: bool,
}

#[derive(Clone)]
pub struct Database {
    conn: Arc<Mutex<Connection>>,
}

impl Database {
    pub fn open_file<P: AsRef<Path>>(path: P) -> Result<Self, String> {
        let conn = Connection::open(path).map_err(|e| format!("Failed to open DuckDB: {e}"))?;
        let db = Self {
            conn: Arc::new(Mutex::new(conn)),
        };
        db.init_schema()?;
        Ok(db)
    }

    pub fn open_in_memory() -> Result<Self, String> {
        let conn = Connection::open_in_memory()
            .map_err(|e| format!("Failed to open in-memory DuckDB: {e}"))?;
        let db = Self {
            conn: Arc::new(Mutex::new(conn)),
        };
        db.init_schema()?;
        Ok(db)
    }

    pub fn init_schema(&self) -> Result<(), String> {
        let conn = self.conn.lock().map_err(|e| format!("DB lock error: {e}"))?;

        conn.execute_batch(
            r#"
            CREATE SEQUENCE IF NOT EXISTS seq_libraries_id;
            CREATE TABLE IF NOT EXISTS libraries (
                id BIGINT PRIMARY KEY DEFAULT nextval('seq_libraries_id'),
                name VARCHAR NOT NULL,
                path VARCHAR NOT NULL UNIQUE,
                created_at VARCHAR NOT NULL,
                last_indexed_at VARCHAR
            );

            CREATE SEQUENCE IF NOT EXISTS seq_documents_id;
            CREATE TABLE IF NOT EXISTS documents (
                id BIGINT PRIMARY KEY DEFAULT nextval('seq_documents_id'),
                library_id BIGINT NOT NULL,
                filename VARCHAR NOT NULL,
                file_path VARCHAR NOT NULL UNIQUE,
                file_extension VARCHAR NOT NULL,
                modified_time BIGINT NOT NULL,
                file_size BIGINT NOT NULL,
                is_encrypted BOOLEAN NOT NULL DEFAULT FALSE,
                indexed_at VARCHAR
            );

            CREATE SEQUENCE IF NOT EXISTS seq_document_pages_id;
            CREATE TABLE IF NOT EXISTS document_pages (
                id BIGINT PRIMARY KEY DEFAULT nextval('seq_document_pages_id'),
                document_id BIGINT NOT NULL,
                page_number INTEGER NOT NULL,
                content TEXT NOT NULL,
                embedding_json TEXT
            );

            CREATE TABLE IF NOT EXISTS saved_passwords (
                file_path VARCHAR PRIMARY KEY,
                encrypted_password VARCHAR NOT NULL,
                nonce VARCHAR NOT NULL,
                updated_at VARCHAR NOT NULL
            );
            "#,
        )
        .map_err(|e| format!("Schema initialization error: {e}"))?;

        Ok(())
    }

    pub fn add_or_get_library(&self, name: &str, path: &str) -> Result<Library, String> {
        let conn = self.conn.lock().map_err(|e| format!("DB lock error: {e}"))?;
        let now = chrono::Utc::now().to_rfc3339();

        let mut stmt = conn
            .prepare("SELECT id, name, path, created_at, last_indexed_at FROM libraries WHERE path = ?")
            .map_err(|e| e.to_string())?;

        let existing = stmt
            .query_row(params![path], |row| {
                Ok(Library {
                    id: row.get(0)?,
                    name: row.get(1)?,
                    path: row.get(2)?,
                    created_at: row.get(3)?,
                    last_indexed_at: row.get(4)?,
                })
            })
            .ok();

        if let Some(lib) = existing {
            return Ok(lib);
        }

        conn.execute(
            "INSERT INTO libraries (name, path, created_at) VALUES (?, ?, ?)",
            params![name, path, now],
        )
        .map_err(|e| format!("Failed to insert library: {e}"))?;

        let mut stmt = conn
            .prepare("SELECT id, name, path, created_at, last_indexed_at FROM libraries WHERE path = ?")
            .map_err(|e| e.to_string())?;

        stmt.query_row(params![path], |row| {
            Ok(Library {
                id: row.get(0)?,
                name: row.get(1)?,
                path: row.get(2)?,
                created_at: row.get(3)?,
                last_indexed_at: row.get(4)?,
            })
        })
        .map_err(|e| e.to_string())
    }

    pub fn list_libraries(&self) -> Result<Vec<Library>, String> {
        let conn = self.conn.lock().map_err(|e| format!("DB lock error: {e}"))?;
        let mut stmt = conn
            .prepare("SELECT id, name, path, created_at, last_indexed_at FROM libraries ORDER BY name ASC")
            .map_err(|e| e.to_string())?;

        let mut rows = stmt.query([]).map_err(|e| e.to_string())?;
        let mut libraries = Vec::new();
        while let Some(row) = rows.next().map_err(|e| e.to_string())? {
            libraries.push(Library {
                id: row.get(0).map_err(|e| e.to_string())?,
                name: row.get(1).map_err(|e| e.to_string())?,
                path: row.get(2).map_err(|e| e.to_string())?,
                created_at: row.get(3).map_err(|e| e.to_string())?,
                last_indexed_at: row.get(4).map_err(|e| e.to_string())?,
            });
        }
        Ok(libraries)
    }

    pub fn update_library_indexed_time(&self, library_id: i64) -> Result<(), String> {
        let conn = self.conn.lock().map_err(|e| format!("DB lock error: {e}"))?;
        let now = chrono::Utc::now().to_rfc3339();
        conn.execute(
            "UPDATE libraries SET last_indexed_at = ? WHERE id = ?",
            params![now, library_id],
        )
        .map_err(|e| e.to_string())?;
        Ok(())
    }

    pub fn list_documents_in_library(&self, library_id: i64) -> Result<Vec<DocumentRecord>, String> {
        let conn = self.conn.lock().map_err(|e| format!("DB lock error: {e}"))?;
        let mut stmt = conn
            .prepare(
                r#"
                SELECT id, library_id, filename, file_path, file_extension, modified_time, file_size, is_encrypted, indexed_at 
                FROM documents 
                WHERE library_id = ? 
                ORDER BY filename COLLATE NOCASE ASC
                "#,
            )
            .map_err(|e| e.to_string())?;

        let mut rows = stmt.query(params![library_id]).map_err(|e| e.to_string())?;
        let mut docs = Vec::new();
        while let Some(row) = rows.next().map_err(|e| e.to_string())? {
            docs.push(DocumentRecord {
                id: row.get(0).map_err(|e| e.to_string())?,
                library_id: row.get(1).map_err(|e| e.to_string())?,
                filename: row.get(2).map_err(|e| e.to_string())?,
                file_path: row.get(3).map_err(|e| e.to_string())?,
                file_extension: row.get(4).map_err(|e| e.to_string())?,
                modified_time: row.get(5).map_err(|e| e.to_string())?,
                file_size: row.get(6).map_err(|e| e.to_string())?,
                is_encrypted: row.get(7).map_err(|e| e.to_string())?,
                indexed_at: row.get(8).map_err(|e| e.to_string())?,
            });
        }
        Ok(docs)
    }

    pub fn get_document_by_path(&self, file_path: &str) -> Result<Option<DocumentRecord>, String> {
        let conn = self.conn.lock().map_err(|e| format!("DB lock error: {e}"))?;
        let mut stmt = conn
            .prepare(
                "SELECT id, library_id, filename, file_path, file_extension, modified_time, file_size, is_encrypted, indexed_at FROM documents WHERE file_path = ?",
            )
            .map_err(|e| e.to_string())?;

        let doc = stmt
            .query_row(params![file_path], |row| {
                Ok(DocumentRecord {
                    id: row.get(0)?,
                    library_id: row.get(1)?,
                    filename: row.get(2)?,
                    file_path: row.get(3)?,
                    file_extension: row.get(4)?,
                    modified_time: row.get(5)?,
                    file_size: row.get(6)?,
                    is_encrypted: row.get(7)?,
                    indexed_at: row.get(8)?,
                })
            })
            .ok();

        Ok(doc)
    }

    pub fn upsert_document(
        &self,
        library_id: i64,
        filename: &str,
        file_path: &str,
        file_extension: &str,
        modified_time: i64,
        file_size: i64,
        is_encrypted: bool,
    ) -> Result<i64, String> {
        let conn = self.conn.lock().map_err(|e| format!("DB lock error: {e}"))?;
        let now = chrono::Utc::now().to_rfc3339();

        let existing_id: Option<i64> = conn
            .query_row(
                "SELECT id FROM documents WHERE file_path = ?",
                params![file_path],
                |row| row.get(0),
            )
            .ok();

        if let Some(id) = existing_id {
            conn.execute(
                r#"
                UPDATE documents 
                SET filename = ?, file_extension = ?, modified_time = ?, file_size = ?, is_encrypted = ?, indexed_at = ?
                WHERE id = ?
                "#,
                params![
                    filename,
                    file_extension,
                    modified_time,
                    file_size,
                    is_encrypted,
                    now,
                    id
                ],
            )
            .map_err(|e| e.to_string())?;

            // Remove old pages to refresh index
            conn.execute("DELETE FROM document_pages WHERE document_id = ?", params![id])
                .map_err(|e| e.to_string())?;

            Ok(id)
        } else {
            conn.execute(
                r#"
                INSERT INTO documents (library_id, filename, file_path, file_extension, modified_time, file_size, is_encrypted, indexed_at)
                VALUES (?, ?, ?, ?, ?, ?, ?, ?)
                "#,
                params![
                    library_id,
                    filename,
                    file_path,
                    file_extension,
                    modified_time,
                    file_size,
                    is_encrypted,
                    now
                ],
            )
            .map_err(|e| e.to_string())?;

            let id: i64 = conn
                .query_row(
                    "SELECT id FROM documents WHERE file_path = ?",
                    params![file_path],
                    |row| row.get(0),
                )
                .map_err(|e| e.to_string())?;

            Ok(id)
        }
    }

    pub fn insert_page(
        &self,
        document_id: i64,
        page_number: i32,
        content: &str,
        embedding: Option<&[f32]>,
    ) -> Result<(), String> {
        let conn = self.conn.lock().map_err(|e| format!("DB lock error: {e}"))?;
        let emb_json = embedding.map(|e| serde_json::to_string(e).unwrap_or_default());

        conn.execute(
            r#"
            INSERT INTO document_pages (document_id, page_number, content, embedding_json)
            VALUES (?, ?, ?, ?)
            "#,
            params![document_id, page_number, content, emb_json],
        )
        .map_err(|e| e.to_string())?;

        Ok(())
    }

    pub fn save_password(
        &self,
        file_path: &str,
        encrypted_password: &str,
        nonce: &str,
    ) -> Result<(), String> {
        let conn = self.conn.lock().map_err(|e| format!("DB lock error: {e}"))?;
        let now = chrono::Utc::now().to_rfc3339();

        conn.execute(
            r#"
            INSERT OR REPLACE INTO saved_passwords (file_path, encrypted_password, nonce, updated_at)
            VALUES (?, ?, ?, ?)
            "#,
            params![file_path, encrypted_password, nonce, now],
        )
        .map_err(|e| e.to_string())?;

        Ok(())
    }

    pub fn get_saved_password(&self, file_path: &str) -> Result<Option<SavedPasswordRecord>, String> {
        let conn = self.conn.lock().map_err(|e| format!("DB lock error: {e}"))?;
        let mut stmt = conn
            .prepare("SELECT file_path, encrypted_password, nonce, updated_at FROM saved_passwords WHERE file_path = ?")
            .map_err(|e| e.to_string())?;

        let rec = stmt
            .query_row(params![file_path], |row| {
                Ok(SavedPasswordRecord {
                    file_path: row.get(0)?,
                    encrypted_password: row.get(1)?,
                    nonce: row.get(2)?,
                    updated_at: row.get(3)?,
                })
            })
            .ok();

        Ok(rec)
    }

    pub fn search_text(
        &self,
        library_id: Option<i64>,
        query: &str,
        limit: usize,
    ) -> Result<Vec<SearchHit>, String> {
        let conn = self.conn.lock().map_err(|e| format!("DB lock error: {e}"))?;
        let pattern = format!("%{}%", query.to_lowercase());

        let sql = if let Some(_) = library_id {
            r#"
            SELECT d.id, d.filename, d.file_path, p.page_number, p.content
            FROM document_pages p
            JOIN documents d ON p.document_id = d.id
            WHERE d.library_id = ? AND lower(p.content) LIKE ?
            LIMIT ?
            "#
        } else {
            r#"
            SELECT d.id, d.filename, d.file_path, p.page_number, p.content
            FROM document_pages p
            JOIN documents d ON p.document_id = d.id
            WHERE lower(p.content) LIKE ?
            LIMIT ?
            "#
        };

        let mut hits = Vec::new();
        let mut stmt = conn.prepare(sql).map_err(|e| e.to_string())?;

        let mut rows = if let Some(lib_id) = library_id {
            stmt.query(params![lib_id, pattern, limit as i64])
                .map_err(|e| e.to_string())?
        } else {
            stmt.query(params![pattern, limit as i64])
                .map_err(|e| e.to_string())?
        };

        while let Some(row) = rows.next().map_err(|e| e.to_string())? {
            let doc_id: i64 = row.get(0).map_err(|e| e.to_string())?;
            let filename: String = row.get(1).map_err(|e| e.to_string())?;
            let file_path: String = row.get(2).map_err(|e| e.to_string())?;
            let page_number: i32 = row.get(3).map_err(|e| e.to_string())?;
            let content: String = row.get(4).map_err(|e| e.to_string())?;

            let snippet = create_snippet(&content, query);

            hits.push(SearchHit {
                document_id: doc_id,
                filename,
                file_path,
                page_number,
                snippet,
                score: 1.0,
                is_semantic: false,
            });
        }

        Ok(hits)
    }

    pub fn search_semantic(
        &self,
        library_id: Option<i64>,
        query_embedding: &[f32],
        top_k: usize,
    ) -> Result<Vec<SearchHit>, String> {
        let conn = self.conn.lock().map_err(|e| format!("DB lock error: {e}"))?;

        let sql = if let Some(_) = library_id {
            r#"
            SELECT d.id, d.filename, d.file_path, p.page_number, p.content, p.embedding_json
            FROM document_pages p
            JOIN documents d ON p.document_id = d.id
            WHERE d.library_id = ? AND p.embedding_json IS NOT NULL
            "#
        } else {
            r#"
            SELECT d.id, d.filename, d.file_path, p.page_number, p.content, p.embedding_json
            FROM document_pages p
            JOIN documents d ON p.document_id = d.id
            WHERE p.embedding_json IS NOT NULL
            "#
        };

        let mut stmt = conn.prepare(sql).map_err(|e| e.to_string())?;
        let mut rows = if let Some(lib_id) = library_id {
            stmt.query(params![lib_id]).map_err(|e| e.to_string())?
        } else {
            stmt.query([]).map_err(|e| e.to_string())?
        };

        let mut scored_hits = Vec::new();

        while let Some(row) = rows.next().map_err(|e| e.to_string())? {
            let doc_id: i64 = row.get(0).map_err(|e| e.to_string())?;
            let filename: String = row.get(1).map_err(|e| e.to_string())?;
            let file_path: String = row.get(2).map_err(|e| e.to_string())?;
            let page_number: i32 = row.get(3).map_err(|e| e.to_string())?;
            let content: String = row.get(4).map_err(|e| e.to_string())?;
            let emb_json: Option<String> = row.get(5).ok();

            if let Some(json) = emb_json {
                if let Ok(emb) = serde_json::from_str::<Vec<f32>>(&json) {
                    let sim = cosine_similarity(query_embedding, &emb);
                    if sim > 0.25 {
                        let snippet = if content.chars().count() > 180 {
                            let truncated: String = content.chars().take(180).collect();
                            format!("{truncated}...")
                        } else {
                            content
                        };

                        scored_hits.push(SearchHit {
                            document_id: doc_id,
                            filename,
                            file_path,
                            page_number,
                            snippet,
                            score: sim,
                            is_semantic: true,
                        });
                    }
                }
            }
        }

        scored_hits.sort_by(|a, b| b.score.partial_cmp(&a.score).unwrap_or(std::cmp::Ordering::Equal));
        scored_hits.truncate(top_k);

        Ok(scored_hits)
    }
}

pub fn cosine_similarity(a: &[f32], b: &[f32]) -> f32 {
    if a.len() != b.len() || a.is_empty() {
        return 0.0;
    }
    let mut dot = 0.0f32;
    let mut norm_a = 0.0f32;
    let mut norm_b = 0.0f32;
    for (x, y) in a.iter().zip(b.iter()) {
        dot += x * y;
        norm_a += x * x;
        norm_b += y * y;
    }
    if norm_a == 0.0 || norm_b == 0.0 {
        return 0.0;
    }
    dot / (norm_a.sqrt() * norm_b.sqrt())
}

fn create_snippet(content: &str, query: &str) -> String {
    let lower_content = content.to_lowercase();
    let lower_query = query.to_lowercase();

    if let Some(pos) = lower_content.find(&lower_query) {
        let char_indices: Vec<(usize, char)> = content.char_indices().collect();
        let query_char_idx = char_indices.iter().position(|&(byte_pos, _)| byte_pos >= pos).unwrap_or(0);

        let start_idx = query_char_idx.saturating_sub(40);
        let end_idx = (query_char_idx + query.chars().count() + 60).min(char_indices.len());

        let snippet_chars: String = char_indices[start_idx..end_idx].iter().map(|&(_, c)| c).collect();
        let prefix = if start_idx > 0 { "..." } else { "" };
        let suffix = if end_idx < char_indices.len() { "..." } else { "" };
        format!("{prefix}{snippet_chars}{suffix}")
    } else {
        let truncated: String = content.chars().take(120).collect();
        format!("{truncated}...")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_db_lifecycle_and_crud() {
        let db = Database::open_in_memory().expect("Should open in-memory DB");

        let lib = db
            .add_or_get_library("My Docs", "/path/to/my_docs")
            .expect("Should insert library");
        assert_eq!(lib.name, "My Docs");

        let lib_same = db
            .add_or_get_library("My Docs", "/path/to/my_docs")
            .expect("Should fetch same library");
        assert_eq!(lib.id, lib_same.id);

        let libraries = db.list_libraries().unwrap();
        assert_eq!(libraries.len(), 1);

        db.update_library_indexed_time(lib.id).unwrap();

        let doc_id = db
            .upsert_document(
                lib.id,
                "Report.pdf",
                "/path/to/my_docs/Report.pdf",
                "pdf",
                12345678,
                1024,
                false,
            )
            .unwrap();

        let doc = db
            .get_document_by_path("/path/to/my_docs/Report.pdf")
            .unwrap()
            .expect("Doc should exist");
        assert_eq!(doc.id, doc_id);
        assert_eq!(doc.filename, "Report.pdf");

        let docs = db.list_documents_in_library(lib.id).unwrap();
        assert_eq!(docs.len(), 1);
        assert_eq!(docs[0].filename, "Report.pdf");

        // Insert pages
        let emb = vec![1.0f32, 0.0f32, 0.0f32];
        db.insert_page(doc_id, 1, "This is the first page about Rust programming.", Some(&emb))
            .unwrap();
        db.insert_page(doc_id, 2, "This is the second page about DuckDB datastore.", None)
            .unwrap();

        // Text search
        let hits = db.search_text(Some(lib.id), "Rust", 10).unwrap();
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].page_number, 1);
        assert!(hits[0].snippet.contains("Rust"));

        // Global text search (None library_id)
        let hits_global = db.search_text(None, "DuckDB", 10).unwrap();
        assert_eq!(hits_global.len(), 1);
        assert_eq!(hits_global[0].page_number, 2);

        // Semantic search
        let query_emb = vec![0.9f32, 0.1f32, 0.0f32];
        let sem_hits = db.search_semantic(Some(lib.id), &query_emb, 5).unwrap();
        assert_eq!(sem_hits.len(), 1);
        assert_eq!(sem_hits[0].page_number, 1);
        assert!(sem_hits[0].score > 0.8);

        // Password saving and retrieval
        db.save_password("/path/to/my_docs/Secret.pdf", "enc_hex_123", "nonce_456")
            .unwrap();
        let saved_pwd = db
            .get_saved_password("/path/to/my_docs/Secret.pdf")
            .unwrap()
            .expect("Password record should exist");
        assert_eq!(saved_pwd.encrypted_password, "enc_hex_123");
        assert_eq!(saved_pwd.nonce, "nonce_456");
    }

    #[test]
    fn test_cosine_similarity() {
        let v1 = vec![1.0, 0.0, 0.0];
        let v2 = vec![1.0, 0.0, 0.0];
        assert!((cosine_similarity(&v1, &v2) - 1.0).abs() < 1e-5);

        let v3 = vec![0.0, 1.0, 0.0];
        assert!(cosine_similarity(&v1, &v3).abs() < 1e-5);

        let empty: Vec<f32> = vec![];
        assert_eq!(cosine_similarity(&empty, &v1), 0.0);
    }

    #[test]
    fn test_create_snippet() {
        let content = "The quick brown fox jumps over the lazy dog in a sunny afternoon.";
        let snippet = create_snippet(content, "fox");
        assert!(snippet.contains("fox"));

        let not_found_snippet = create_snippet(content, "cat");
        assert!(!not_found_snippet.is_empty());
    }
}
