use crate::db::Database;
use crate::embeddings::EmbeddingGenerator;
use crate::parsers::{is_supported_file, parse_document, ParseError};
use crate::security::{decrypt_password, encrypt_password, KeyProvider};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::mpsc::Sender;
use walkdir::WalkDir;

#[derive(Debug, Clone, PartialEq)]
pub struct PasswordRequest {
    pub file_path: String,
    pub filename: String,
}

#[derive(Debug, Clone, PartialEq)]
pub struct PasswordResponse {
    pub file_path: String,
    pub password: Option<String>,
    pub save_for_future: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub enum IndexingProgress {
    Started { total_files: usize },
    ScanningFile { current: usize, total: usize, file_path: String },
    FileIndexed { current: usize, total: usize, file_path: String },
    FileSkipped { current: usize, total: usize, file_path: String },
    PasswordPromptNeeded(PasswordRequest),
    Completed { indexed_count: usize, skipped_count: usize, error_count: usize },
    Error(String),
}

#[derive(Debug, Default, Clone, PartialEq)]
pub struct IndexingStats {
    pub indexed_count: usize,
    pub skipped_count: usize,
    pub error_count: usize,
}

pub struct Indexer<K: KeyProvider> {
    db: Database,
    embedding_gen: Box<dyn EmbeddingGenerator>,
    key_provider: K,
}

impl<K: KeyProvider> Indexer<K> {
    pub fn new(db: Database, embedding_gen: Box<dyn EmbeddingGenerator>, key_provider: K) -> Self {
        Self {
            db,
            embedding_gen,
            key_provider,
        }
    }

    pub fn scan_and_index<F>(
        &self,
        library_id: i64,
        library_path: &Path,
        progress_sender: Option<&Sender<IndexingProgress>>,
        password_handler: F,
    ) -> Result<IndexingStats, String>
    where
        F: Fn(&PasswordRequest) -> PasswordResponse,
    {
        if !library_path.exists() {
            return Err(format!("Library path does not exist: {}", library_path.display()));
        }

        // 1. Discover all candidate files in subfolders
        let mut candidate_files: Vec<PathBuf> = Vec::new();
        for entry in WalkDir::new(library_path).into_iter().filter_map(|e| e.ok()) {
            if entry.file_type().is_file() && is_supported_file(entry.path()) {
                candidate_files.push(entry.path().to_path_buf());
            }
        }

        let total_files = candidate_files.len();
        if let Some(tx) = progress_sender {
            let _ = tx.send(IndexingProgress::Started { total_files });
        }

        let mut stats = IndexingStats::default();
        let master_key = self
            .key_provider
            .get_or_create_master_key()
            .unwrap_or([0u8; 32]);

        enum ProcessResult {
            Indexed,
            Skipped,
            Error,
        }

        for (idx, file_path) in candidate_files.into_iter().enumerate() {
            let current = idx + 1;
            let path_str = file_path.to_string_lossy().to_string();

            if let Some(tx) = progress_sender {
                let _ = tx.send(IndexingProgress::ScanningFile {
                    current,
                    total: total_files,
                    file_path: path_str.clone(),
                });
            }

            let file_result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                let metadata = match fs::metadata(&file_path) {
                    Ok(m) => m,
                    Err(_) => {
                        return ProcessResult::Error;
                    }
                };

                let modified_time = metadata
                    .modified()
                    .ok()
                    .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                    .map(|d| d.as_secs() as i64)
                    .unwrap_or(0);

                let file_size = metadata.len() as i64;
                let filename = file_path
                    .file_name()
                    .and_then(|n| n.to_str())
                    .unwrap_or("unknown")
                    .to_string();
                let file_extension = file_path
                    .extension()
                    .and_then(|e| e.to_str())
                    .map(|e| e.to_lowercase())
                    .unwrap_or_default();

                // Check if document already indexed and unchanged
                if let Ok(Some(existing)) = self.db.get_document_by_path(&path_str)
                    && existing.modified_time == modified_time && existing.indexed_at.is_some() {
                        return ProcessResult::Skipped;
                    }

                // Attempt initial parse without password or with saved password
                let mut password_to_use: Option<String> = None;
                if let Ok(Some(saved)) = self.db.get_saved_password(&path_str)
                    && let Ok(decrypted) = decrypt_password(
                        &saved.encrypted_password,
                        &saved.nonce,
                        &master_key,
                    ) {
                        password_to_use = Some(decrypted);
                    }

                let mut parse_res = parse_document(&file_path, password_to_use.as_deref());

                // Handle password prompt if encrypted
                if matches!(parse_res, Err(ParseError::PasswordRequired | ParseError::InvalidPassword)) {
                    let req = PasswordRequest {
                        file_path: path_str.clone(),
                        filename: filename.clone(),
                    };

                    if let Some(tx) = progress_sender {
                        let _ = tx.send(IndexingProgress::PasswordPromptNeeded(req.clone()));
                    }

                    let resp = password_handler(&req);
                    if let Some(pwd) = resp.password {
                        if resp.save_for_future
                            && let Ok(encrypted) = encrypt_password(&pwd, &master_key) {
                                let _ = self.db.save_password(
                                    &path_str,
                                    &encrypted.ciphertext_hex,
                                    &encrypted.nonce_hex,
                                );
                            }
                        parse_res = parse_document(&file_path, Some(&pwd));
                    }
                }

                match parse_res {
                    Ok(doc) => {
                        let doc_id = match self.db.upsert_document(
                            library_id,
                            &filename,
                            &path_str,
                            &file_extension,
                            modified_time,
                            file_size,
                            doc.is_encrypted,
                        ) {
                            Ok(id) => id,
                            Err(_) => return ProcessResult::Error,
                        };

                        let _ = self.db.delete_pages_for_document(doc_id);

                        for page in doc.pages {
                            let emb = self
                                .embedding_gen
                                .generate_embedding(&page.text)
                                .ok();
                            let _ = self.db.insert_page(
                                doc_id,
                                page.page_number,
                                &page.text,
                                emb.as_deref(),
                            );
                        }
                        ProcessResult::Indexed
                    }
                    Err(_) => ProcessResult::Error,
                }
            }));

            match file_result {
                Ok(ProcessResult::Indexed) => {
                    stats.indexed_count += 1;
                    if let Some(tx) = progress_sender {
                        let _ = tx.send(IndexingProgress::FileIndexed {
                            current,
                            total: total_files,
                            file_path: path_str,
                        });
                    }
                }
                Ok(ProcessResult::Skipped) => {
                    stats.skipped_count += 1;
                    if let Some(tx) = progress_sender {
                        let _ = tx.send(IndexingProgress::FileSkipped {
                            current,
                            total: total_files,
                            file_path: path_str,
                        });
                    }
                }
                Ok(ProcessResult::Error) | Err(_) => {
                    stats.error_count += 1;
                }
            }
        }

        let _ = self.db.update_library_indexed_time(library_id);

        if let Some(tx) = progress_sender {
            let _ = tx.send(IndexingProgress::Completed {
                indexed_count: stats.indexed_count,
                skipped_count: stats.skipped_count,
                error_count: stats.error_count,
            });
        }

        Ok(stats)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::embeddings::DeterministicEmbeddingGenerator;
    use crate::security::MemoryKeyProvider;
    use std::fs::File;
    use std::io::Write;

    #[test]
    fn test_indexer_flow_with_subfolders() {
        let temp_dir = tempfile::tempdir().unwrap();
        let sub_dir = temp_dir.path().join("subfolder");
        fs::create_dir_all(&sub_dir).unwrap();

        // Create HTML file in root
        let html_path = temp_dir.path().join("root_doc.html");
        let mut f1 = File::create(&html_path).unwrap();
        writeln!(f1, "<html><body><h1>Root Heading</h1><p>Root document text.</p></body></html>").unwrap();

        // Create HTML file in subfolder
        let sub_html_path = sub_dir.join("sub_doc.html");
        let mut f2 = File::create(&sub_html_path).unwrap();
        writeln!(f2, "<html><body><p>Subfolder document content.</p></body></html>").unwrap();

        let db = Database::open_in_memory().unwrap();
        let lib = db
            .add_or_get_library("Test Library", temp_dir.path().to_str().unwrap())
            .unwrap();

        let key_prov = MemoryKeyProvider::random();
        let emb_gen = Box::new(DeterministicEmbeddingGenerator::new(64));
        let indexer = Indexer::new(db.clone(), emb_gen, key_prov);

        let (tx, rx) = std::sync::mpsc::channel();

        let stats = indexer
            .scan_and_index(lib.id, temp_dir.path(), Some(&tx), |_req| {
                PasswordResponse {
                    file_path: "".to_string(),
                    password: None,
                    save_for_future: false,
                }
            })
            .expect("Indexing should succeed");

        assert_eq!(stats.indexed_count, 2);
        assert_eq!(stats.skipped_count, 0);

        // Verify documents are in DB
        let docs = db.list_documents_in_library(lib.id).unwrap();
        assert_eq!(docs.len(), 2);
        assert_eq!(docs[0].filename, "root_doc.html");
        assert_eq!(docs[1].filename, "sub_doc.html");

        // Re-index without modifications: should skip
        let stats2 = indexer
            .scan_and_index(lib.id, temp_dir.path(), None, |_| PasswordResponse {
                file_path: "".to_string(),
                password: None,
                save_for_future: false,
            })
            .unwrap();

        assert_eq!(stats2.indexed_count, 0);
        assert_eq!(stats2.skipped_count, 2);

        // Modify root_doc.html after timestamp difference
        std::thread::sleep(std::time::Duration::from_millis(1100));
        let mut f1_mod = File::create(&html_path).unwrap();
        writeln!(f1_mod, "<html><body><p>Updated text after modification.</p></body></html>").unwrap();

        let stats3 = indexer
            .scan_and_index(lib.id, temp_dir.path(), None, |_| PasswordResponse {
                file_path: "".to_string(),
                password: None,
                save_for_future: false,
            })
            .unwrap();

        assert_eq!(stats3.indexed_count, 1);
        assert_eq!(stats3.skipped_count, 1);

        // Verify updated content in DB
        let hits = db.search_text(Some(lib.id), "Updated", 10).unwrap();
        assert_eq!(hits.len(), 1);
        assert!(hits[0].snippet.contains("Updated text after modification"));

        // Progress messages verified
        let mut received = Vec::new();
        while let Ok(msg) = rx.try_recv() {
            received.push(msg);
        }
        assert!(!received.is_empty());
    }

    #[test]
    fn test_indexer_corrupted_file_error_handling() {
        let temp_dir = tempfile::tempdir().unwrap();
        let bad_pdf = temp_dir.path().join("broken.pdf");
        let mut f = File::create(&bad_pdf).unwrap();
        writeln!(f, "Not a valid PDF header or stream").unwrap();

        let db = Database::open_in_memory().unwrap();
        let lib = db
            .add_or_get_library("Bad Lib", temp_dir.path().to_str().unwrap())
            .unwrap();

        let key_prov = MemoryKeyProvider::random();
        let emb_gen = Box::new(DeterministicEmbeddingGenerator::new(64));
        let indexer = Indexer::new(db, emb_gen, key_prov);

        let stats = indexer
            .scan_and_index(lib.id, temp_dir.path(), None, |_| PasswordResponse {
                file_path: "".to_string(),
                password: None,
                save_for_future: false,
            })
            .unwrap();

        assert_eq!(stats.indexed_count, 0);
        assert_eq!(stats.error_count, 1);
    }

    #[test]
    fn test_indexer_nonexistent_directory() {
        let db = Database::open_in_memory().unwrap();
        let key_prov = MemoryKeyProvider::random();
        let emb_gen = Box::new(DeterministicEmbeddingGenerator::new(64));
        let indexer = Indexer::new(db, emb_gen, key_prov);

        let res = indexer.scan_and_index(1, Path::new("/invalid/path/12345"), None, |_| {
            PasswordResponse {
                file_path: "".to_string(),
                password: None,
                save_for_future: false,
            }
        });

        assert!(res.is_err());
    }
}
