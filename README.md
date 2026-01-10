# swr-cache

A **stale-while-revalidate (SWR) cache library** for Rust with support for multi-tier caching, background revalidation, and pluggable storage backends (in-memory, Redis).

## Features

- 🚀 **Stale-While-Revalidate (SWR)** semantics for optimal performance and user experience
- 🔄 **Multi-tier caching** - chain multiple stores (e.g., memory L1 + Redis L2)
- 🎯 **Type-safe** - generic over both namespace and value types
- 🔌 **Pluggable stores** - in-memory (HashMap) and Redis implementations out of the box
- ⚡ **Background revalidation** - automatic cache refreshing using `tokio::spawn`
- 🎪 **Deduplication** - prevents thundering herd with concurrent origin requests
- 📦 **Zero-copy entry times** - uses Unix milliseconds for TTL management
- 🧪 **Well-tested** - integration tests for all store backends

## Installation

Add to your `Cargo.toml`:

```toml
[dependencies]
swr-cache = "0.1.0"
tokio = { version = "1", features = ["sync", "time", "rt", "macros"] }
```

## Quick Start

```rust
use swr_cache::{Namespace, HashMapStore, HashMapStoreConfig};
use std::sync::Arc;

#[derive(Clone, Hash, Eq, PartialEq)]
enum CacheNamespace {
    Users,
    Products,
}

impl std::fmt::Display for CacheNamespace {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            CacheNamespace::Users => write!(f, "users"),
            CacheNamespace::Products => write!(f, "products"),
        }
    }
}

#[tokio::main]
async fn main() {
    // Create an in-memory cache
    let memory = Arc::new(HashMapStore::new(HashMapStoreConfig::default()));

    // Wrap in a namespace with 1 minute fresh time, 5 minute stale time
    let cache = Namespace::new(vec![memory], 60_000, 300_000);

    // Use SWR pattern - returns cached value if available, loads from origin if not
    let user = cache.swr(CacheNamespace::Users, "user:123", |key| async {
        // Load from database or API
        fetch_user_from_db(&key).await
    }).await.unwrap();

    println!("User: {:?}", user);
}

async fn fetch_user_from_db(key: &str) -> Option<String> {
    // Simulate database lookup
    Some(format!("User data for {}", key))
}
```

## Architecture

### Core Components

#### 1. **Store Trait** (`src/store.rs`)

Generic interface for cache implementations:

```rust
#[async_trait]
pub trait Store<N, V>: Send + Sync
where
    N: Clone + Eq + Hash + Display + Send + Sync,
    V: Clone + Send + Sync,
{
    fn name(&self) -> &'static str;
    async fn get(&self, namespace: N, key: &str) -> Result<Option<Entry<V>>, CacheError>;
    async fn set(&self, namespace: N, key: &str, entry: Entry<V>) -> Result<(), CacheError>;
    async fn remove(&self, namespace: N, keys: &[&str]) -> Result<(), CacheError>;
}
```

#### 2. **Entry** (`src/entry.rs`)

Cache entry with freshness tracking:

```rust
pub struct Entry<V> {
    pub value: V,
    pub fresh_until: i64,  // Unix milliseconds - entry is fresh before this
    pub stale_until: i64,  // Unix milliseconds - entry is expired after this
}
```

**States:**
- **Fresh**: `now < fresh_until` → return immediately
- **Stale**: `fresh_until <= now < stale_until` → return + revalidate in background
- **Expired**: `now >= stale_until` → load from origin

#### 3. **HashMapStore** (`src/stores/memory.rs`)

Simple in-memory cache using `tokio::sync::RwLock<HashMap>`:

```rust
let config = HashMapStoreConfig {
    evict_on_set: Some(EvictOnSetConfig {
        frequency: 0.5,      // 50% chance to evict on set
        max_items: 1000,     // Keep under 1000 items
    }),
};
let store = HashMapStore::new(config);
```

**Features:**
- Automatic expiration check on get
- Probabilistic LRU eviction
- Composite cache keys: `{namespace}::{key}`
- Zero external dependencies (beyond tokio)

**Best for:**
- Low to moderate concurrency (<8 threads)
- Small to medium cache sizes (<1,000 items)
- Applications prioritizing simplicity

#### 4. **MokaStore** (`src/stores/moka.rs`)

High-performance concurrent cache using the Moka library:

```rust
let config = MokaStoreConfig {
    max_capacity: 10_000,
    time_to_live: Some(Duration::from_secs(300)),
    time_to_idle: Some(Duration::from_secs(60)),
};
let store = MokaStore::new(config);
```

**Features:**
- Lock-free concurrent access
- Automatic background eviction
- Configurable TTL and idle timeout
- TinyLFU admission policy
- Near-optimal hit rate

**Best for:**
- High concurrency (>8 threads)
- Large cache sizes (>10,000 items)
- Production workloads requiring consistent P99 latency
- Applications where performance is critical

#### 5. **RedisStore** (`src/stores/redis.rs`)

Redis-backed cache with TTL support:

```rust
let config = RedisStoreConfig {
    url: "redis://user:password@localhost:6379/0".to_string(),
};
let store = RedisStore::new(config).await?;
```

**Features:**
- Values stored as JSON
- Automatic TTL using `SETEX`
- Async connection pooling
- Automatic expiration cleanup

#### 5. **TieredStore** (`src/tiered.rs`)

Multi-level cache hierarchy:

```rust
let l1 = Arc::new(MemoryStore::new(MemoryStoreConfig::default()));
let l2 = Arc::new(RedisStore::new(redis_config).await?);

let tiered = TieredStore::from_stores(vec![l1, l2]);
```

**Behavior:**
- Checks stores in order (L1 → L2 → L3...)
- On hit, populates all lower tiers in background
- On set/remove, updates all tiers in parallel

#### 6. **SwrCache** (`src/swr.rs`)

Stale-while-revalidate implementation:

```rust
pub async fn swr<F, Fut>(
    &self,
    namespace: N,
    key: &str,
    load_from_origin: F,
) -> Result<Option<V>, CacheError>
where
    F: FnOnce(String) -> Fut + Send + 'static,
    Fut: Future<Output = Option<V>> + Send,
```

**Features:**
- Deduplicates concurrent origin loads
- Returns stale value while revalidating
- Background cache updates with `tokio::spawn`
- No blocking on cache misses

#### 7. **Namespace** (`src/namespace.rs`)

High-level API combining `TieredStore` + `SwrCache`:

```rust
let cache = Namespace::new(stores, fresh_ms, stale_ms);

// Simple CRUD
cache.set(ns, "key", value).await?;
let value = cache.get(ns, "key").await?;
cache.remove(ns, "key").await?;

// SWR pattern
let value = cache.swr(ns, "key", |key| async {
    fetch_from_origin(&key).await
}).await?;
```

## Usage Patterns

### Choosing the Right Store

| Store | Concurrency | Cache Size | Use When | Latency (P99) |
|-------|-------------|------------|----------|---------------|
| **HashMapStore** | Low (<8 threads) | Small (<1k items) | Simplicity matters, zero deps | 100µs-1ms |
| **MokaStore** | High (>8 threads) | Large (>10k items) | Performance critical | 50-200µs |
| **RedisStore** | Any | Any | Persistence, multi-node | 1-5ms |

### Pattern 1: Simple In-Memory Cache with HashMapStore

```rust
let cache = Namespace::new(
    vec![Arc::new(HashMapStore::new(HashMapStoreConfig::default()))],
    60_000,   // 1 minute fresh
    300_000,  // 5 minutes stale
);

let user = cache.swr(Namespace::Users, "user:123", |key| async {
    db.get_user(&key).await
}).await?;
```

### Pattern 2: High-Performance Cache with MokaStore

```rust
let moka = Arc::new(MokaStore::new(MokaStoreConfig {
    max_capacity: 10_000,
    time_to_live: Some(Duration::from_secs(300)),
    time_to_idle: None,
}));

let cache = Namespace::new(vec![moka], 60_000, 300_000);
```

### Pattern 3: L1 Memory + L2 Redis (CDN Pattern)

```rust
let moka = Arc::new(MokaStore::new(MokaStoreConfig {
    max_capacity: 1_000,   // Hot set
    time_to_live: None,
    time_to_idle: Some(Duration::from_secs(300)),
}));

let redis = Arc::new(RedisStore::new(RedisStoreConfig {
    url: "redis://localhost:6379".to_string(),
}).await?);

let cache = Namespace::new(vec![moka, redis], 60_000, 300_000);

// First request: miss L1, miss L2, load from origin, populate both
// Second request: hit L1, immediate response
// After L1 eviction: hit L2, populate L1 in background
```

### Pattern 4: Namespace-Specific Configurations

```rust
// Users: short cache lifetime
let user_cache = Namespace::new(stores.clone(), 30_000, 120_000);

// Products: longer cache lifetime
let product_cache = Namespace::new(stores.clone(), 300_000, 3_600_000);

// API responses: medium lifetime
let api_cache = Namespace::new(stores.clone(), 60_000, 600_000);
```

## Cache State Diagram

```
                        ┌─────────────────────────┐
                        │   Cache Miss / Expired  │
                        └────────────┬────────────┘
                                     │
                    Load from origin (blocking until loaded)
                                     │
                    Cache in background (tokio::spawn)
                                     │
                        ┌────────────▼─────────────┐
                        │      Fresh Entry        │
                        │  (now < fresh_until)    │
                        └────────────┬─────────────┘
                                     │
                        Return value immediately
                     Time passes → now >= fresh_until
                                     │
                        ┌────────────▼─────────────┐
                        │     Stale Entry         │
                        │ (fresh_until <= now     │
                        │  < stale_until)         │
                        └────────────┬─────────────┘
                                     │
                  Return value + spawn revalidation in background
                                     │
                                     │
                        ┌────────────▼──────────────┐
                        │    Expired Entry         │
                        │  (now >= stale_until)    │
                        │   (Auto-deleted)         │
                        └─────────────────────────┘
```

## Testing

The library includes comprehensive integration tests covering:

### Test Scenarios

1. **Memory Store Tests**
   - Cache miss loads from origin
   - Cache hit avoids origin call
   - Deduplication of concurrent loads
   - CRUD operations

2. **Redis Store Tests**
   - Connection and serialization
   - TTL handling
   - Concurrent access
   - Cleanup after remove

3. **Tiered Store Tests**
   - L2 hit populates L1
   - Namespace isolation
   - Full SWR with multiple tiers

### Running Tests

With Redis running locally:

```bash
# Start Redis
./scripts/start-redis.sh

# Run tests
cargo test

# Stop Redis
./scripts/stop-redis.sh
```

Or use the convenience script:

```bash
./scripts/test.sh    # Auto-starts Redis if needed
```

Or with Docker Compose:

```bash
docker compose up -d redis
cargo test
docker compose down
```

## CI/CD

GitHub Actions workflow (`.github/workflows/ci.yml`):
- Spins up Redis 7 service container
- Runs `cargo build`, `cargo test`, `cargo clippy`, `cargo fmt`
- Runs on every push to `main`

## Use Cases

### 1. API Gateway Caching

```rust
// Cache API responses from upstream services
let cache = Namespace::new(stores, 60_000, 600_000);

let response = cache.swr(Namespace::Api, endpoint_path, |path| async {
    fetch_from_upstream(&path).await
}).await?;
```

**Benefits:**
- Serves stale responses during upstream outages
- Background revalidation keeps data fresh
- Reduces load on upstream services

### 2. Database Query Caching

```rust
// Cache frequently queried data
let cache = Namespace::new(stores, 120_000, 600_000);

let user = cache.swr(Namespace::Users, user_id, |id| async {
    db.query("SELECT * FROM users WHERE id = ?", id).await
}).await?;
```

**Benefits:**
- Sub-millisecond cache hits
- Multi-tier reduces database load
- Stale responses during DB issues

### 3. Session Storage

```rust
// Multi-tier session cache
let session_cache = Namespace::new(
    vec![memory_store, redis_store],  // Fast local + persistent
    300_000,  // 5 min fresh
    3_600_000 // 1 hour stale (enough for session recovery)
);

let session = cache.swr(Namespace::Sessions, session_id, |id| async {
    load_session_from_store(&id).await
}).await?;
```

**Benefits:**
- Memory for hot sessions
- Redis for persistence across nodes
- Graceful degradation if Redis unavailable

### 4. Rate Limiting Decision Cache

```rust
// Cache rate limit decisions to avoid database calls
let cache = Namespace::new(stores, 1_000, 60_000);  // Very short TTL

let allowed = cache.swr(Namespace::RateLimit, client_id, |id| async {
    check_rate_limit(&id).await
}).await?;
```

**Benefits:**
- Extremely fast decisions
- Probabilistic revalidation balances freshness/performance
- Reduces load on rate limiter

## Performance Characteristics

### HashMapStore

| Operation | Time | Notes |
|-----------|------|-------|
| Get (no contention) | ~100ns | Direct HashMap lookup |
| Get (with writers) | ~10-100µs | Readers wait for write lock |
| Set | ~1-10µs | Exclusive lock blocks all ops |
| Eviction | ~1-50ms | Scans entire map under lock |
| Scalability | Peaks at 8 threads | RwLock contention |

### MokaStore

| Operation | Time | Notes |
|-----------|------|-------|
| Get (any load) | ~50-100ns | Lock-free |
| Set (any load) | ~100-200ns | Lock-free CAS |
| Eviction | Background | Never blocks operations |
| Scalability | Linear to 32+ threads | Lock-free design |

### RedisStore

| Operation | Time | Notes |
|-----------|------|-------|
| Get | ~1-5ms | Network roundtrip |
| Set | ~1-5ms | Network + TTL |
| Remove | ~1-5ms | Network roundtrip |

### SWR Operations

| Operation | Time | Notes |
|-----------|------|-------|
| SWR hit (fresh) | Store get time | No network/IO |
| SWR hit (stale) | Store get time | Return immediately, revalidate async |
| SWR miss | 0ms + origin time | Non-blocking, background cache |

## Configuration Recommendations

### High-Traffic API (millions of req/s)

**Use MokaStore** for optimal performance:

```rust
let moka = Arc::new(MokaStore::new(MokaStoreConfig {
    max_capacity: 100_000,
    time_to_live: Some(Duration::from_secs(30)),
    time_to_idle: None,
}));

let redis = Arc::new(RedisStore::new(config).await?);

let cache = Namespace::new(vec![moka, redis], 30_000, 180_000);
```

### Medium-Traffic App (thousands of req/s)

**Either store works, MokaStore recommended:**

```rust
let moka = Arc::new(MokaStore::new(MokaStoreConfig {
    max_capacity: 10_000,
    time_to_live: Some(Duration::from_secs(60)),
    time_to_idle: None,
}));

let redis = Arc::new(RedisStore::new(config).await?);

let cache = Namespace::new(vec![moka, redis], 60_000, 600_000);
```

### Single-Service App (hundreds of req/s)

**HashMapStore is fine:**

```rust
let hashmap = Arc::new(HashMapStore::new(HashMapStoreConfig {
    evict_on_set: Some(EvictOnSetConfig {
        frequency: 0.5,
        max_items: 1_000,
    }),
}));

let cache = Namespace::new(vec![hashmap], 120_000, 600_000);
```

## Benchmarking

The library includes comprehensive benchmarks to compare performance of different store configurations under various workloads.

### Running Benchmarks

#### Quick Start (Local Redis)

```bash
# Start Redis
./scripts/start-redis.sh

# Run all benchmarks
cargo bench --bench cache_benchmark

# View results
open target/criterion/report/index.html
```

#### With Remote Redis

Test against a production Redis instance:

```bash
export REDIS_URL="redis://user:password@prod-redis.example.com:6379"
cargo bench --bench cache_benchmark
```

#### With Redis on Different Port

```bash
export REDIS_URL="redis://localhost:6380"
cargo bench --bench cache_benchmark
```

#### With Redis Cluster

```bash
export REDIS_URL="redis://node1:6379,node2:6379,node3:6379"
cargo bench --bench cache_benchmark
```

### Configuration Options

Control benchmark behavior with environment variables:

```bash
# Adjust database query latency (default: 50ms)
DB_LATENCY_MS=100 cargo bench --bench cache_benchmark

# Adjust sample size (default: 100)
BENCH_SAMPLE_SIZE=200 cargo bench --bench cache_benchmark

# Combined configuration
REDIS_URL=redis://remote:6379 DB_LATENCY_MS=25 cargo bench
```

### Benchmark Scenarios

The suite includes three scenarios:

1. **`hot_cache`** - All requests hit L1 cache (pure read performance)
   - Measures: Throughput and latency under different thread counts (1, 2, 4, 8, 16)
   - Best for: Comparing raw cache performance

2. **`cold_cache`** - All requests miss cache and load from origin
   - Measures: Origin load handling and cache population
   - Best for: Understanding initial cache warmup behavior

3. **`mixed_workload`** - 80% cache hits, 20% misses (realistic)
   - Measures: Real-world performance with hot and cold data
   - Best for: Making production store decisions

### Running Specific Benchmarks

```bash
# Run only hot cache benchmarks
cargo bench --bench cache_benchmark -- hot_cache

# Run only mixed workload
cargo bench --bench cache_benchmark -- mixed_workload

# Run with longer warm-up time
cargo bench --bench cache_benchmark -- --warm-up-time 10
```

### Interpreting Results

Example output:

```
hot_cache/hashmap_redis/8   time:   [45.2 µs 47.1 µs 49.3 µs]
                            thrpt:  [2.03M elem/s 2.12M elem/s 2.21M elem/s]

hot_cache/moka_redis/8      time:   [12.3 µs 13.1 µs 14.2 µs]
                            thrpt:  [7.04M elem/s 7.63M elem/s 8.13M elem/s]
```

This shows MokaStore is ~3.6x faster than HashMapStore at 8 threads for hot cache reads.

### CI/CD Integration

Skip benchmarks in CI if Redis is unavailable:

```yaml
# In .github/workflows/bench.yml
- name: Run Benchmarks
  if: env.REDIS_AVAILABLE == 'true'
  run: cargo bench --bench cache_benchmark
```

## Future Enhancements

- [ ] Metrics/observability: hit rates, eviction counts
- [ ] Per-namespace type safety with GATs
- [ ] Distributed cache invalidation
- [ ] Cache warming strategies

## License

MIT

## Contributing

Contributions welcome! Please ensure:
- Tests pass: `cargo test`
- Clippy clean: `cargo clippy -- -D warnings`
- Formatted: `cargo fmt`
