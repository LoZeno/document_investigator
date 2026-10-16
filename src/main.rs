use document_investigator::db::Database;
use document_investigator::gui::DocumentInvestigatorApp;
use eframe::egui;

fn main() -> eframe::Result<()> {
    let db_path = "document_investigator.duckdb";
    let db = Database::open_file(db_path).unwrap_or_else(|e| {
        eprintln!("Failed to open file database ({e}), falling back to in-memory.");
        Database::open_in_memory().expect("In-memory DuckDB should always open")
    });

    let app = DocumentInvestigatorApp::new(db);

    let native_options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([1100.0, 720.0])
            .with_min_inner_size([800.0, 500.0])
            .with_title("Document Investigator"),
        ..Default::default()
    };

    eframe::run_native(
        "Document Investigator",
        native_options,
        Box::new(|_cc| Ok(Box::new(app))),
    )
}
