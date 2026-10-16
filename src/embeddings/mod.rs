// Embeddings module stub
pub trait EmbeddingGenerator: Send + Sync {
    fn generate_embedding(&self, text: &str) -> Result<Vec<f32>, String>;
}
