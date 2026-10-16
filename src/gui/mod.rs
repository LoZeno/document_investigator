use crate::db::{Database, DocumentRecord, Library};
use crate::embeddings::{DeterministicEmbeddingGenerator, EmbeddingGenerator, FastEmbedGenerator};
use crate::indexer::{Indexer, IndexingProgress, PasswordRequest, PasswordResponse};
use crate::search::{open_document_at_page, DocumentSearchResult, SearchEngine};
use crate::security::KeyringProvider;
use eframe::egui;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{channel, Receiver, Sender};
use std::thread;

pub struct DocumentInvestigatorApp {
    pub db: Database,
    pub libraries: Vec<Library>,
    pub selected_library_id: Option<i64>,
    pub documents: Vec<DocumentRecord>,
    pub search_query: String,
    pub search_results: Vec<DocumentSearchResult>,
    pub selected_page_per_doc: HashMap<i64, i32>,
    pub search_engine: SearchEngine,
    pub is_indexing: bool,
    pub indexing_status_text: String,
    pub progress_rx: Option<Receiver<IndexingProgress>>,
    pub password_req_rx: Option<Receiver<PasswordRequest>>,
    pub password_resp_tx: Option<Sender<PasswordResponse>>,
    pub pending_password_request: Option<PasswordRequest>,
    pub password_input: String,
    pub save_password_checkbox: bool,
    pub error_message: Option<String>,
    pub status_message: Option<String>,
}

impl DocumentInvestigatorApp {
    pub fn new(db: Database) -> Self {
        let embedding_gen: Box<dyn EmbeddingGenerator> = match FastEmbedGenerator::try_new() {
            Ok(generator) => Box::new(generator),
            Err(_) => Box::new(DeterministicEmbeddingGenerator::default()),
        };

        let search_engine = SearchEngine::new(db.clone(), embedding_gen);
        let mut app = Self {
            db,
            libraries: Vec::new(),
            selected_library_id: None,
            documents: Vec::new(),
            search_query: String::new(),
            search_results: Vec::new(),
            selected_page_per_doc: HashMap::new(),
            search_engine,
            is_indexing: false,
            indexing_status_text: String::new(),
            progress_rx: None,
            password_req_rx: None,
            password_resp_tx: None,
            pending_password_request: None,
            password_input: String::new(),
            save_password_checkbox: true,
            error_message: None,
            status_message: None,
        };

        app.reload_libraries();
        app
    }

    pub fn reload_libraries(&mut self) {
        if let Ok(libs) = self.db.list_libraries() {
            self.libraries = libs;
            if self.selected_library_id.is_none() && !self.libraries.is_empty() {
                self.selected_library_id = Some(self.libraries[0].id);
                self.reload_documents();
            }
        }
    }

    pub fn reload_documents(&mut self) {
        if let Some(lib_id) = self.selected_library_id {
            if let Ok(docs) = self.db.list_documents_in_library(lib_id) {
                self.documents = docs;
            }
        } else {
            self.documents.clear();
        }
    }

    pub fn add_library_folder<P: AsRef<Path>>(&mut self, path: P) {
        let path_ref = path.as_ref();
        let name = path_ref
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("Library")
            .to_string();
        let path_str = path_ref.to_string_lossy().to_string();

        match self.db.add_or_get_library(&name, &path_str) {
            Ok(lib) => {
                self.selected_library_id = Some(lib.id);
                self.reload_libraries();
                self.reload_documents();
                self.start_indexing(lib.id, path_ref.to_path_buf());
            }
            Err(e) => {
                self.error_message = Some(format!("Failed to add library: {e}"));
            }
        }
    }

    pub fn start_indexing(&mut self, library_id: i64, library_path: PathBuf) {
        if self.is_indexing {
            return;
        }

        let (prog_tx, prog_rx) = channel::<IndexingProgress>();
        let (req_tx, req_rx) = channel::<PasswordRequest>();
        let (resp_tx, resp_rx) = channel::<PasswordResponse>();

        self.is_indexing = true;
        self.indexing_status_text = "Starting indexing...".to_string();
        self.progress_rx = Some(prog_rx);
        self.password_req_rx = Some(req_rx);
        self.password_resp_tx = Some(resp_tx);

        let db_clone = self.db.clone();

        thread::spawn(move || {
            let embedding_gen: Box<dyn EmbeddingGenerator> = match FastEmbedGenerator::try_new() {
                Ok(generator) => Box::new(generator),
                Err(_) => Box::new(DeterministicEmbeddingGenerator::default()),
            };

            let key_provider = KeyringProvider;
            let indexer = Indexer::new(db_clone, embedding_gen, key_provider);

            let _ = indexer.scan_and_index(
                library_id,
                &library_path,
                Some(&prog_tx),
                |req| {
                    let _ = req_tx.send(req.clone());
                    resp_rx.recv().unwrap_or(PasswordResponse {
                        file_path: req.file_path.clone(),
                        password: None,
                        save_for_future: false,
                    })
                },
            );
        });
    }

    pub fn trigger_search(&mut self) {
        let query = self.search_query.trim();
        self.selected_page_per_doc.clear();
        if query.is_empty() {
            self.search_results.clear();
            return;
        }

        match self
            .search_engine
            .search(self.selected_library_id, query, 50)
        {
            Ok(results) => {
                self.search_results = results;
            }
            Err(e) => {
                self.error_message = Some(format!("Search failed: {e}"));
            }
        }
    }

    pub fn update_background_channels(&mut self) {
        if let Some(ref rx) = self.password_req_rx {
            if let Ok(req) = rx.try_recv() {
                self.pending_password_request = Some(req);
                self.password_input.clear();
            }
        }

        let mut should_reload = false;
        if let Some(ref rx) = self.progress_rx {
            while let Ok(msg) = rx.try_recv() {
                match msg {
                    IndexingProgress::Started { total_files } => {
                        self.indexing_status_text = format!("Found {total_files} candidate documents...");
                    }
                    IndexingProgress::ScanningFile { current, total, file_path } => {
                        let name = Path::new(&file_path)
                            .file_name()
                            .and_then(|n| n.to_str())
                            .unwrap_or(&file_path);
                        self.indexing_status_text = format!("Indexing ({current}/{total}): {name}");
                    }
                    IndexingProgress::FileIndexed { current, total, .. } => {
                        self.indexing_status_text = format!("Indexed ({current}/{total})");
                    }
                    IndexingProgress::FileSkipped { current, total, .. } => {
                        self.indexing_status_text = format!("Skipped unmodified ({current}/{total})");
                    }
                    IndexingProgress::PasswordPromptNeeded(_) => {}
                    IndexingProgress::Completed { indexed_count, skipped_count, error_count } => {
                        self.is_indexing = false;
                        self.indexing_status_text = format!(
                            "Indexing complete: {indexed_count} indexed, {skipped_count} skipped, {error_count} errors"
                        );
                        self.status_message = Some(self.indexing_status_text.clone());
                        should_reload = true;
                    }
                    IndexingProgress::Error(e) => {
                        self.is_indexing = false;
                        self.error_message = Some(e);
                    }
                }
            }
        }

        if should_reload {
            self.reload_documents();
        }
    }
}

impl eframe::App for DocumentInvestigatorApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        self.update_background_channels();

        // Top Control Bar
        egui::TopBottomPanel::top("top_panel").show(ctx, |ui| {
            ui.add_space(6.0);
            ui.horizontal(|ui| {
                ui.heading("📚 Document Investigator");

                ui.separator();

                // Select library dropdown
                ui.label("Library:");
                let selected_name = self
                    .libraries
                    .iter()
                    .find(|lib| Some(lib.id) == self.selected_library_id)
                    .map(|lib| lib.name.clone())
                    .unwrap_or_else(|| "Select Library...".to_string());

                let mut switch_to_lib_id = None;
                egui::ComboBox::from_id_salt("library_select")
                    .selected_text(selected_name)
                    .show_ui(ui, |ui| {
                        for lib in &self.libraries {
                            let is_selected = Some(lib.id) == self.selected_library_id;
                            if ui.selectable_label(is_selected, &lib.name).clicked() {
                                switch_to_lib_id = Some(lib.id);
                            }
                        }
                    });

                if let Some(new_id) = switch_to_lib_id {
                    self.selected_library_id = Some(new_id);
                    self.reload_documents();
                    if !self.search_query.is_empty() {
                        self.trigger_search();
                    }
                }

                // Add Library Button
                if ui.button("➕ Add Library Folder").clicked() {
                    if let Some(folder) = rfd::FileDialog::new().pick_folder() {
                        self.add_library_folder(folder);
                    }
                }

                // Refresh Button
                if let Some(lib_id) = self.selected_library_id {
                    let btn = ui.add_enabled(!self.is_indexing, egui::Button::new("🔄 Refresh"));
                    if btn.clicked() {
                        if let Some(lib) = self.libraries.iter().find(|l| l.id == lib_id) {
                            let path = PathBuf::from(&lib.path);
                            self.start_indexing(lib_id, path);
                        }
                    }
                }

                if self.is_indexing {
                    ui.spinner();
                    ui.label(&self.indexing_status_text);
                }
            });

            ui.add_space(6.0);

            // Search Box Bar
            ui.horizontal(|ui| {
                ui.label("🔍 Search text & topics:");
                let text_edit = ui.text_edit_singleline(&mut self.search_query);
                if text_edit.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) {
                    self.trigger_search();
                }

                if ui.button("Search").clicked() {
                    self.trigger_search();
                }

                if !self.search_query.is_empty() {
                    if ui.button("❌ Clear").clicked() {
                        self.search_query.clear();
                        self.search_results.clear();
                        self.selected_page_per_doc.clear();
                    }
                }

                if !self.search_results.is_empty() {
                    ui.label(format!("Found {} matching document(s)", self.search_results.len()));
                }
            });

            ui.add_space(4.0);
        });

        // Bottom Status Bar
        egui::TopBottomPanel::bottom("status_bar").show(ctx, |ui| {
            ui.horizontal(|ui| {
                if let Some(ref err) = self.error_message {
                    ui.colored_label(egui::Color32::RED, format!("⚠️ {err}"));
                } else if let Some(ref status) = self.status_message {
                    ui.label(status);
                } else {
                    ui.label(format!("Total documents in library: {}", self.documents.len()));
                }
            });
        });

        // Central Panel - Document and Results Grid
        egui::CentralPanel::default().show(ctx, |ui| {
            egui::ScrollArea::both().show(ui, |ui| {
                if !self.search_query.trim().is_empty() && !self.search_results.is_empty() {
                    // Search Results View
                    ui.heading("Search Results");
                    ui.separator();

                    egui::Grid::new("search_results_grid")
                        .striped(true)
                        .spacing([14.0, 10.0])
                        .min_col_width(80.0)
                        .show(ui, |ui| {
                            ui.strong("Filename");
                            ui.strong("Path");
                            ui.strong("Snippet");
                            ui.strong("Actions");
                            ui.end_row();

                            for res in &self.search_results {
                                let mut hovered_page: Option<i32> = None;
                                let mut clicked_page: Option<i32> = None;

                                // Column 1: Filename with page numbers UNDER it
                                ui.vertical(|ui| {
                                    ui.label(egui::RichText::new(&res.filename).strong());
                                    if !res.matching_pages.is_empty() {
                                        ui.horizontal_wrapped(|ui| {
                                            ui.label(
                                                egui::RichText::new("Pages:")
                                                    .small()
                                                    .color(egui::Color32::from_gray(140)),
                                            );
                                            for &page in &res.matching_pages {
                                                let is_selected = self
                                                    .selected_page_per_doc
                                                    .get(&res.document_id)
                                                    == Some(&page);
                                                let btn_text = format!("p. {page}");
                                                let btn = if is_selected {
                                                    egui::Button::new(
                                                        egui::RichText::new(btn_text)
                                                            .strong()
                                                            .color(egui::Color32::LIGHT_BLUE),
                                                    )
                                                } else {
                                                    egui::Button::new(btn_text)
                                                };
                                                let resp = ui.add(btn);
                                                if resp.hovered() {
                                                    hovered_page = Some(page);
                                                }
                                                if resp.clicked() {
                                                    clicked_page = Some(page);
                                                }
                                            }
                                        });
                                    }
                                });

                                if let Some(page) = clicked_page {
                                    self.selected_page_per_doc.insert(res.document_id, page);
                                    let _ = open_document_at_page(&res.file_path, page);
                                }

                                // Column 2: File Path
                                ui.label(&res.file_path);

                                // Column 3: Snippet (Wider column, showing hovered page snippet or clicked page snippet or first page snippet)
                                let active_page = hovered_page
                                    .or_else(|| self.selected_page_per_doc.get(&res.document_id).copied())
                                    .or_else(|| res.matching_pages.first().copied());

                                let snippet_text = active_page
                                    .and_then(|p| res.get_snippet_for_page(p))
                                    .or_else(|| res.get_default_snippet())
                                    .unwrap_or("No snippet available");

                                ui.vertical(|ui| {
                                    ui.set_min_width(420.0);
                                    ui.set_max_width(650.0);
                                    if let Some(p) = active_page {
                                        ui.label(
                                            egui::RichText::new(format!("Page {p}:"))
                                                .small()
                                                .color(egui::Color32::from_gray(150)),
                                        );
                                    }
                                    ui.add(egui::Label::new(egui::RichText::new(snippet_text).italics()).wrap());
                                });

                                // Column 4: Actions
                                ui.vertical(|ui| {
                                    if ui.button("Open Document").clicked() {
                                        let target_page = self
                                            .selected_page_per_doc
                                            .get(&res.document_id)
                                            .copied()
                                            .unwrap_or(1);
                                        let _ = open_document_at_page(&res.file_path, target_page);
                                    }
                                });

                                ui.end_row();
                            }
                        });
                } else {
                    // Default Library Documents Grid (Sorted alphabetically by filename)
                    ui.heading("Library Documents");
                    ui.separator();

                    if self.documents.is_empty() {
                        ui.label("No documents found in selected library. Click 'Add Library Folder' or 'Refresh'.");
                    } else {
                        egui::Grid::new("documents_grid")
                            .striped(true)
                            .spacing([12.0, 8.0])
                            .min_col_width(100.0)
                            .show(ui, |ui| {
                                ui.strong("Filename");
                                ui.strong("File Path");
                                ui.strong("Type");
                                ui.strong("Size");
                                ui.strong("Encrypted");
                                ui.strong("Actions");
                                ui.end_row();

                                for doc in &self.documents {
                                    ui.label(&doc.filename);
                                    ui.label(&doc.file_path);
                                    ui.label(&doc.file_extension.to_uppercase());
                                    ui.label(format!("{:.1} KB", doc.file_size as f64 / 1024.0));
                                    ui.label(if doc.is_encrypted { "🔒 Yes" } else { "No" });

                                    if ui.button("Open").clicked() {
                                        let _ = open_document_at_page(&doc.file_path, 1);
                                    }

                                    ui.end_row();
                                }
                            });
                    }
                }
            });
        });

        // Password Modal Popup Window
        if let Some(ref req) = self.pending_password_request.clone() {
            egui::Window::new("🔐 Password Protected Document")
                .collapsible(false)
                .resizable(false)
                .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
                .show(ctx, |ui| {
                    ui.label("This document is encrypted and requires a password to index:");
                    ui.add_space(4.0);
                    ui.monospace(&req.file_path);
                    ui.add_space(8.0);

                    ui.horizontal(|ui| {
                        ui.label("Password:");
                        let text_box = ui.add(
                            egui::TextEdit::singleline(&mut self.password_input).password(true),
                        );
                        text_box.request_focus();
                    });

                    ui.add_space(4.0);
                    ui.checkbox(&mut self.save_password_checkbox, "Save password for future access");

                    ui.add_space(8.0);
                    ui.horizontal(|ui| {
                        if ui.button("Unlock & Index").clicked() {
                            if let Some(ref tx) = self.password_resp_tx {
                                let _ = tx.send(PasswordResponse {
                                    file_path: req.file_path.clone(),
                                    password: Some(self.password_input.clone()),
                                    save_for_future: self.save_password_checkbox,
                                });
                            }
                            self.pending_password_request = None;
                            self.password_input.clear();
                        }

                        if ui.button("Skip File").clicked() {
                            if let Some(ref tx) = self.password_resp_tx {
                                let _ = tx.send(PasswordResponse {
                                    file_path: req.file_path.clone(),
                                    password: None,
                                    save_for_future: false,
                                });
                            }
                            self.pending_password_request = None;
                            self.password_input.clear();
                        }
                    });
                });
        }

        ctx.request_repaint_after(std::time::Duration::from_millis(150));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_gui_app_state_and_reload() {
        let db = Database::open_in_memory().unwrap();
        let lib = db.add_or_get_library("Docs", "/test/docs").unwrap();
        db.upsert_document(lib.id, "A_Doc.pdf", "/test/docs/A_Doc.pdf", "pdf", 10, 100, false).unwrap();
        db.upsert_document(lib.id, "B_Doc.docx", "/test/docs/B_Doc.docx", "docx", 10, 200, false).unwrap();

        let mut app = DocumentInvestigatorApp::new(db);
        assert_eq!(app.libraries.len(), 1);
        assert_eq!(app.documents.len(), 2);
        assert_eq!(app.documents[0].filename, "A_Doc.pdf");
        assert_eq!(app.documents[1].filename, "B_Doc.docx");

        // Test search triggering
        app.search_query = "Doc".to_string();
        app.trigger_search();
        assert!(app.search_results.is_empty());

        // Test background message channel processing
        let (prog_tx, prog_rx) = channel();
        let (req_tx, req_rx) = channel();
        let (resp_tx, _resp_rx) = channel();

        app.progress_rx = Some(prog_rx);
        app.password_req_rx = Some(req_rx);
        app.password_resp_tx = Some(resp_tx);

        prog_tx.send(IndexingProgress::Started { total_files: 5 }).unwrap();
        prog_tx.send(IndexingProgress::ScanningFile { current: 1, total: 5, file_path: "/test/docs/A_Doc.pdf".to_string() }).unwrap();
        prog_tx.send(IndexingProgress::FileIndexed { current: 1, total: 5, file_path: "/test/docs/A_Doc.pdf".to_string() }).unwrap();
        prog_tx.send(IndexingProgress::FileSkipped { current: 2, total: 5, file_path: "/test/docs/B_Doc.docx".to_string() }).unwrap();
        prog_tx.send(IndexingProgress::Completed { indexed_count: 1, skipped_count: 1, error_count: 0 }).unwrap();

        req_tx.send(PasswordRequest {
            file_path: "/test/docs/Secret.pdf".to_string(),
            filename: "Secret.pdf".to_string(),
        }).unwrap();

        app.update_background_channels();

        assert!(!app.is_indexing);
        assert!(app.indexing_status_text.contains("Indexing complete"));
        assert_eq!(
            app.pending_password_request,
            Some(PasswordRequest {
                file_path: "/test/docs/Secret.pdf".to_string(),
                filename: "Secret.pdf".to_string(),
            })
        );

        // Test error message handling
        prog_tx.send(IndexingProgress::Error("Disk read error".to_string())).unwrap();
        app.update_background_channels();
        assert_eq!(app.error_message, Some("Disk read error".to_string()));

        // Test clearing search
        app.search_query.clear();
        app.trigger_search();
        assert!(app.search_results.is_empty());

        // Test adding new library folder
        let temp_lib_dir = tempfile::tempdir().unwrap();
        app.add_library_folder(temp_lib_dir.path());
        assert_eq!(app.libraries.len(), 2);

        // Test library selection switch
        app.selected_library_id = Some(lib.id);
        app.reload_documents();
        assert_eq!(app.selected_library_id, Some(lib.id));
        assert_eq!(app.documents.len(), 2);

        // Test search result snippet and page selection state
        let sample_res = DocumentSearchResult {
            document_id: 1,
            filename: "Doc1.pdf".to_string(),
            file_path: "/test/Doc1.pdf".to_string(),
            matching_pages: vec![1, 3],
            snippets: vec!["Page 1 snippet".to_string(), "Page 3 snippet".to_string()],
            page_snippets: HashMap::from([
                (1, "Page 1 snippet".to_string()),
                (3, "Page 3 snippet".to_string()),
            ]),
            max_score: 1.0,
            has_semantic_matches: false,
        };
        app.search_results = vec![sample_res.clone()];
        assert_eq!(sample_res.get_default_snippet(), Some("Page 1 snippet"));
        assert_eq!(sample_res.get_snippet_for_page(3), Some("Page 3 snippet"));

        app.selected_page_per_doc.insert(1, 3);
        assert_eq!(app.selected_page_per_doc.get(&1), Some(&3));

        app.search_query.clear();
        app.trigger_search();
        assert!(app.selected_page_per_doc.is_empty());
    }
}
