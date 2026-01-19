use criterion::{BenchmarkId, Criterion, Throughput, black_box, criterion_group, criterion_main};
use std::sync::Arc;
use std::time::Duration;
use swr_cache::{
    Cache, HashMapStore, HashMapStoreConfig, MokaStore, MokaStoreConfig, RedisStore,
    RedisStoreConfig, Store,
};
use tokio::runtime::Runtime;

mod common;
use common::{BenchConfig, BenchUser, FakeDatabase, KeyGenerator};

/// Setup HashMapStore + Redis tier
async fn setup_hashmap_redis(redis_url: &str) -> Cache<BenchUser> {
    let hashmap = Arc::new(HashMapStore::new(HashMapStoreConfig::default()));
    let redis_config = RedisStoreConfig {
        url: redis_url.to_string(),
        disable_expiration: false,
    };
    let redis: Arc<dyn Store> = Arc::new(
        RedisStore::new(redis_config)
            .await
            .expect("Redis connection failed"),
    );

    Cache::new("users", vec![hashmap, redis], 60_000, 300_000)
}

/// Setup MokaStore + Redis tier
async fn setup_moka_redis(redis_url: &str) -> Cache<BenchUser> {
    let moka = Arc::new(MokaStore::new(MokaStoreConfig {
        max_capacity: 10_000,
        time_to_live: None,
        time_to_idle: None,
    }));
    let redis_config = RedisStoreConfig {
        url: redis_url.to_string(),
        disable_expiration: false,
    };
    let redis: Arc<dyn Store> = Arc::new(
        RedisStore::new(redis_config)
            .await
            .expect("Redis connection failed"),
    );

    Cache::new("users", vec![moka, redis], 60_000, 300_000)
}

/// Benchmark 1: Hot Cache (all hits, pure cache read performance)
fn bench_hot_cache(c: &mut Criterion, config: &BenchConfig) {
    let rt = Runtime::new().unwrap();
    let mut group = c.benchmark_group("hot_cache");
    group.sample_size(config.sample_size);

    let db = FakeDatabase::new(10000, config.db_latency_ms);
    let keys = KeyGenerator::new(10000).sequential();

    for threads in [1, 2, 4, 8, 16] {
        group.throughput(Throughput::Elements(10000));

        // HashMapStore + Redis
        group.bench_with_input(
            BenchmarkId::new("hashmap_redis", threads),
            &threads,
            |b, &_t| {
                let cache = rt.block_on(setup_hashmap_redis(&config.redis_url));
                let db = db.clone();
                let keys = keys.clone();

                // Pre-populate cache
                rt.block_on(async {
                    for key in &keys {
                        let _ = cache
                            .swr(key, {
                                let db = db.clone();
                                move |k| {
                                    let db = db.clone();
                                    async move { db.get(&k).await }
                                }
                            })
                            .await;
                    }
                    tokio::time::sleep(Duration::from_millis(100)).await;
                });

                b.to_async(&rt).iter(|| async {
                    for key in &keys {
                        let _ = black_box(cache.get(key).await);
                    }
                });
            },
        );

        // MokaStore + Redis
        group.bench_with_input(
            BenchmarkId::new("moka_redis", threads),
            &threads,
            |b, &_t| {
                let cache = rt.block_on(setup_moka_redis(&config.redis_url));
                let db = db.clone();
                let keys = keys.clone();

                // Pre-populate cache
                rt.block_on(async {
                    for key in &keys {
                        let _ = cache
                            .swr(key, {
                                let db = db.clone();
                                move |k| {
                                    let db = db.clone();
                                    async move { db.get(&k).await }
                                }
                            })
                            .await;
                    }
                    tokio::time::sleep(Duration::from_millis(100)).await;
                });

                b.to_async(&rt).iter(|| async {
                    for key in &keys {
                        let _ = black_box(cache.get(key).await);
                    }
                });
            },
        );
    }

    group.finish();
}

/// Benchmark 2: Cold Cache (all misses, origin load performance)
fn bench_cold_cache(c: &mut Criterion, config: &BenchConfig) {
    let rt = Runtime::new().unwrap();
    let mut group = c.benchmark_group("cold_cache");
    group.sample_size(config.sample_size.min(20)); // Fewer samples due to origin latency
    group.measurement_time(Duration::from_secs(30));

    let db = FakeDatabase::new(1000, config.db_latency_ms);
    let key_gen = KeyGenerator::new(1000);

    // Create caches ONCE before benchmark loop
    let hashmap_cache = rt.block_on(setup_hashmap_redis(&config.redis_url));
    let moka_cache = rt.block_on(setup_moka_redis(&config.redis_url));

    // HashMapStore + Redis
    group.bench_function("hashmap_redis", |b| {
        b.to_async(&rt).iter(|| {
            let cache = hashmap_cache.clone();
            let db = db.clone();
            let keys = key_gen.sequential();
            async move {
                for key in keys.iter().take(10) {
                    let _ = black_box(
                        cache
                            .swr(key, {
                                let db = db.clone();
                                move |k| {
                                    let db = db.clone();
                                    async move { db.get(&k).await }
                                }
                            })
                            .await,
                    );
                }
            }
        });
    });

    // MokaStore + Redis
    group.bench_function("moka_redis", |b| {
        b.to_async(&rt).iter(|| {
            let cache = moka_cache.clone();
            let db = db.clone();
            let keys = key_gen.sequential();
            async move {
                for key in keys.iter().take(10) {
                    let _ = black_box(
                        cache
                            .swr(key, {
                                let db = db.clone();
                                move |k| {
                                    let db = db.clone();
                                    async move { db.get(&k).await }
                                }
                            })
                            .await,
                    );
                }
            }
        });
    });

    group.finish();
}

/// Benchmark 3: Mixed Workload (80% hits, 20% misses - realistic)
fn bench_mixed_workload(c: &mut Criterion, config: &BenchConfig) {
    let rt = Runtime::new().unwrap();
    let mut group = c.benchmark_group("mixed_workload");
    group.sample_size(config.sample_size.min(50));

    let db = FakeDatabase::new(500, config.db_latency_ms);
    let key_gen = KeyGenerator::new(500);

    // Create caches ONCE before benchmark loop
    let hashmap_cache = rt.block_on(setup_hashmap_redis(&config.redis_url));
    let moka_cache = rt.block_on(setup_moka_redis(&config.redis_url));

    // HashMapStore + Redis
    group.bench_function("hashmap_redis", |b| {
        b.to_async(&rt).iter(|| {
            let cache = hashmap_cache.clone();
            let db = db.clone();
            let keys = key_gen.mixed(0.8);
            async move {
                for key in keys.iter().take(50) {
                    let _ = black_box(
                        cache
                            .swr(key, {
                                let db = db.clone();
                                move |k| {
                                    let db = db.clone();
                                    async move { db.get(&k).await }
                                }
                            })
                            .await,
                    );
                }
            }
        });
    });

    // MokaStore + Redis
    group.bench_function("moka_redis", |b| {
        b.to_async(&rt).iter(|| {
            let cache = moka_cache.clone();
            let db = db.clone();
            let keys = key_gen.mixed(0.8);
            async move {
                for key in keys.iter().take(50) {
                    let _ = black_box(
                        cache
                            .swr(key, {
                                let db = db.clone();
                                move |k| {
                                    let db = db.clone();
                                    async move { db.get(&k).await }
                                }
                            })
                            .await,
                    );
                }
            }
        });
    });

    group.finish();
}

fn run_benchmarks(c: &mut Criterion) {
    let config = BenchConfig::new();

    eprintln!("\n=== Running Benchmarks ===\n");

    bench_hot_cache(c, &config);
    bench_cold_cache(c, &config);
    bench_mixed_workload(c, &config);
}

criterion_group!(benches, run_benchmarks);
criterion_main!(benches);
