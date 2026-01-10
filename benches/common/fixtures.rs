use rand::Rng;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

/// Test data structure for benchmarks
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct BenchUser {
    pub id: u64,
    pub name: String,
    pub email: String,
    pub score: u32,
}

impl BenchUser {
    pub fn new(id: u64) -> Self {
        Self {
            id,
            name: format!("User {}", id),
            email: format!("user{}@example.com", id),
            score: (id % 1000) as u32,
        }
    }
}

/// Simulated database with configurable latency
#[derive(Clone)]
pub struct FakeDatabase {
    data: Arc<HashMap<String, BenchUser>>,
    latency_ms: u64,
    query_count: Arc<AtomicUsize>,
}

impl FakeDatabase {
    pub fn new(num_users: usize, latency_ms: u64) -> Self {
        let mut data = HashMap::new();
        for i in 0..num_users {
            let user = BenchUser::new(i as u64);
            data.insert(format!("user:{}", i), user);
        }

        Self {
            data: Arc::new(data),
            latency_ms,
            query_count: Arc::new(AtomicUsize::new(0)),
        }
    }

    pub async fn get(&self, key: &str) -> Option<BenchUser> {
        self.query_count.fetch_add(1, Ordering::Relaxed);

        // Simulate database latency
        tokio::time::sleep(Duration::from_millis(self.latency_ms)).await;

        self.data.get(key).cloned()
    }

    #[allow(dead_code)]
    pub fn query_count(&self) -> usize {
        self.query_count.load(Ordering::Relaxed)
    }

    #[allow(dead_code)]
    pub fn reset_count(&self) {
        self.query_count.store(0, Ordering::Relaxed);
    }
}

/// Generate test keys for different workload patterns
pub struct KeyGenerator {
    num_keys: usize,
}

impl KeyGenerator {
    pub fn new(num_keys: usize) -> Self {
        Self { num_keys }
    }

    /// Generate sequential keys (for cold cache tests)
    pub fn sequential(&self) -> Vec<String> {
        (0..self.num_keys).map(|i| format!("user:{}", i)).collect()
    }

    /// Generate random keys with uniform distribution
    #[allow(dead_code)]
    pub fn uniform_random(&self, count: usize) -> Vec<String> {
        let mut rng = rand::thread_rng();
        (0..count)
            .map(|_| format!("user:{}", rng.gen_range(0..self.num_keys)))
            .collect()
    }

    /// Generate keys with Zipf distribution (realistic - few hot keys)
    #[allow(dead_code)]
    pub fn zipf_random(&self, count: usize) -> Vec<String> {
        let mut rng = rand::thread_rng();
        let mut keys = Vec::with_capacity(count);

        for _ in 0..count {
            // Simplified Zipf: 80% of requests go to 20% of keys
            let key_id = if rng.gen_bool(0.8) {
                // Hot keys (first 20%)
                rng.gen_range(0..(self.num_keys / 5))
            } else {
                // Cold keys (remaining 80%)
                rng.gen_range((self.num_keys / 5)..self.num_keys)
            };
            keys.push(format!("user:{}", key_id));
        }

        keys
    }

    /// Generate keys for mixed workload (some hits, some misses)
    pub fn mixed(&self, hit_ratio: f64) -> Vec<String> {
        let mut rng = rand::thread_rng();
        let hot_key_count = (self.num_keys as f64 * hit_ratio) as usize;

        (0..1000)
            .map(|_| {
                if rng.gen_bool(hit_ratio) {
                    format!("user:{}", rng.gen_range(0..hot_key_count))
                } else {
                    format!("user:{}", rng.gen_range(hot_key_count..self.num_keys))
                }
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    #[tokio::test]
    async fn test_fake_database() {
        use super::FakeDatabase;

        let db = FakeDatabase::new(100, 10);

        let user = db.get("user:0").await;
        assert!(user.is_some());
        assert_eq!(user.unwrap().id, 0);

        assert_eq!(db.query_count(), 1);
    }

    #[test]
    fn test_key_generator() {
        use super::KeyGenerator;

        let key_gen = KeyGenerator::new(100);

        let seq = key_gen.sequential();
        assert_eq!(seq.len(), 100);
        assert_eq!(seq[0], "user:0");

        let uniform = key_gen.uniform_random(50);
        assert_eq!(uniform.len(), 50);

        let zipf = key_gen.zipf_random(100);
        assert_eq!(zipf.len(), 100);
    }
}
