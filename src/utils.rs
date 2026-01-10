//! Shared utilities for the cache library.

use std::fmt::Display;
use std::time::{SystemTime, UNIX_EPOCH};

/// Build a composite cache key from namespace and key.
///
/// Format: `{namespace}::{key}`
pub fn build_cache_key<N: Display>(namespace: &N, key: &str) -> String {
    format!("{}::{}", namespace, key)
}

/// Get the current time in milliseconds since UNIX epoch.
pub fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_millis() as i64
}

/// Simple pseudo-random number generator (0.0 to 1.0).
/// This avoids adding a dependency on rand crate.
pub fn rand_simple() -> f64 {
    use std::collections::hash_map::RandomState;
    use std::hash::{BuildHasher, Hasher};

    let state = RandomState::new();
    let mut hasher = state.build_hasher();
    hasher.write_u64(
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos() as u64,
    );
    (hasher.finish() as f64) / (u64::MAX as f64)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_build_cache_key() {
        let key = build_cache_key(&"users", "user:123");
        assert_eq!(key, "users::user:123");
    }

    #[test]
    fn test_now_ms_is_positive() {
        let now = now_ms();
        assert!(now > 0);
    }

    #[test]
    fn test_rand_simple_in_range() {
        for _ in 0..100 {
            let r = rand_simple();
            assert!((0.0..=1.0).contains(&r));
        }
    }
}
