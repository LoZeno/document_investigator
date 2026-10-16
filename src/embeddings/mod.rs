use fastembed::{InitOptions, TextEmbedding};
use std::sync::{Arc, Mutex};

pub trait EmbeddingGenerator: Send + Sync {
    fn generate_embedding(&self, text: &str) -> Result<Vec<f32>, String>;
    fn generate_embeddings_batch(&self, texts: &[&str]) -> Result<Vec<Vec<f32>>, String> {
        let mut results = Vec::with_capacity(texts.len());
        for text in texts {
            results.push(self.generate_embedding(text)?);
        }
        Ok(results)
    }
}

pub struct FastEmbedGenerator {
    model: Arc<Mutex<TextEmbedding>>,
}

impl FastEmbedGenerator {
    pub fn try_new() -> Result<Self, String> {
        let options = InitOptions::default();
        let model = TextEmbedding::try_new(options)
            .map_err(|e| format!("Failed to initialize FastEmbed model: {e}"))?;
        Ok(Self {
            model: Arc::new(Mutex::new(model)),
        })
    }
}

impl EmbeddingGenerator for FastEmbedGenerator {
    fn generate_embedding(&self, text: &str) -> Result<Vec<f32>, String> {
        let model = self.model.lock().map_err(|e| format!("Model lock error: {e}"))?;
        let documents = vec![text];
        let embeddings = model
            .embed(documents, None)
            .map_err(|e| format!("Embedding generation error: {e}"))?;

        embeddings
            .into_iter()
            .next()
            .ok_or_else(|| "No embedding returned".to_string())
    }

    fn generate_embeddings_batch(&self, texts: &[&str]) -> Result<Vec<Vec<f32>>, String> {
        if texts.is_empty() {
            return Ok(Vec::new());
        }
        let model = self.model.lock().map_err(|e| format!("Model lock error: {e}"))?;
        let documents: Vec<&str> = texts.to_vec();
        let embeddings = model
            .embed(documents, None)
            .map_err(|e| format!("Batch embedding generation error: {e}"))?;

        Ok(embeddings)
    }
}

/// A lightweight, deterministic embedding generator used for fast tests and fallbacks.
pub struct DeterministicEmbeddingGenerator {
    dimension: usize,
}

impl DeterministicEmbeddingGenerator {
    pub fn new(dimension: usize) -> Self {
        Self { dimension }
    }
}

impl Default for DeterministicEmbeddingGenerator {
    fn default() -> Self {
        Self::new(384)
    }
}

impl EmbeddingGenerator for DeterministicEmbeddingGenerator {
    fn generate_embedding(&self, text: &str) -> Result<Vec<f32>, String> {
        let mut vec = vec![0.0f32; self.dimension];
        let words = text.to_lowercase();
        let tokens: Vec<&str> = words.split_whitespace().collect();

        if tokens.is_empty() {
            return Ok(vec);
        }

        for token in &tokens {
            let mut hash = 5381u64;
            for b in token.bytes() {
                hash = ((hash << 5).wrapping_add(hash)).wrapping_add(b as u64);
            }
            let idx = (hash as usize) % self.dimension;
            vec[idx] += 1.0;
        }

        // L2 Normalize
        let norm: f32 = vec.iter().map(|v| v * v).sum::<f32>().sqrt();
        if norm > 0.0 {
            for v in &mut vec {
                *v /= norm;
            }
        }

        Ok(vec)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::cosine_similarity;

    #[test]
    fn test_deterministic_embedding_generator() {
        let generator = DeterministicEmbeddingGenerator::new(64);

        let emb1 = generator.generate_embedding("Rust programming language").unwrap();
        let emb2 = generator.generate_embedding("Rust programming systems").unwrap();
        let emb3 = generator.generate_embedding("Cooking delicious pasta recipe").unwrap();

        assert_eq!(emb1.len(), 64);
        assert_eq!(emb2.len(), 64);
        assert_eq!(emb3.len(), 64);

        let sim_similar = cosine_similarity(&emb1, &emb2);
        let sim_different = cosine_similarity(&emb1, &emb3);

        assert!(sim_similar > sim_different);
        assert!(sim_similar > 0.4);
    }

    #[test]
    fn test_batch_embeddings() {
        let generator = DeterministicEmbeddingGenerator::default();
        let batch = vec!["First document", "Second document", "Third document"];
        let results = generator.generate_embeddings_batch(&batch).unwrap();

        assert_eq!(results.len(), 3);
        assert_eq!(results[0].len(), 384);
    }

    #[test]
    fn test_empty_string_embedding() {
        let generator = DeterministicEmbeddingGenerator::default();
        let emb = generator.generate_embedding("").unwrap();
        assert_eq!(emb.len(), 384);
        assert!(emb.iter().all(|&v| v == 0.0));
    }
}
