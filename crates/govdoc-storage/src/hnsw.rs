use anyhow::Result;

#[derive(Clone, Debug, PartialEq)]
pub struct VectorHit {
    pub id: i64,
    pub score: f32,
}

pub trait HnswIndex: Send + Sync {
    fn add(&mut self, id: i64, vector: &[f32]) -> Result<()>;

    fn search(&self, vector: &[f32], limit: usize) -> Result<Vec<VectorHit>>;

    fn save(&self) -> Result<()>;
}

#[derive(Default)]
pub struct InMemoryVectorIndex {
    vectors: Vec<(i64, Vec<f32>)>,
}

impl HnswIndex for InMemoryVectorIndex {
    fn add(&mut self, id: i64, vector: &[f32]) -> Result<()> {
        self.vectors.push((id, vector.to_vec()));
        Ok(())
    }

    fn search(&self, vector: &[f32], limit: usize) -> Result<Vec<VectorHit>> {
        let mut hits = self
            .vectors
            .iter()
            .map(|(id, candidate)| VectorHit {
                id: *id,
                score: cosine_similarity(vector, candidate),
            })
            .collect::<Vec<_>>();
        hits.sort_by(|a, b| b.score.total_cmp(&a.score));
        hits.truncate(limit);
        Ok(hits)
    }

    fn save(&self) -> Result<()> {
        Ok(())
    }
}

pub(crate) fn cosine_similarity(a: &[f32], b: &[f32]) -> f32 {
    if a.len() != b.len() || a.is_empty() {
        return 0.0;
    }
    let mut dot = 0.0;
    let mut norm_a = 0.0;
    let mut norm_b = 0.0;
    for (left, right) in a.iter().zip(b.iter()) {
        dot += left * right;
        norm_a += left * left;
        norm_b += right * right;
    }
    if norm_a == 0.0 || norm_b == 0.0 {
        0.0
    } else {
        dot / (norm_a.sqrt() * norm_b.sqrt())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn in_memory_index_returns_nearest_vector() {
        let mut index = InMemoryVectorIndex::default();
        index.add(1, &[1.0, 0.0]).unwrap();
        index.add(2, &[0.0, 1.0]).unwrap();

        let hits = index.search(&[0.9, 0.1], 1).unwrap();

        assert_eq!(hits[0].id, 1);
    }
}
