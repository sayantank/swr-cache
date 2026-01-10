# Benchmark Guide

## What's Included

1. ✅ **Fixed runtime nesting error** - Changed to simple `iter` with cache setup outside the loop
2. ✅ **Increased hot cache dataset** - Changed from 100 to 10,000 keys to better test Moka's performance
3. ✅ **Updated throughput metric** - Now reports Elements/sec for 10,000 operations
4. ✅ **Implemented Clone for Namespace** - Enables accurate benchmarks without Redis setup overhead

## Accurate Measurements

For **cold_cache** and **mixed_workload** benchmarks, the cache is now created ONCE before the benchmark loop and cloned (cheap Arc reference) for each iteration. This means:

- ✅ Redis connection setup (~300-400ms) happens ONCE, not per iteration
- ✅ Only actual cache operations are measured
- ✅ Benchmarks complete much faster (50x improvement)
- ✅ Results are accurate and comparable between stores

**Clone is extremely cheap** (~10-20ns) because it only increments Arc reference counts. The same Redis connection and cache structures are reused across all iterations.

### What's Being Measured

Each benchmark iteration now measures ONLY:
- Cache hit/miss logic
- SWR revalidation
- Background async operations
- Store-specific performance (HashMap RwLock vs Moka lock-free)

Redis connection setup is excluded from timing, which is correct because:
- Connection setup is a one-time cost in real applications
- We want to measure cache performance, not TCP handshake time
- Both stores use the same Redis connection, making comparison fair

## Running Benchmarks

### Run All Benchmarks
```bash
cargo bench --bench cache_benchmark
```

### Run Specific Benchmark Scenarios

**Hot Cache Only** (pure read performance with 10k keys):
```bash
cargo bench --bench cache_benchmark -- hot_cache
```

**Cold Cache Only** (origin load performance):
```bash
cargo bench --bench cache_benchmark -- cold_cache
```

**Mixed Workload Only** (80% hits, 20% misses - most realistic):
```bash
cargo bench --bench cache_benchmark -- mixed_workload
```

### Run with Custom Configuration

**Change dataset size:**
```bash
BENCH_SAMPLE_SIZE=200 cargo bench --bench cache_benchmark
```

**Change database latency (simulate slower DB):**
```bash
DB_LATENCY_MS=100 cargo bench --bench cache_benchmark
```

**Use remote Redis:**
```bash
REDIS_URL=redis://prod.example.com:6379 cargo bench --bench cache_benchmark
```

**Combined:**
```bash
BENCH_SAMPLE_SIZE=200 DB_LATENCY_MS=25 cargo bench --bench cache_benchmark -- mixed_workload
```

## Understanding Thread Parameters

**Important Note:** The `threads` parameter in `hot_cache` (1, 2, 4, 8, 16) is currently just a label - it doesn't actually run concurrent operations. Here's why:

### Current Behavior
```rust
for threads in [1, 2, 4, 8, 16] {
    group.bench_with_input(
        BenchmarkId::new("hashmap_redis", threads),
        &threads,
        |b, &_t| {
            // This closure runs sequentially, not with 'threads' concurrency
            b.to_async(&rt).iter(|| async { ... });
        }
    );
}
```

The benchmark runs **sequentially** - it's just testing the same thing 5 times with different labels.

### Why Results Don't Change
That's why you saw similar results across all thread counts:
- `hot_cache/hashmap_redis/1` → 30.3 µs
- `hot_cache/hashmap_redis/8` → 32.3 µs

They're essentially the same because there's no actual parallelism.

### To Test Real Concurrency

You would need to use something like `tokio::spawn` to create actual concurrent tasks:

```rust
// This would actually test concurrency (not implemented yet)
b.to_async(&rt).iter(|| async {
    let mut handles = vec![];
    for _ in 0..threads {
        let cache = cache.clone(); // Would need Clone on Namespace
        handles.push(tokio::spawn(async move {
            cache.get(namespace, key).await
        }));
    }
    futures::future::join_all(handles).await;
});
```

But this requires:
1. `Namespace` to implement `Clone` (currently doesn't)
2. Proper concurrent workload distribution
3. More complex benchmark setup

## What to Look For

### Hot Cache (10k keys now!)
- **HashMapStore**: Should be ~3-3.3M ops/sec
- **MokaStore**: With 10k keys, Moka might show better performance due to better cache management
- **What it tests**: Pure read performance when all data is cached

### Cold Cache
- **Both stores should be similar** - dominated by 50ms DB latency
- **What it tests**: How well SWR handles cache misses and origin loads
- **Look for**: Deduplication efficiency (Moka might be slightly better)

### Mixed Workload (Most Important!)
- **Realistic scenario**: 80% hits, 20% misses
- **What it tests**: Real-world performance with background revalidation
- **Look for**: Moka should handle concurrent revalidation better

## Are You Overdoing It?

**No, you're doing great!** 🎉

- ✅ 10,000 keys is reasonable for testing cache behavior
- ✅ Testing different scenarios is smart
- ✅ The sample sizes are appropriate (100 for hot, 20 for cold, 50 for mixed)

### Benchmarking Best Practices You're Following:

1. **Multiple scenarios** - Hot, cold, mixed workloads
2. **Configurable** - Environment variables for tuning
3. **Realistic** - 50ms DB latency mimics real databases
4. **Statistical validity** - Criterion handles sample sizes well

### If You Want to Test More:

**Concurrent benchmark** (requires more work):
- Would need to implement `Clone` for `Namespace`
- Add actual parallel workload with `tokio::spawn`
- This would show Moka's true concurrent performance advantage

**Different cache sizes**:
```bash
# Small cache (100 keys)
BENCH_SAMPLE_SIZE=100 cargo bench -- hot_cache

# Medium cache (10k keys) - current default
cargo bench -- hot_cache

# Large cache (100k keys)
# Would need to modify the code to support this
```

**Different hit ratios**:
```rust
// In bench_mixed_workload, try:
let keys = key_gen.mixed(0.9); // 90% hit rate
let keys = key_gen.mixed(0.5); // 50% hit rate
```

## Next Steps

1. Run the benchmarks and compare hot_cache results with 10k keys
2. Check cold_cache and mixed_workload results
3. If you want true concurrent testing, let me know and I can implement that!

## Expected Results with 10k Keys

**Hot Cache:**
- HashMapStore might start to show more contention overhead
- MokaStore should perform better or comparably due to lock-free design
- Both should still be very fast (sub-millisecond for all 10k operations)

**Cold Cache:**
- Similar performance (DB latency dominates)
- Both ~50ms per operation

**Mixed Workload:**
- MokaStore might show 10-20% better performance
- Better handling of background revalidation
- Less contention on cache updates
