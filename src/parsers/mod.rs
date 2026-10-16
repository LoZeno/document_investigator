// Parsers module stub
pub struct ExtractedPage {
    pub page_number: i32,
    pub text: String,
}

pub struct ExtractedDocument {
    pub pages: Vec<ExtractedPage>,
    pub is_encrypted: bool,
}
