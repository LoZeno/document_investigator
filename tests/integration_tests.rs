use document_investigator::db::Database;
use document_investigator::embeddings::DeterministicEmbeddingGenerator;
use document_investigator::indexer::{Indexer, PasswordResponse};
use document_investigator::parsers::{is_supported_file, parse_document};
use document_investigator::search::SearchEngine;
use document_investigator::security::{decrypt_password, encrypt_password, MemoryKeyProvider};
use lopdf::dictionary;
use std::fs::{self, File};
use std::io::Write;
use tempfile::tempdir;
use zip::write::{SimpleFileOptions, ZipWriter};

#[test]
fn test_end_to_end_library_lifecycle() {
    let temp_root = tempdir().unwrap();
    let root_path = temp_root.path();

    // 1. Create subfolder structure
    let subfolder_a = root_path.join("documents").join("sub_a");
    let subfolder_b = root_path.join("documents").join("sub_b");
    fs::create_dir_all(&subfolder_a).unwrap();
    fs::create_dir_all(&subfolder_b).unwrap();

    // 2. Add HTML doc in sub_a
    let html_file = subfolder_a.join("manual.html");
    let mut f_html = File::create(&html_file).unwrap();
    writeln!(
        f_html,
        "<html><body><h1>User Manual</h1><p>Rust systems safety guidelines.</p></body></html>"
    )
    .unwrap();

    // 3. Add DOCX doc in sub_b
    let docx_file = subfolder_b.join("specs.docx");
    {
        let f_docx = File::create(&docx_file).unwrap();
        let mut zip = ZipWriter::new(f_docx);
        let options = SimpleFileOptions::default();
        zip.start_file("word/document.xml", options).unwrap();
        zip.write_all(b"<w:document><w:body><w:p><w:t>DuckDB vector embeddings specification and design.</w:t></w:p></w:body></w:document>").unwrap();
        zip.finish().unwrap();
    }

    // 4. Add EPUB doc in sub_b
    let epub_file = subfolder_b.join("book.epub");
    {
        let f_epub = File::create(&epub_file).unwrap();
        let mut zip = ZipWriter::new(f_epub);
        let options = SimpleFileOptions::default();
        zip.start_file("chapter1.xhtml", options).unwrap();
        zip.write_all(b"<html><body><h1>Chapter 1</h1><p>Architecture of cross-platform desktop applications.</p></body></html>").unwrap();
        zip.finish().unwrap();
    }

    // 5. Initialize Database & Indexer
    let db = Database::open_in_memory().unwrap();
    let lib = db
        .add_or_get_library("Project Library", root_path.to_str().unwrap())
        .unwrap();

    let key_provider = MemoryKeyProvider::random();
    let emb_generator = Box::new(DeterministicEmbeddingGenerator::new(64));
    let indexer = Indexer::new(db.clone(), emb_generator, key_provider);

    // 6. Perform initial indexing
    let stats = indexer
        .scan_and_index(lib.id, root_path, None, |_| PasswordResponse {
            file_path: "".to_string(),
            password: None,
            save_for_future: false,
        })
        .unwrap();

    assert_eq!(stats.indexed_count, 3);
    assert_eq!(stats.skipped_count, 0);
    assert_eq!(stats.error_count, 0);

    // 7. Verify document grid sorting alphabetically by filename
    let docs = db.list_documents_in_library(lib.id).unwrap();
    assert_eq!(docs.len(), 3);
    assert_eq!(docs[0].filename, "book.epub");
    assert_eq!(docs[1].filename, "manual.html");
    assert_eq!(docs[2].filename, "specs.docx");

    // 8. Perform Text & Semantic Search
    let search_emb = Box::new(DeterministicEmbeddingGenerator::new(64));
    let engine = SearchEngine::new(db.clone(), search_emb);

    // Search for "safety"
    let hits_safety = engine.search(Some(lib.id), "safety", 10).unwrap();
    assert_eq!(hits_safety.len(), 1);
    assert_eq!(hits_safety[0].filename, "manual.html");
    assert_eq!(hits_safety[0].matching_pages, vec![1]);

    // Search for "embeddings"
    let hits_emb = engine.search(Some(lib.id), "embeddings", 10).unwrap();
    assert_eq!(hits_emb.len(), 1);
    assert_eq!(hits_emb[0].filename, "specs.docx");

    // 9. Modify a document and verify incremental refresh
    std::thread::sleep(std::time::Duration::from_millis(1100)); // Ensure modified timestamp ticks
    let mut f_html_updated = File::create(&html_file).unwrap();
    writeln!(
        f_html_updated,
        "<html><body><h1>Updated Manual</h1><p>Quantum computing algorithms.</p></body></html>"
    )
    .unwrap();

    let stats_refresh = indexer
        .scan_and_index(lib.id, root_path, None, |_| PasswordResponse {
            file_path: "".to_string(),
            password: None,
            save_for_future: false,
        })
        .unwrap();

    assert_eq!(stats_refresh.indexed_count, 1);
    assert_eq!(stats_refresh.skipped_count, 2);

    // Verify updated search results
    let hits_quantum = engine.search(Some(lib.id), "quantum", 10).unwrap();
    assert_eq!(hits_quantum.len(), 1);
    assert_eq!(hits_quantum[0].filename, "manual.html");
}

#[test]
fn test_password_protected_workflow_and_persistence() {
    let temp_root = tempdir().unwrap();
    let root_path = temp_root.path();

    // Create a plain text file pretending to be encrypted
    let doc_file = root_path.join("secret_notes.html");
    let mut f = File::create(&doc_file).unwrap();
    writeln!(f, "<html><body><p>Top secret confidential report.</p></body></html>").unwrap();

    let db = Database::open_in_memory().unwrap();
    let lib = db
        .add_or_get_library("Secure Vault", root_path.to_str().unwrap())
        .unwrap();

    let master_key = [99u8; 32];
    let key_provider = MemoryKeyProvider::new(master_key);

    // Save encrypted password in DB
    let enc = encrypt_password("OpenSesame123!", &master_key).unwrap();
    db.save_password(
        &doc_file.to_string_lossy(),
        &enc.ciphertext_hex,
        &enc.nonce_hex,
    )
    .unwrap();

    // Fetch and decrypt
    let saved = db
        .get_saved_password(&doc_file.to_string_lossy())
        .unwrap()
        .unwrap();
    let decrypted = decrypt_password(&saved.encrypted_password, &saved.nonce, &master_key).unwrap();
    assert_eq!(decrypted, "OpenSesame123!");

    // Run indexer with saved password
    let emb_generator = Box::new(DeterministicEmbeddingGenerator::new(64));
    let indexer = Indexer::new(db.clone(), emb_generator, key_provider);

    let prompt_called = std::sync::atomic::AtomicBool::new(false);
    let stats = indexer
        .scan_and_index(lib.id, root_path, None, |_req| {
            prompt_called.store(true, std::sync::atomic::Ordering::SeqCst);
            PasswordResponse {
                file_path: "".to_string(),
                password: Some("OpenSesame123!".to_string()),
                save_for_future: true,
            }
        })
        .unwrap();

    assert_eq!(stats.indexed_count, 1);
    assert!(!prompt_called.load(std::sync::atomic::Ordering::SeqCst)); // Because saved password was available
}

#[test]
fn test_all_file_format_parsers() {
    let temp_root = tempdir().unwrap();

    // FB2
    let fb2_path = temp_root.path().join("book.fb2");
    fs::write(
        &fb2_path,
        b"<FictionBook><body><section><p>Fiction Book Paragraph</p></section></body></FictionBook>",
    )
    .unwrap();
    let doc_fb2 = parse_document(&fb2_path, None).unwrap();
    assert!(doc_fb2.pages[0].text.contains("Fiction Book Paragraph"));

    // ODT
    let odt_path = temp_root.path().join("document.odt");
    {
        let f = File::create(&odt_path).unwrap();
        let mut zip = ZipWriter::new(f);
        let options = SimpleFileOptions::default();
        zip.start_file("content.xml", options).unwrap();
        zip.write_all(b"<office:document-content><office:body><office:text><text:p>ODT Text</text:p></office:text></office:body></office:document-content>").unwrap();
        zip.finish().unwrap();
    }
    let doc_odt = parse_document(&odt_path, None).unwrap();
    assert!(doc_odt.pages[0].text.contains("ODT Text"));

    // Supported file check
    assert!(is_supported_file("test.pdf"));
    assert!(is_supported_file("test.mobi"));
    assert!(is_supported_file("test.azw3"));
    assert!(is_supported_file("test.lit"));
    assert!(is_supported_file("test.lrf"));
}

#[test]
fn test_pdf_integration_and_page_search() {
    let temp_root = tempdir().unwrap();
    let pdf_path = temp_root.path().join("sample.pdf");

    // Build minimal unencrypted PDF on disk using lopdf
    let mut doc = lopdf::Document::with_version("1.5");
    let pages_id = doc.new_object_id();
    let font_id = doc.add_object(lopdf::dictionary! {
        "Type" => "Font",
        "Subtype" => "Type1",
        "BaseFont" => "Helvetica",
    });
    let resources_id = doc.add_object(lopdf::dictionary! {
        "Font" => lopdf::dictionary! {
            "F1" => font_id,
        },
    });
    let content = lopdf::content::Content {
        operations: vec![
            lopdf::content::Operation::new("BT", vec![]),
            lopdf::content::Operation::new("Tf", vec!["F1".into(), 12.into()]),
            lopdf::content::Operation::new("Td", vec![100.into(), 100.into()]),
            lopdf::content::Operation::new("Tj", vec![lopdf::Object::string_literal("Advanced Scientific Computing in Rust")]),
            lopdf::content::Operation::new("ET", vec![]),
        ],
    };
    let content_id = doc.add_object(lopdf::Stream::new(lopdf::dictionary! {}, content.encode().unwrap()));
    let page_id = doc.add_object(lopdf::dictionary! {
        "Type" => "Page",
        "Parent" => pages_id,
        "Contents" => content_id,
        "Resources" => resources_id,
        "MediaBox" => vec![0.into(), 0.into(), 595.into(), 842.into()],
    });
    let pages = lopdf::dictionary! {
        "Type" => "Pages",
        "Kids" => vec![page_id.into()],
        "Count" => 1,
    };
    doc.set_object(pages_id, pages);
    let catalog_id = doc.add_object(lopdf::dictionary! {
        "Type" => "Catalog",
        "Pages" => pages_id,
    });
    doc.trailer.set("Root", catalog_id);
    doc.save(&pdf_path).unwrap();

    let db = Database::open_in_memory().unwrap();
    let lib = db
        .add_or_get_library("PDF Library", temp_root.path().to_str().unwrap())
        .unwrap();

    let key_provider = MemoryKeyProvider::random();
    let emb_generator = Box::new(DeterministicEmbeddingGenerator::new(64));
    let indexer = Indexer::new(db.clone(), emb_generator, key_provider);

    let stats = indexer
        .scan_and_index(lib.id, temp_root.path(), None, |_| PasswordResponse {
            file_path: "".to_string(),
            password: None,
            save_for_future: false,
        })
        .unwrap();

    assert_eq!(stats.indexed_count, 1);

    let search_emb = Box::new(DeterministicEmbeddingGenerator::new(64));
    let engine = SearchEngine::new(db, search_emb);

    let hits = engine.search(Some(lib.id), "Scientific", 10).unwrap();
    assert_eq!(hits.len(), 1);
    assert_eq!(hits[0].filename, "sample.pdf");
    assert_eq!(hits[0].matching_pages, vec![1]);
}
