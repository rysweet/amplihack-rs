//! Embedding utilities for semantic search in the hive mind.
//!
//! Provides cosine similarity and vector normalization functions.
//! The Python counterpart wraps `sentence-transformers` for model loading;
//! here we expose the pure-math utilities that are usable without any ML
//! runtime.

/// Cosine similarity between two vectors.
///
/// For L2-normalized vectors this equals the dot product.
/// Returns a value in \[−1.0, 1.0\].
///
/// # Panics
///
/// Panics if the vectors have different lengths.
pub fn cosine_similarity(a: &[f32], b: &[f32]) -> f32 {
    assert_eq!(a.len(), b.len(), "vectors must have the same dimension");

    let dot: f32 = a.iter().zip(b.iter()).map(|(x, y)| x * y).sum();
    let norm_a: f32 = a.iter().map(|x| x * x).sum::<f32>().sqrt();
    let norm_b: f32 = b.iter().map(|x| x * x).sum::<f32>().sqrt();

    if norm_a == 0.0 || norm_b == 0.0 {
        return 0.0;
    }
    dot / (norm_a * norm_b)
}

/// Cosine similarity between a query vector and a batch of candidate vectors.
///
/// Returns a score for each candidate.
///
/// # Panics
///
/// Panics if any candidate has a different dimension than the query.
pub fn cosine_similarity_batch(query: &[f32], candidates: &[Vec<f32>]) -> Vec<f32> {
    candidates
        .iter()
        .map(|c| cosine_similarity(query, c))
        .collect()
}

/// L2-normalize a vector in place.
///
/// After normalization, cosine similarity equals the dot product.
pub fn normalize(vec: &mut [f32]) {
    let norm: f32 = vec.iter().map(|x| x * x).sum::<f32>().sqrt();
    if norm > 0.0 {
        for x in vec.iter_mut() {
            *x /= norm;
        }
    }
}

/// Dot product of two vectors (useful for pre-normalized embeddings).
///
/// # Panics
///
/// Panics if the vectors have different lengths.
pub fn dot_product(a: &[f32], b: &[f32]) -> f32 {
    assert_eq!(a.len(), b.len(), "vectors must have the same dimension");
    a.iter().zip(b.iter()).map(|(x, y)| x * y).sum()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn identical_vectors_similarity_one() {
        let v = vec![1.0, 0.0, 0.0];
        let sim = cosine_similarity(&v, &v);
        assert!((sim - 1.0).abs() < 1e-6);
    }

    #[test]
    fn orthogonal_vectors_similarity_zero() {
        let a = vec![1.0, 0.0, 0.0];
        let b = vec![0.0, 1.0, 0.0];
        let sim = cosine_similarity(&a, &b);
        assert!(sim.abs() < 1e-6);
    }

    #[test]
    fn opposite_vectors_similarity_negative() {
        let a = vec![1.0, 0.0];
        let b = vec![-1.0, 0.0];
        let sim = cosine_similarity(&a, &b);
        assert!((sim - (-1.0)).abs() < 1e-6);
    }

    #[test]
    fn zero_vector_returns_zero() {
        let a = vec![0.0, 0.0, 0.0];
        let b = vec![1.0, 2.0, 3.0];
        assert_eq!(cosine_similarity(&a, &b), 0.0);
        assert_eq!(cosine_similarity(&b, &a), 0.0);
    }

    #[test]
    fn batch_similarity() {
        let query = vec![1.0, 0.0, 0.0];
        let candidates = vec![
            vec![1.0, 0.0, 0.0],
            vec![0.0, 1.0, 0.0],
            vec![-1.0, 0.0, 0.0],
        ];
        let scores = cosine_similarity_batch(&query, &candidates);
        assert_eq!(scores.len(), 3);
        assert!((scores[0] - 1.0).abs() < 1e-6);
        assert!(scores[1].abs() < 1e-6);
        assert!((scores[2] - (-1.0)).abs() < 1e-6);
    }

    #[test]
    fn batch_empty_candidates() {
        let query = vec![1.0, 0.0];
        let scores = cosine_similarity_batch(&query, &[]);
        assert!(scores.is_empty());
    }

    #[test]
    fn normalize_unit_vector() {
        let mut v = vec![3.0, 4.0];
        normalize(&mut v);
        let norm: f32 = v.iter().map(|x| x * x).sum::<f32>().sqrt();
        assert!((norm - 1.0).abs() < 1e-6);
        assert!((v[0] - 0.6).abs() < 1e-6);
        assert!((v[1] - 0.8).abs() < 1e-6);
    }

    #[test]
    fn normalize_zero_vector_unchanged() {
        let mut v = vec![0.0, 0.0, 0.0];
        normalize(&mut v);
        assert!(v.iter().all(|&x| x == 0.0));
    }

    #[test]
    fn normalized_cosine_equals_dot() {
        let mut a = vec![3.0, 4.0, 0.0];
        let mut b = vec![1.0, 2.0, 3.0];
        normalize(&mut a);
        normalize(&mut b);
        let cos = cosine_similarity(&a, &b);
        let dot = dot_product(&a, &b);
        assert!((cos - dot).abs() < 1e-6);
    }

    #[test]
    fn dot_product_basic() {
        let a = vec![1.0, 2.0, 3.0];
        let b = vec![4.0, 5.0, 6.0];
        let d = dot_product(&a, &b);
        assert!((d - 32.0).abs() < 1e-6);
    }

    #[test]
    #[should_panic(expected = "same dimension")]
    fn cosine_different_dimensions_panics() {
        cosine_similarity(&[1.0, 2.0], &[1.0]);
    }

    #[test]
    #[should_panic(expected = "same dimension")]
    fn dot_product_different_dimensions_panics() {
        dot_product(&[1.0], &[1.0, 2.0]);
    }

    #[test]
    fn similarity_in_range() {
        let a = vec![0.5, 0.3, 0.8, 0.1];
        let b = vec![0.2, 0.9, 0.4, 0.6];
        let sim = cosine_similarity(&a, &b);
        assert!((-1.0..=1.0).contains(&sim));
    }
}
