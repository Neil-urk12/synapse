use fastembed::{EmbeddingModel, TextEmbedding};

pub struct Embedder {
    model: TextEmbedding,
}

impl Embedder {
    pub fn try_new() -> Result<Self, Box<dyn std::error::Error>> {
        let model =
            TextEmbedding::try_new(fastembed::InitOptions::new(EmbeddingModel::BGESmallENV15))?;
        Ok(Self { model })
    }

    pub fn embed(&mut self, texts: &[String]) -> Result<Vec<Vec<f32>>, Box<dyn std::error::Error>> {
        let embeddings = self.model.embed(texts, Some(256))?;
        Ok(embeddings.into_iter().map(|e| e.to_vec()).collect())
    }
}

pub fn is_zero_vector(vec: &[f32]) -> bool {
    vec.iter().all(|&x| x == 0.0 || x == -0.0)
}

#[cfg(test)]
mod tests {
    use super::*;

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
