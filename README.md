# Document Investigator

A fast, cross-platform desktop application written in Rust for indexing, organizing, and investigating document libraries with hybrid full-text and semantic AI vector search.

---

## Features

- **Cross-Platform Immediate-Mode GUI**: Built with `egui` and `eframe` for native performance across Windows, Linux, and macOS.
- **Embedded DuckDB Datastore**: High-performance local analytical database managing libraries, documents, logical pages, full-text tokens, and vector embeddings without external database server dependencies.
- **Extensive Multi-Format Support**:
  - **Documents**: PDF, Microsoft Word (`.doc`, `.docx`), OpenDocument Text (`.odt`)
  - **eBooks**: EPUB, MOBI, Amazon Kindle (`.azw`, `.azw3`, `.azw4`), FictionBook (`.fb2`), LIT, LRF
  - **Web / Markup**: HTML, HTM
  - **Hybrid Parser**: Pure-Rust native parsers with graceful fallbacks and system CLI integration (e.g., Calibre / `pdftotext`).
- **Hybrid Search Engine**:
  - Combines exact full-text search (FTS) with local semantic vector embeddings (`all-MiniLM-L6-v2` via FastEmbed/ONNX).
  - Relevance scoring that ranks closest matches at the top and furthest matches at the bottom.
  - Page-level result targeting with interactive page buttons.
  - Dynamic snippet preview: hover or click on page numbers to inspect relevant context and open documents at matching locations.
- **Incremental & Background Indexing**:
  - Asynchronous background worker threads with real-time UI progress updates.
  - Automatic file modification time tracking to only index new or modified files.
- **Secure Password & Credential Management**:
  - Detects encrypted/password-protected PDFs during indexing and prompts via modal challenge dialogs.
  - Stores saved passwords encrypted with AES-256-GCM backed by OS native keyrings (Windows Credential Manager, macOS Keychain, Linux Secret Service).

---

## Architecture Overview

```text
document_investigator/
├── src/
│   ├── db/          # Embedded DuckDB schema, migrations, document & page queries
│   ├── embeddings/  # FastEmbed ONNX embedding generator & cosine similarity
│   ├── gui/         # egui/eframe desktop UI, grids, search views, modals
│   ├── indexer/     # Recursive directory scanner, change detection, worker threads
│   ├── parsers/     # Multi-format document extractors & password handling
│   ├── search/      # Hybrid scoring, ranking, snippet resolution, document opening
│   ├── security/    # AES-256-GCM encryption & OS keyring integration
│   ├── lib.rs       # Core library definitions
│   └── main.rs      # Application entrypoint
└── tests/           # Integration test suites
```

---

## Getting Started

### Prerequisites

- [Rust & Cargo](https://rustup.rs/) (edition 2021, Rust 1.75+ recommended)
- Platform C++ build tools (for ONNX Runtime / DuckDB bundling)
  - **Windows**: Visual Studio C++ Build Tools
  - **Linux**: `build-essential`, `libssl-dev`, `pkg-config`
  - **macOS**: Xcode Command Line Tools

### Build and Run

1. Clone the repository:
   ```bash
   git clone https://github.com/<your-username>/document_investigator.git
   cd document_investigator
   ```

2. Run in development mode:
   ```bash
   cargo run
   ```

3. Build optimized release binary:
   ```bash
   cargo build --release
   ```
   The compiled executable will be located in `target/release/document_investigator`.

---

## Running Tests

Execute the full suite of unit and integration tests:

```bash
cargo test
```

---

## License

MIT License or Apache 2.0 (Dual Licensed).
