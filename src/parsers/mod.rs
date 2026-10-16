use lopdf::dictionary;
use quick_xml::events::Event;
use quick_xml::reader::Reader;
use std::io::{Cursor, Read};
use std::path::Path;

pub const SUPPORTED_EXTENSIONS: &[&str] = &[
    "pdf", "doc", "docx", "odt", "ods", "odp", "epub", "mobi", "azw3", "azw", "azw4", "fb2",
    "html", "htm", "lit", "lrf",
];

#[derive(Debug, Clone, PartialEq)]
pub struct ExtractedPage {
    pub page_number: i32,
    pub text: String,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ExtractedDocument {
    pub pages: Vec<ExtractedPage>,
    pub is_encrypted: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub enum ParseError {
    PasswordRequired,
    InvalidPassword,
    IoError(String),
    FormatError(String),
    UnsupportedFormat(String),
}

impl std::fmt::Display for ParseError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ParseError::PasswordRequired => write!(f, "Document is password protected"),
            ParseError::InvalidPassword => write!(f, "Invalid password provided for document"),
            ParseError::IoError(e) => write!(f, "IO error: {e}"),
            ParseError::FormatError(e) => write!(f, "Format error: {e}"),
            ParseError::UnsupportedFormat(e) => write!(f, "Unsupported format: {e}"),
        }
    }
}

impl std::error::Error for ParseError {}

pub fn is_supported_file<P: AsRef<Path>>(path: P) -> bool {
    let ext = path
        .as_ref()
        .extension()
        .and_then(|e| e.to_str())
        .map(|e| e.to_lowercase())
        .unwrap_or_default();
    SUPPORTED_EXTENSIONS.contains(&ext.as_str())
}

pub fn parse_document<P: AsRef<Path>>(
    path: P,
    password: Option<&str>,
) -> Result<ExtractedDocument, ParseError> {
    let path_ref = path.as_ref();
    let ext = path_ref
        .extension()
        .and_then(|e| e.to_str())
        .map(|e| e.to_lowercase())
        .unwrap_or_default();

    match ext.as_str() {
        "pdf" => parse_pdf_file(path_ref, password),
        "docx" => parse_docx_file(path_ref),
        "odt" | "ods" | "odp" => parse_odt_file(path_ref),
        "epub" => parse_epub_file(path_ref),
        "fb2" => parse_fb2_file(path_ref),
        "html" | "htm" => parse_html_file(path_ref),
        "mobi" | "azw" | "azw3" | "azw4" | "lit" | "lrf" | "doc" => {
            parse_hybrid_ebook_file(path_ref)
        }
        _ => Err(ParseError::UnsupportedFormat(ext)),
    }
}

// ----------------------------------------------------------------------------
// PDF Parser (lopdf)
// ----------------------------------------------------------------------------

pub fn parse_pdf_bytes(
    bytes: &[u8],
    password: Option<&str>,
) -> Result<ExtractedDocument, ParseError> {
    let mut doc = lopdf::Document::load_mem(bytes)
        .map_err(|e| ParseError::FormatError(format!("Failed to load PDF bytes: {e}")))?;

    if doc.is_encrypted() {
        if let Some(pwd) = password {
            if doc.decrypt(pwd.as_bytes()).is_err() {
                return Err(ParseError::InvalidPassword);
            }
        } else {
            // Check if it decrypts with empty password (some encrypted PDFs have empty user password)
            if doc.decrypt(b"").is_err() {
                return Err(ParseError::PasswordRequired);
            }
        }
    }

    let mut pages = Vec::new();
    let page_numbers = doc.get_pages();
    for (page_num, page_id) in page_numbers {
        let text = doc.extract_text(&[page_num]).unwrap_or_else(|_| {
            // Fallback manual content stream extraction for page
            let mut page_text = String::new();
            if let Ok(content_data) = doc.get_page_content(page_id) {
                if let Ok(content) = lopdf::content::Content::decode(&content_data) {
                    for op in content.operations {
                        if op.operator == "Tj" || op.operator == "TJ" {
                            for operand in op.operands {
                                if let Ok(s) = operand.as_str() {
                                    page_text.push_str(&String::from_utf8_lossy(s));
                                    page_text.push(' ');
                                }
                            }
                        }
                    }
                }
            }
            page_text
        });

        let cleaned_text = text.trim().to_string();
        if !cleaned_text.is_empty() {
            pages.push(ExtractedPage {
                page_number: page_num as i32,
                text: cleaned_text,
            });
        }
    }

    if pages.is_empty() {
        pages.push(ExtractedPage {
            page_number: 1,
            text: String::new(),
        });
    }

    Ok(ExtractedDocument {
        pages,
        is_encrypted: doc.is_encrypted(),
    })
}

fn parse_pdf_file<P: AsRef<Path>>(
    path: P,
    password: Option<&str>,
) -> Result<ExtractedDocument, ParseError> {
    let bytes = std::fs::read(&path)
        .map_err(|e| ParseError::IoError(format!("Failed to read PDF file: {e}")))?;
    parse_pdf_bytes(&bytes, password)
}

// ----------------------------------------------------------------------------
// DOCX Parser (ZIP + word/document.xml)
// ----------------------------------------------------------------------------

pub fn parse_docx_bytes(bytes: &[u8]) -> Result<ExtractedDocument, ParseError> {
    let cursor = Cursor::new(bytes);
    let mut archive = zip::ZipArchive::new(cursor)
        .map_err(|e| ParseError::FormatError(format!("Invalid DOCX zip archive: {e}")))?;

    let mut doc_xml = String::new();
    let mut file = archive
        .by_name("word/document.xml")
        .map_err(|e| ParseError::FormatError(format!("word/document.xml not found in DOCX: {e}")))?;
    file.read_to_string(&mut doc_xml)
        .map_err(|e| ParseError::IoError(e.to_string()))?;

    let text = extract_text_from_xml(&doc_xml);
    let pages = split_text_into_logical_pages(&text, 2500);

    Ok(ExtractedDocument {
        pages,
        is_encrypted: false,
    })
}

fn parse_docx_file<P: AsRef<Path>>(path: P) -> Result<ExtractedDocument, ParseError> {
    let bytes = std::fs::read(&path)
        .map_err(|e| ParseError::IoError(format!("Failed to read DOCX file: {e}")))?;
    parse_docx_bytes(&bytes)
}

// ----------------------------------------------------------------------------
// ODT Parser (ZIP + content.xml)
// ----------------------------------------------------------------------------

pub fn parse_odt_bytes(bytes: &[u8]) -> Result<ExtractedDocument, ParseError> {
    let cursor = Cursor::new(bytes);
    let mut archive = zip::ZipArchive::new(cursor)
        .map_err(|e| ParseError::FormatError(format!("Invalid ODT zip archive: {e}")))?;

    let mut content_xml = String::new();
    let mut file = archive
        .by_name("content.xml")
        .map_err(|e| ParseError::FormatError(format!("content.xml not found in ODT: {e}")))?;
    file.read_to_string(&mut content_xml)
        .map_err(|e| ParseError::IoError(e.to_string()))?;

    let text = extract_text_from_xml(&content_xml);
    let pages = split_text_into_logical_pages(&text, 2500);

    Ok(ExtractedDocument {
        pages,
        is_encrypted: false,
    })
}

fn parse_odt_file<P: AsRef<Path>>(path: P) -> Result<ExtractedDocument, ParseError> {
    let bytes = std::fs::read(&path)
        .map_err(|e| ParseError::IoError(format!("Failed to read ODT file: {e}")))?;
    parse_odt_bytes(&bytes)
}

// ----------------------------------------------------------------------------
// EPUB Parser (ZIP + (X)HTML content files)
// ----------------------------------------------------------------------------

pub fn parse_epub_bytes(bytes: &[u8]) -> Result<ExtractedDocument, ParseError> {
    let cursor = Cursor::new(bytes);
    let mut archive = zip::ZipArchive::new(cursor)
        .map_err(|e| ParseError::FormatError(format!("Invalid EPUB zip archive: {e}")))?;

    let mut file_names: Vec<String> = Vec::new();
    for i in 0..archive.len() {
        if let Ok(file) = archive.by_index(i) {
            if let Ok(name_cow) = file.name() {
                let name = name_cow.to_string();
                if name.ends_with(".xhtml") || name.ends_with(".html") || name.ends_with(".htm") {
                    file_names.push(name);
                }
            }
        }
    }

    file_names.sort();

    let mut pages = Vec::new();
    let mut page_num = 1;

    for name in file_names {
        if let Ok(mut file) = archive.by_name(&name) {
            let mut html_content = String::new();
            if file.read_to_string(&mut html_content).is_ok() {
                let text = strip_html_tags(&html_content);
                if !text.trim().is_empty() {
                    pages.push(ExtractedPage {
                        page_number: page_num,
                        text: text.trim().to_string(),
                    });
                    page_num += 1;
                }
            }
        }
    }

    if pages.is_empty() {
        pages.push(ExtractedPage {
            page_number: 1,
            text: String::new(),
        });
    }

    Ok(ExtractedDocument {
        pages,
        is_encrypted: false,
    })
}

fn parse_epub_file<P: AsRef<Path>>(path: P) -> Result<ExtractedDocument, ParseError> {
    let bytes = std::fs::read(&path)
        .map_err(|e| ParseError::IoError(format!("Failed to read EPUB file: {e}")))?;
    parse_epub_bytes(&bytes)
}

// ----------------------------------------------------------------------------
// FB2 Parser (FictionBook XML)
// ----------------------------------------------------------------------------

pub fn parse_fb2_bytes(bytes: &[u8]) -> Result<ExtractedDocument, ParseError> {
    let xml_str = String::from_utf8_lossy(bytes);
    let text = extract_text_from_xml(&xml_str);
    let pages = split_text_into_logical_pages(&text, 2500);

    Ok(ExtractedDocument {
        pages,
        is_encrypted: false,
    })
}

fn parse_fb2_file<P: AsRef<Path>>(path: P) -> Result<ExtractedDocument, ParseError> {
    let bytes = std::fs::read(&path)
        .map_err(|e| ParseError::IoError(format!("Failed to read FB2 file: {e}")))?;
    parse_fb2_bytes(&bytes)
}

// ----------------------------------------------------------------------------
// HTML / HTM Parser
// ----------------------------------------------------------------------------

pub fn parse_html_bytes(bytes: &[u8]) -> Result<ExtractedDocument, ParseError> {
    let html_str = String::from_utf8_lossy(bytes);
    let text = strip_html_tags(&html_str);
    let pages = split_text_into_logical_pages(&text, 2500);

    Ok(ExtractedDocument {
        pages,
        is_encrypted: false,
    })
}

fn parse_html_file<P: AsRef<Path>>(path: P) -> Result<ExtractedDocument, ParseError> {
    let bytes = std::fs::read(&path)
        .map_err(|e| ParseError::IoError(format!("Failed to read HTML file: {e}")))?;
    parse_html_bytes(&bytes)
}

// ----------------------------------------------------------------------------
// Hybrid EBook Parser (MOBI / AZW / AZW3 / AZW4 / LIT / LRF / DOC)
// ----------------------------------------------------------------------------

fn parse_hybrid_ebook_file<P: AsRef<Path>>(path: P) -> Result<ExtractedDocument, ParseError> {
    let path_ref = path.as_ref();

    // Strategy 1: Check if external CLI tool (ebook-convert or pdftotext/catdoc) is available
    if let Ok(tool_path) = which::which("ebook-convert") {
        let temp_txt = tempfile::Builder::new()
            .suffix(".txt")
            .tempfile()
            .map_err(|e| ParseError::IoError(e.to_string()))?;

        let status = std::process::Command::new(tool_path)
            .arg(path_ref)
            .arg(temp_txt.path())
            .output();

        if let Ok(output) = status {
            if output.status.success() {
                if let Ok(text) = std::fs::read_to_string(temp_txt.path()) {
                    let pages = split_text_into_logical_pages(&text, 2500);
                    return Ok(ExtractedDocument {
                        pages,
                        is_encrypted: false,
                    });
                }
            }
        }
    }

    // Strategy 2: Pure Rust fallback (PalmDOC unpacker or printable text stream extractor)
    let bytes = std::fs::read(path_ref)
        .map_err(|e| ParseError::IoError(format!("Failed to read file: {e}")))?;

    parse_raw_text_stream_fallback(&bytes)
}

pub fn parse_raw_text_stream_fallback(bytes: &[u8]) -> Result<ExtractedDocument, ParseError> {
    // Attempt PalmDOC / MOBI decompression if header matches
    if bytes.len() > 78 && &bytes[60..68] == b"BOOKMOBI" {
        if let Some(text) = try_extract_mobi_text(bytes) {
            let pages = split_text_into_logical_pages(&text, 2500);
            return Ok(ExtractedDocument {
                pages,
                is_encrypted: false,
            });
        }
    }

    // Generic fallback: Extract consecutive ASCII/UTF-8 readable sequences
    let mut collected = String::new();
    let mut current_word = Vec::new();

    for &b in bytes {
        if b >= 32 && b <= 126 || b == b'\n' || b == b'\r' || b == b'\t' {
            current_word.push(b);
        } else if b >= 128 {
            current_word.push(b);
        } else {
            if current_word.len() >= 4 {
                if let Ok(s) = std::str::from_utf8(&current_word) {
                    collected.push_str(s);
                    collected.push(' ');
                }
            }
            current_word.clear();
        }
    }

    if current_word.len() >= 4 {
        if let Ok(s) = std::str::from_utf8(&current_word) {
            collected.push_str(s);
        }
    }

    let pages = split_text_into_logical_pages(&collected, 2500);
    Ok(ExtractedDocument {
        pages,
        is_encrypted: false,
    })
}

fn try_extract_mobi_text(bytes: &[u8]) -> Option<String> {
    if bytes.len() < 86 {
        return None;
    }
    let num_records = u16::from_be_bytes([bytes[76], bytes[77]]) as usize;
    if num_records == 0 || bytes.len() < 78 + num_records * 8 {
        return None;
    }

    let mut record_offsets = Vec::new();
    for i in 0..num_records {
        let offset_pos = 78 + i * 8;
        let offset = u32::from_be_bytes([
            bytes[offset_pos],
            bytes[offset_pos + 1],
            bytes[offset_pos + 2],
            bytes[offset_pos + 3],
        ]) as usize;
        record_offsets.push(offset);
    }
    record_offsets.push(bytes.len());

    let mut out_text = String::new();
    // Record 0 has metadata; records 1..num_records contain text/html
    for i in 1..num_records.min(record_offsets.len() - 1) {
        let start = record_offsets[i];
        let end = record_offsets[i + 1].min(bytes.len());
        if start < end && end <= bytes.len() {
            let slice = &bytes[start..end];
            let decompressed = decompress_palmdoc(slice);
            let cleaned = strip_html_tags(&String::from_utf8_lossy(&decompressed));
            if !cleaned.trim().is_empty() {
                out_text.push_str(&cleaned);
                out_text.push('\n');
            }
        }
    }

    if out_text.trim().is_empty() {
        None
    } else {
        Some(out_text)
    }
}

fn decompress_palmdoc(data: &[u8]) -> Vec<u8> {
    let mut out = Vec::new();
    let mut i = 0;
    while i < data.len() {
        let b = data[i];
        i += 1;
        if b >= 1 && b <= 8 {
            // Literal next b bytes
            let count = b as usize;
            for _ in 0..count {
                if i < data.len() {
                    out.push(data[i]);
                    i += 1;
                }
            }
        } else if b <= 0x7F {
            // Literal single byte
            out.push(b);
        } else if b >= 0xC0 {
            // Space + char
            out.push(b' ');
            out.push(b ^ 0x80);
        } else {
            // Distance + length
            if i < data.len() {
                let b2 = data[i];
                i += 1;
                let distance = (((b as usize & 0x3F) << 3) | ((b2 as usize) >> 5)) as usize;
                let length = ((b2 as usize & 0x1F) + 3) as usize;
                if distance > 0 && distance <= out.len() {
                    let start = out.len() - distance;
                    for j in 0..length {
                        let ch = out[start + (j % distance)];
                        out.push(ch);
                    }
                }
            }
        }
    }
    out
}

// ----------------------------------------------------------------------------
// XML and HTML Text Extraction Utilities
// ----------------------------------------------------------------------------

pub fn extract_text_from_xml(xml: &str) -> String {
    let mut reader = Reader::from_str(xml);
    reader.config_mut().trim_text(true);

    let mut buf = Vec::new();
    let mut text_parts = Vec::new();

    while let Ok(event) = reader.read_event_into(&mut buf) {
        match event {
            Event::Text(e) => {
                if let Ok(t) = e.unescape() {
                    let trimmed = t.trim();
                    if !trimmed.is_empty() {
                        text_parts.push(trimmed.to_string());
                    }
                }
            }
            Event::Eof => break,
            _ => {}
        }
        buf.clear();
    }

    text_parts.join(" ")
}

pub fn strip_html_tags(html: &str) -> String {
    let mut result = String::with_capacity(html.len());
    let mut in_tag = false;
    let mut in_script_or_style = false;
    let mut current_tag = String::new();

    let mut chars = html.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '<' {
            in_tag = true;
            current_tag.clear();
        } else if c == '>' {
            in_tag = false;
            let tag_lower = current_tag.to_lowercase();
            if tag_lower.starts_with("script") || tag_lower.starts_with("style") {
                in_script_or_style = true;
            } else if tag_lower.starts_with("/script") || tag_lower.starts_with("/style") {
                in_script_or_style = false;
            } else if tag_lower == "p" || tag_lower == "br" || tag_lower == "div" || tag_lower == "h1" || tag_lower == "h2" || tag_lower == "h3" {
                result.push('\n');
            }
        } else if in_tag {
            current_tag.push(c);
        } else if !in_script_or_style {
            result.push(c);
        }
    }

    // Decode HTML entities like &nbsp;, &amp;, &lt;, &gt;
    let decoded = result
        .replace("&nbsp;", " ")
        .replace("&amp;", "&")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&#39;", "'");

    // Clean up excessive blank lines
    let mut cleaned_lines = Vec::new();
    for line in decoded.lines() {
        let trimmed = line.trim();
        if !trimmed.is_empty() {
            cleaned_lines.push(trimmed);
        }
    }

    cleaned_lines.join("\n")
}

pub fn split_text_into_logical_pages(text: &str, chars_per_page: usize) -> Vec<ExtractedPage> {
    let trimmed = text.trim();
    if trimmed.is_empty() {
        return vec![ExtractedPage {
            page_number: 1,
            text: String::new(),
        }];
    }

    let char_vec: Vec<char> = trimmed.chars().collect();
    let mut pages = Vec::new();
    let mut page_num = 1;
    let mut start = 0;

    while start < char_vec.len() {
        let end = (start + chars_per_page).min(char_vec.len());
        let page_text: String = char_vec[start..end].iter().collect();
        pages.push(ExtractedPage {
            page_number: page_num,
            text: page_text.trim().to_string(),
        });
        page_num += 1;
        start = end;
    }

    pages
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use zip::write::{SimpleFileOptions, ZipWriter};

    #[test]
    fn test_is_supported_file() {
        assert!(is_supported_file("doc.pdf"));
        assert!(is_supported_file("book.EPUB"));
        assert!(is_supported_file("notes.docx"));
        assert!(is_supported_file("chapter.fb2"));
        assert!(is_supported_file("page.html"));
        assert!(is_supported_file("ebook.mobi"));
        assert!(!is_supported_file("image.png"));
        assert!(!is_supported_file("binary.exe"));
    }

    #[test]
    fn test_strip_html_tags() {
        let html = "<html><head><style>body { color: red; }</style></head><body><h1>Hello World</h1><p>This is a <b>test</b> &amp; example.</p></body></html>";
        let text = strip_html_tags(html);
        assert!(text.contains("Hello World"));
        assert!(text.contains("This is a test & example."));
        assert!(!text.contains("<style>"));
        assert!(!text.contains("color: red"));
    }

    #[test]
    fn test_extract_text_from_xml() {
        let xml = "<root><title>Book Title</title><chapter><p>First paragraph.</p><p>Second paragraph.</p></chapter></root>";
        let text = extract_text_from_xml(xml);
        assert!(text.contains("Book Title"));
        assert!(text.contains("First paragraph."));
        assert!(text.contains("Second paragraph."));
    }

    #[test]
    fn test_split_text_into_logical_pages() {
        let text = "Hello World! This is a test.";
        let pages = split_text_into_logical_pages(text, 10);
        assert!(pages.len() >= 2);
        assert_eq!(pages[0].page_number, 1);
        assert_eq!(pages[1].page_number, 2);

        let empty_pages = split_text_into_logical_pages("", 100);
        assert_eq!(empty_pages.len(), 1);
        assert_eq!(empty_pages[0].page_number, 1);
    }

    #[test]
    fn test_parse_docx_bytes() {
        let mut buf = Vec::new();
        {
            let mut zip = ZipWriter::new(Cursor::new(&mut buf));
            let options = SimpleFileOptions::default();
            zip.start_file("word/document.xml", options).unwrap();
            zip.write_all(b"<w:document><w:body><w:p><w:t>Hello from DOCX!</w:t></w:p></w:body></w:document>").unwrap();
            zip.finish().unwrap();
        }

        let doc = parse_docx_bytes(&buf).expect("DOCX parsing should succeed");
        assert_eq!(doc.pages.len(), 1);
        assert!(doc.pages[0].text.contains("Hello from DOCX!"));
    }

    #[test]
    fn test_parse_odt_bytes() {
        let mut buf = Vec::new();
        {
            let mut zip = ZipWriter::new(Cursor::new(&mut buf));
            let options = SimpleFileOptions::default();
            zip.start_file("content.xml", options).unwrap();
            zip.write_all(b"<office:document-content><office:body><office:text><text:p>Hello from ODT!</text:p></office:text></office:body></office:document-content>").unwrap();
            zip.finish().unwrap();
        }

        let doc = parse_odt_bytes(&buf).expect("ODT parsing should succeed");
        assert_eq!(doc.pages.len(), 1);
        assert!(doc.pages[0].text.contains("Hello from ODT!"));
    }

    #[test]
    fn test_parse_epub_bytes() {
        let mut buf = Vec::new();
        {
            let mut zip = ZipWriter::new(Cursor::new(&mut buf));
            let options = SimpleFileOptions::default();
            zip.start_file("chapter1.xhtml", options).unwrap();
            zip.write_all(b"<html><body><h1>Chapter 1</h1><p>EPUB content here.</p></body></html>").unwrap();
            zip.finish().unwrap();
        }

        let doc = parse_epub_bytes(&buf).expect("EPUB parsing should succeed");
        assert_eq!(doc.pages.len(), 1);
        assert!(doc.pages[0].text.contains("Chapter 1"));
        assert!(doc.pages[0].text.contains("EPUB content here."));
    }

    #[test]
    fn test_parse_fb2_bytes() {
        let fb2_content = b"<FictionBook><body><section><title><p>FB2 Title</p></title><p>Story content</p></section></body></FictionBook>";
        let doc = parse_fb2_bytes(fb2_content).expect("FB2 parsing should succeed");
        assert_eq!(doc.pages.len(), 1);
        assert!(doc.pages[0].text.contains("FB2 Title"));
        assert!(doc.pages[0].text.contains("Story content"));
    }

    #[test]
    fn test_parse_html_bytes() {
        let html_content = b"<html><body><p>HTML document paragraph.</p></body></html>";
        let doc = parse_html_bytes(html_content).expect("HTML parsing should succeed");
        assert_eq!(doc.pages.len(), 1);
        assert!(doc.pages[0].text.contains("HTML document paragraph."));
    }

    #[test]
    fn test_parse_pdf_unencrypted() {
        // Build minimal unencrypted PDF in memory using lopdf
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
                lopdf::content::Operation::new("Tj", vec![lopdf::Object::string_literal("Hello PDF World")]),
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

        let mut bytes = Vec::new();
        doc.save_to(&mut bytes).unwrap();

        let extracted = parse_pdf_bytes(&bytes, None).expect("PDF parsing should succeed");
        assert!(!extracted.is_encrypted);
        assert_eq!(extracted.pages.len(), 1);
        assert!(extracted.pages[0].text.contains("Hello PDF World"));
    }

    #[test]
    fn test_parse_document_unsupported_extension() {
        let res = parse_document("file.xyz", None);
        assert!(matches!(res, Err(ParseError::UnsupportedFormat(_))));
    }

    #[test]
    fn test_parse_error_display() {
        assert_eq!(
            ParseError::PasswordRequired.to_string(),
            "Document is password protected"
        );
        assert_eq!(
            ParseError::InvalidPassword.to_string(),
            "Invalid password provided for document"
        );
        assert_eq!(
            ParseError::IoError("disk error".to_string()).to_string(),
            "IO error: disk error"
        );
        assert_eq!(
            ParseError::FormatError("bad header".to_string()).to_string(),
            "Format error: bad header"
        );
        assert_eq!(
            ParseError::UnsupportedFormat("xyz".to_string()).to_string(),
            "Unsupported format: xyz"
        );
    }
}
