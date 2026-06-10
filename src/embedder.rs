use fastembed::{TextEmbedding, EmbeddingModel};

pub struct Embedder {
    model: TextEmbedding,
}

impl Embedder {
    pub fn try_new() -> Result<Self, Box<dyn std::error::Error>> {
        let model = TextEmbedding::try_new(
            fastembed::InitOptions::new(EmbeddingModel::BGESmallENV15),
        )?;
        Ok(Self { model })
    }

    pub fn embed(&mut self, texts: &[String]) -> Result<Vec<Vec<f32>>, Box<dyn std::error::Error>> {
        let embeddings = self.model.embed(texts, Some(256))?;
        Ok(embeddings.into_iter().map(|e| e.to_vec()).collect())
    }
}

pub fn dot_product(a: &[f32], b: &[f32]) -> f32 {
    debug_assert_eq!(a.len(), b.len(), "vectors must have same length");
    let dot: f32 = a.iter().zip(b.iter()).map(|(x, y)| x * y).sum();
    dot.clamp(-1.0, 1.0)
}

pub fn is_zero_vector(vec: &[f32]) -> bool {
    vec.iter().all(|&x| x == 0.0 || x == -0.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_dot_product_identical() {
        let v = vec![1.0_f32, 0.0, 0.0];
        let sim = dot_product(&v, &v);
        assert!((sim - 1.0).abs() < 0.001, "identical unit vectors should have dot product ~1.0, got {}", sim);
    }

    #[test]
    fn test_dot_product_orthogonal() {
        let a = vec![1.0_f32, 0.0];
        let b = vec![0.0_f32, 1.0];
        let sim = dot_product(&a, &b);
        assert!(sim.abs() < 0.001, "orthogonal vectors should have dot product ~0.0, got {}", sim);
    }

    #[test]
    fn test_dot_product_known() {
        let a = vec![0.5_f32, 0.5];
        let b = vec![0.5_f32, 0.5];
        let sim = dot_product(&a, &b);
        assert!((sim - 0.5).abs() < 0.001, "got {}", sim);
    }

    #[test]
    fn test_is_zero_vector() {
        assert!(is_zero_vector(&vec![0.0; 384]));
        assert!(!is_zero_vector(&{
            let mut v = vec![0.0; 384];
            v[0] = 1.0;
            v
        }));
    }
}
