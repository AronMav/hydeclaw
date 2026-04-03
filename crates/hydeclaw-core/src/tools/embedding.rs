/// In-memory cache for tool embeddings used by semantic top-K selection.
///
/// Tool descriptors (name + description) are embedded once per unique key
/// and cached indefinitely (tools rarely change at runtime).
/// The cache is shared across all calls within one agent engine instance.
use std::collections::HashMap;
use tokio::sync::RwLock;

pub struct ToolEmbeddingCache {
    embeddings: RwLock<HashMap<String, Vec<f32>>>,
}

impl ToolEmbeddingCache {
    pub fn new() -> Self {
        Self {
            embeddings: RwLock::new(HashMap::new()),
        }
    }

    /// Return the cached embedding for `key`, or compute it from `text` and cache it.
    pub async fn get_or_embed(
        &self,
        key: &str,
        text: &str,
        store: &crate::memory::MemoryStore,
    ) -> anyhow::Result<Vec<f32>> {
        if let Some(v) = self.embeddings.read().await.get(key) {
            return Ok(v.clone());
        }
        let v = store.embed(text).await?;
        let mut cache = self.embeddings.write().await;
        cache.insert(key.to_string(), v.clone());
        if cache.len() > 200 {
            cache.clear(); // Simple eviction — re-embed on next access
            cache.insert(key.to_string(), v.clone());
        }
        Ok(v)
    }
}

/// Cosine similarity between two equal-length float vectors.
/// Returns 0.0 on zero-norm inputs.
pub fn cosine_similarity(a: &[f32], b: &[f32]) -> f32 {
    let dot: f32 = a.iter().zip(b.iter()).map(|(x, y)| x * y).sum();
    let norm_a: f32 = a.iter().map(|x| x * x).sum::<f32>().sqrt();
    let norm_b: f32 = b.iter().map(|x| x * x).sum::<f32>().sqrt();
    if norm_a == 0.0 || norm_b == 0.0 {
        return 0.0;
    }
    (dot / (norm_a * norm_b)).clamp(-1.0, 1.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn identical_vectors() {
        let v = [1.0f32, 2.0, 3.0];
        assert!((cosine_similarity(&v, &v) - 1.0).abs() < 1e-6);
    }

    #[test]
    fn opposite_vectors() {
        let a = [-1.0f32, -2.0, -3.0];
        let b = [1.0f32, 2.0, 3.0];
        assert!((cosine_similarity(&a, &b) - (-1.0)).abs() < 1e-6);
    }

    #[test]
    fn orthogonal_vectors() {
        let a = [1.0f32, 0.0];
        let b = [0.0f32, 1.0];
        assert!((cosine_similarity(&a, &b) - 0.0).abs() < 1e-6);
    }

    #[test]
    fn zero_norm_returns_zero() {
        let zero = [0.0f32, 0.0];
        let other = [1.0f32, 2.0];
        assert_eq!(cosine_similarity(&zero, &other), 0.0);
        assert_eq!(cosine_similarity(&other, &zero), 0.0);
    }

    #[test]
    fn single_element() {
        let a = [3.0f32];
        let b = [3.0f32];
        assert!((cosine_similarity(&a, &b) - 1.0).abs() < 1e-6);
    }
}
