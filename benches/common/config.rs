use std::env;

/// Configuration for benchmarks, loaded from environment variables.
#[derive(Debug, Clone)]
pub struct BenchConfig {
    /// Redis URL (from REDIS_URL env var, defaults to localhost)
    pub redis_url: String,

    /// Simulated database latency in milliseconds (from DB_LATENCY_MS env var, defaults to 50)
    pub db_latency_ms: u64,

    /// Sample size for benchmarks (from BENCH_SAMPLE_SIZE env var, defaults to 100)
    pub sample_size: usize,
}

impl Default for BenchConfig {
    fn default() -> Self {
        Self {
            redis_url: env::var("REDIS_URL")
                .unwrap_or_else(|_| "redis://localhost:6379".to_string()),
            db_latency_ms: env::var("DB_LATENCY_MS")
                .ok()
                .and_then(|s| s.parse().ok())
                .unwrap_or(50),
            sample_size: env::var("BENCH_SAMPLE_SIZE")
                .ok()
                .and_then(|s| s.parse().ok())
                .unwrap_or(100),
        }
    }
}

impl BenchConfig {
    pub fn new() -> Self {
        let config = Self::default();
        eprintln!("Benchmark Configuration:");
        eprintln!("  Redis URL: {}", config.redis_url);
        eprintln!("  DB Latency: {}ms", config.db_latency_ms);
        eprintln!("  Sample Size: {}", config.sample_size);
        config
    }
}
