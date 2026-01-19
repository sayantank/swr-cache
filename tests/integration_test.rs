//! Integration tests for swr-cache SWR functionality with Memory and Redis stores.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use swr_cache::{
    Cache, Entry, HashMapStore, HashMapStoreConfig, MokaStore, MokaStoreConfig, RedisStore,
    RedisStoreConfig, Store, StoredEntry, TieredStore,
};

// ============================================================================
// Test Types
// ============================================================================

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
struct User {
    id: u64,
    name: String,
    email: String,
}

// ============================================================================
// Fake Database
// ============================================================================

fn fake_user_db() -> HashMap<String, User> {
    let mut db = HashMap::new();
    db.insert(
        "user:1".into(),
        User {
            id: 1,
            name: "Alice".into(),
            email: "alice@example.com".into(),
        },
    );
    db.insert(
        "user:2".into(),
        User {
            id: 2,
            name: "Bob".into(),
            email: "bob@example.com".into(),
        },
    );
    db.insert(
        "user:3".into(),
        User {
            id: 3,
            name: "Charlie".into(),
            email: "charlie@example.com".into(),
        },
    );
    db
}

// ============================================================================
// Helper Functions
// ============================================================================

fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_millis() as i64
}

async fn create_redis_store() -> RedisStore {
    let config = RedisStoreConfig {
        url: "redis://localhost:6379".to_string(),
        disable_expiration: false,
    };
    RedisStore::new(config)
        .await
        .expect("Failed to connect to Redis - is it running?")
}

// ============================================================================
// HashMap Store Tests
// ============================================================================

#[tokio::test]
async fn test_hashmap_store_swr_cache_miss_loads_from_origin() {
    let store: Arc<dyn Store> = Arc::new(HashMapStore::new(HashMapStoreConfig::default()));
    let cache: Cache<User> = Cache::new("users", vec![store], 60_000, 300_000);

    let db = fake_user_db();
    let call_count = Arc::new(AtomicUsize::new(0));
    let call_count_clone = call_count.clone();

    let result = cache
        .swr("user:1", move |key| {
            let db = db.clone();
            let count = call_count_clone.clone();
            async move {
                count.fetch_add(1, Ordering::SeqCst);
                db.get(&key).cloned()
            }
        })
        .await
        .unwrap();

    assert!(result.is_some());
    let user = result.unwrap();
    assert_eq!(user.name, "Alice");
    assert_eq!(call_count.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn test_hashmap_store_swr_cache_hit_does_not_call_origin() {
    let store: Arc<dyn Store> = Arc::new(HashMapStore::new(HashMapStoreConfig::default()));
    let cache = Cache::new("users", vec![store], 60_000, 300_000);

    let db = fake_user_db();
    let call_count = Arc::new(AtomicUsize::new(0));

    // First call - cache miss
    let call_count_clone = call_count.clone();
    let db_clone = db.clone();
    let _ = cache
        .swr("user:2", move |key| {
            let db = db_clone.clone();
            let count = call_count_clone.clone();
            async move {
                count.fetch_add(1, Ordering::SeqCst);
                db.get(&key).cloned()
            }
        })
        .await
        .unwrap();

    // Wait for background cache set
    tokio::time::sleep(tokio::time::Duration::from_millis(50)).await;

    // Second call - cache hit
    let call_count_clone = call_count.clone();
    let db_clone = db.clone();
    let result = cache
        .swr("user:2", move |key| {
            let db = db_clone.clone();
            let count = call_count_clone.clone();
            async move {
                count.fetch_add(1, Ordering::SeqCst);
                db.get(&key).cloned()
            }
        })
        .await
        .unwrap();

    assert!(result.is_some());
    assert_eq!(result.unwrap().name, "Bob");
    // Origin should only have been called once
    assert_eq!(call_count.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn test_hashmap_store_get_set_remove() {
    let store: Arc<dyn Store> = Arc::new(HashMapStore::new(HashMapStoreConfig::default()));
    let cache = Cache::new("users", vec![store], 60_000, 300_000);

    let user = User {
        id: 99,
        name: "Test User".into(),
        email: "test@example.com".into(),
    };

    // Set
    cache.set("user:99", user.clone()).await.unwrap();

    // Get
    let result = cache.get("user:99").await.unwrap();
    assert_eq!(result, Some(user));

    // Remove
    cache.remove("user:99").await.unwrap();

    // Get after remove
    let result = cache.get("user:99").await.unwrap();
    assert!(result.is_none());
}

// ============================================================================
// Moka Store Tests
// ============================================================================

#[tokio::test]
async fn test_moka_store_swr_cache_miss_loads_from_origin() {
    let store: Arc<dyn Store> = Arc::new(MokaStore::new(MokaStoreConfig::default()));
    let cache = Cache::new("users", vec![store], 60_000, 300_000);

    let db = fake_user_db();
    let call_count = Arc::new(AtomicUsize::new(0));
    let call_count_clone = call_count.clone();

    let result = cache
        .swr("user:1", move |key| {
            let db = db.clone();
            let count = call_count_clone.clone();
            async move {
                count.fetch_add(1, Ordering::SeqCst);
                db.get(&key).cloned()
            }
        })
        .await
        .unwrap();

    assert!(result.is_some());
    let user = result.unwrap();
    assert_eq!(user.name, "Alice");
    assert_eq!(call_count.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn test_moka_store_swr_cache_hit_does_not_call_origin() {
    let store: Arc<dyn Store> = Arc::new(MokaStore::new(MokaStoreConfig::default()));
    let cache = Cache::new("users", vec![store], 60_000, 300_000);

    let db = fake_user_db();
    let call_count = Arc::new(AtomicUsize::new(0));

    // First call - cache miss
    let call_count_clone = call_count.clone();
    let db_clone = db.clone();
    let _ = cache
        .swr("user:2", move |key| {
            let db = db_clone.clone();
            let count = call_count_clone.clone();
            async move {
                count.fetch_add(1, Ordering::SeqCst);
                db.get(&key).cloned()
            }
        })
        .await
        .unwrap();

    // Wait for background cache set
    tokio::time::sleep(tokio::time::Duration::from_millis(50)).await;

    // Second call - cache hit
    let call_count_clone = call_count.clone();
    let db_clone = db.clone();
    let result = cache
        .swr("user:2", move |key| {
            let db = db_clone.clone();
            let count = call_count_clone.clone();
            async move {
                count.fetch_add(1, Ordering::SeqCst);
                db.get(&key).cloned()
            }
        })
        .await
        .unwrap();

    assert!(result.is_some());
    assert_eq!(result.unwrap().name, "Bob");
    // Origin should only have been called once
    assert_eq!(call_count.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn test_moka_store_get_set_remove() {
    let store: Arc<dyn Store> = Arc::new(MokaStore::new(MokaStoreConfig::default()));
    let cache = Cache::new("users", vec![store], 60_000, 300_000);

    let user = User {
        id: 99,
        name: "Moka Test User".into(),
        email: "moka@example.com".into(),
    };

    // Set
    cache.set("user:99", user.clone()).await.unwrap();

    // Get
    let result = cache.get("user:99").await.unwrap();
    assert_eq!(result, Some(user));

    // Remove
    cache.remove("user:99").await.unwrap();

    // Get after remove
    let result = cache.get("user:99").await.unwrap();
    assert!(result.is_none());
}

// ============================================================================
// Redis Store Tests
// ============================================================================

#[tokio::test]
async fn test_redis_store_swr_cache_miss_loads_from_origin() {
    let store: Arc<dyn Store> = Arc::new(create_redis_store().await);
    let cache = Cache::new("users", vec![store], 60_000, 300_000);

    // Use unique key to avoid conflicts with other tests
    let test_key = format!("user:redis_test_{}", now_ms());

    let db = fake_user_db();
    let call_count = Arc::new(AtomicUsize::new(0));
    let call_count_clone = call_count.clone();

    let result = cache
        .swr(&test_key, move |_key| {
            let db = db.clone();
            let count = call_count_clone.clone();
            async move {
                count.fetch_add(1, Ordering::SeqCst);
                // Return user:1 for any test key
                db.get("user:1").cloned()
            }
        })
        .await
        .unwrap();

    assert!(result.is_some());
    let user = result.unwrap();
    assert_eq!(user.name, "Alice");
    assert_eq!(call_count.load(Ordering::SeqCst), 1);

    // Cleanup
    cache.remove(&test_key).await.unwrap();
}

#[tokio::test]
async fn test_redis_store_swr_cache_hit_does_not_call_origin() {
    let store: Arc<dyn Store> = Arc::new(create_redis_store().await);
    let cache = Cache::new("users", vec![store], 60_000, 300_000);

    let test_key = format!("user:redis_hit_test_{}", now_ms());

    let db = fake_user_db();
    let call_count = Arc::new(AtomicUsize::new(0));

    // First call - cache miss
    let call_count_clone = call_count.clone();
    let db_clone = db.clone();
    let _ = cache
        .swr(&test_key, move |_key| {
            let db = db_clone.clone();
            let count = call_count_clone.clone();
            async move {
                count.fetch_add(1, Ordering::SeqCst);
                db.get("user:2").cloned()
            }
        })
        .await
        .unwrap();

    // Wait for background cache set
    tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;

    // Second call - cache hit
    let call_count_clone = call_count.clone();
    let db_clone = db.clone();
    let result = cache
        .swr(&test_key, move |_key| {
            let db = db_clone.clone();
            let count = call_count_clone.clone();
            async move {
                count.fetch_add(1, Ordering::SeqCst);
                db.get("user:2").cloned()
            }
        })
        .await
        .unwrap();

    assert!(result.is_some());
    assert_eq!(result.unwrap().name, "Bob");
    // Origin should only have been called once
    assert_eq!(call_count.load(Ordering::SeqCst), 1);

    // Cleanup
    cache.remove(&test_key).await.unwrap();
}

#[tokio::test]
async fn test_redis_store_get_set_remove() {
    let store: Arc<dyn Store> = Arc::new(create_redis_store().await);
    let cache = Cache::new("users", vec![store], 60_000, 300_000);

    let test_key = format!("user:redis_crud_{}", now_ms());

    let user = User {
        id: 100,
        name: "Redis Test User".into(),
        email: "redis@example.com".into(),
    };

    // Set
    cache.set(&test_key, user.clone()).await.unwrap();

    // Get
    let result = cache.get(&test_key).await.unwrap();
    assert_eq!(result, Some(user));

    // Remove
    cache.remove(&test_key).await.unwrap();

    // Get after remove
    let result = cache.get(&test_key).await.unwrap();
    assert!(result.is_none());
}

// ============================================================================
// Tiered Store Tests (Memory L1 + Redis L2)
// ============================================================================

#[tokio::test]
async fn test_tiered_store_l2_hit_populates_l1() {
    let hashmap_store: Arc<dyn Store> = Arc::new(HashMapStore::new(HashMapStoreConfig::default()));
    let redis_store: Arc<dyn Store> = Arc::new(create_redis_store().await);

    let test_key = format!("user:tiered_test_{}", now_ms());

    // Set value directly in Redis (L2)
    let user = User {
        id: 200,
        name: "Tiered User".into(),
        email: "tiered@example.com".into(),
    };
    let now = now_ms();
    let stored_entry = StoredEntry::from_serialized(
        serde_json::to_string(&Entry::new(user.clone(), now + 60_000, now + 300_000)).unwrap(),
        now + 60_000,
        now + 300_000,
    );
    redis_store
        .set("users", &test_key, stored_entry)
        .await
        .unwrap();

    // Memory (L1) should be empty
    let l1_result = hashmap_store.get("users", &test_key).await.unwrap();
    assert!(l1_result.is_none());

    // Create tiered store: HashMap (L1) -> Redis (L2)
    let tiered: Arc<dyn Store> = Arc::new(TieredStore::from_stores(vec![
        hashmap_store.clone(),
        redis_store.clone(),
    ]));

    // Get from tiered - should find in L2 (Redis)
    let result = tiered.get("users", &test_key).await.unwrap();
    assert!(result.is_some());
    let typed_result: Entry<User> = result.unwrap().into_typed().unwrap();
    assert_eq!(typed_result.value.name, "Tiered User");

    // Wait for background L1 population
    tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;

    // Memory (L1) should now have the value
    let l1_result = hashmap_store.get("users", &test_key).await.unwrap();
    assert!(l1_result.is_some());
    let typed_l1: Entry<User> = l1_result.unwrap().into_typed().unwrap();
    assert_eq!(typed_l1.value.name, "Tiered User");

    // Cleanup
    redis_store.remove("users", &[&test_key]).await.unwrap();
}

#[tokio::test]
async fn test_tiered_store_with_namespace_swr() {
    let hashmap_store: Arc<dyn Store> = Arc::new(HashMapStore::new(HashMapStoreConfig::default()));
    let redis_store: Arc<dyn Store> = Arc::new(create_redis_store().await);

    let cache = Cache::new(
        "users",
        vec![hashmap_store.clone(), redis_store.clone()],
        60_000,
        300_000,
    );

    let test_key = format!("user:tiered_swr_{}", now_ms());

    let db = fake_user_db();
    let call_count = Arc::new(AtomicUsize::new(0));

    // First call - cache miss, loads from origin
    let call_count_clone = call_count.clone();
    let db_clone = db.clone();
    let result = cache
        .swr(&test_key, move |_key| {
            let db = db_clone.clone();
            let count = call_count_clone.clone();
            async move {
                count.fetch_add(1, Ordering::SeqCst);
                db.get("user:3").cloned()
            }
        })
        .await
        .unwrap();

    assert!(result.is_some());
    assert_eq!(result.unwrap().name, "Charlie");
    assert_eq!(call_count.load(Ordering::SeqCst), 1);

    // Wait for caching
    tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;

    // Second call - should hit cache (L1 or L2)
    let call_count_clone = call_count.clone();
    let db_clone = db.clone();
    let result = cache
        .swr(&test_key, move |_key| {
            let db = db_clone.clone();
            let count = call_count_clone.clone();
            async move {
                count.fetch_add(1, Ordering::SeqCst);
                db.get("user:3").cloned()
            }
        })
        .await
        .unwrap();

    assert!(result.is_some());
    assert_eq!(result.unwrap().name, "Charlie");
    // Origin should still only have been called once
    assert_eq!(call_count.load(Ordering::SeqCst), 1);

    // Cleanup
    cache.remove(&test_key).await.unwrap();
}

// ============================================================================
// Redis Store No-Expiration Tests
// ============================================================================

#[tokio::test]
async fn test_redis_store_disable_expiration_preserves_stale_data() {
    // Create Redis store with expiration disabled
    let config = RedisStoreConfig {
        url: "redis://localhost:6379".to_string(),
        disable_expiration: true,
    };
    let store: Arc<dyn Store> = Arc::new(
        RedisStore::new(config)
            .await
            .expect("Failed to connect to Redis"),
    );

    let test_key = format!("user:no_expire_{}", now_ms());

    let user = User {
        id: 300,
        name: "Stale User".into(),
        email: "stale@example.com".into(),
    };

    // Set entry with very short TTL (1 second stale_until)
    let now = now_ms();
    let stored_entry = StoredEntry::from_serialized(
        serde_json::to_string(&Entry::new(user.clone(), now - 1000, now + 1000)).unwrap(),
        now - 1000,
        now + 1000,
    );
    store.set("users", &test_key, stored_entry).await.unwrap();

    // Wait for stale_until to pass
    tokio::time::sleep(tokio::time::Duration::from_millis(1500)).await;

    // With disable_expiration=true, the entry should still exist in Redis
    // and get() should return it for fallback purposes
    let result = store.get("users", &test_key).await.unwrap();
    assert!(
        result.is_some(),
        "get() should return expired entry when disable_expiration is true"
    );
    let typed_result: Entry<User> = result.unwrap().into_typed().unwrap();
    assert_eq!(
        typed_result.value.name, "Stale User",
        "Expired entry value should match"
    );

    // However, if we query Redis directly, the data should still be there
    // Let's verify by setting a new entry and checking it persists
    let now = now_ms();
    let stored_entry2 = StoredEntry::from_serialized(
        serde_json::to_string(&Entry::new(user.clone(), now + 60_000, now + 300_000)).unwrap(),
        now + 60_000,
        now + 300_000,
    );
    store.set("users", &test_key, stored_entry2).await.unwrap();

    let result = store.get("users", &test_key).await.unwrap();
    assert!(result.is_some(), "Fresh entry should be retrievable");

    // Cleanup
    store.remove("users", &[&test_key]).await.unwrap();
}

#[tokio::test]
async fn test_redis_store_disable_expiration_with_swr_resilience() {
    // Create Redis store with expiration disabled
    let config = RedisStoreConfig {
        url: "redis://localhost:6379".to_string(),
        disable_expiration: true,
    };
    let store: Arc<dyn Store> = Arc::new(
        RedisStore::new(config)
            .await
            .expect("Failed to connect to Redis"),
    );

    let cache = Cache::new("users", vec![store.clone()], 100, 1000); // 100ms fresh, 1000ms stale

    let test_key = format!("user:swr_resilient_{}", now_ms());

    let user = User {
        id: 400,
        name: "Resilient User".into(),
        email: "resilient@example.com".into(),
    };

    let call_count = Arc::new(AtomicUsize::new(0));

    // First call - cache miss, loads from origin
    let call_count_clone = call_count.clone();
    let user_clone = user.clone();
    let result = cache
        .swr(&test_key, move |_key| {
            let count = call_count_clone.clone();
            let u = user_clone.clone();
            async move {
                count.fetch_add(1, Ordering::SeqCst);
                Some(u)
            }
        })
        .await
        .unwrap();

    assert_eq!(result.as_ref().unwrap().name, "Resilient User");
    assert_eq!(call_count.load(Ordering::SeqCst), 1);

    // Wait for background caching
    tokio::time::sleep(tokio::time::Duration::from_millis(50)).await;

    // Wait for data to become stale (but not expired)
    tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;

    // Second call - data is stale, should return stale data and revalidate
    // Origin fails this time
    let call_count_clone = call_count.clone();
    let result = cache
        .swr(&test_key, move |_key| {
            let count = call_count_clone.clone();
            async move {
                count.fetch_add(1, Ordering::SeqCst);
                None // Origin fails!
            }
        })
        .await
        .unwrap();

    // Should still get the stale data
    assert!(result.is_some(), "Stale data should be returned");
    assert_eq!(result.unwrap().name, "Resilient User");

    // Wait for background revalidation to complete (which fails)
    tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;

    assert_eq!(call_count.load(Ordering::SeqCst), 2);

    // Wait for stale_until to pass (1000ms total)
    tokio::time::sleep(tokio::time::Duration::from_millis(900)).await;

    // Third call - data has expired, but with disable_expiration it's still in Redis
    // Even though it's expired, it won't be auto-deleted from Redis
    // However, the application layer will consider it expired and try to load from origin
    let call_count_clone = call_count.clone();
    let user_clone = user.clone();
    let result = cache
        .swr(&test_key, move |_key| {
            let count = call_count_clone.clone();
            let u = user_clone.clone();
            async move {
                count.fetch_add(1, Ordering::SeqCst);
                Some(u) // Origin succeeds again
            }
        })
        .await
        .unwrap();

    // Should get data from origin
    assert_eq!(result.as_ref().unwrap().name, "Resilient User");
    assert_eq!(call_count.load(Ordering::SeqCst), 3);

    // Cleanup
    cache.remove(&test_key).await.unwrap();
}

#[tokio::test]
async fn test_redis_store_with_expiration_enabled_deletes_stale_data() {
    // Create Redis store with expiration ENABLED (default behavior)
    let config = RedisStoreConfig {
        url: "redis://localhost:6379".to_string(),
        disable_expiration: false,
    };
    let store: Arc<dyn Store> = Arc::new(
        RedisStore::new(config)
            .await
            .expect("Failed to connect to Redis"),
    );

    let test_key = format!("user:with_expire_{}", now_ms());

    let user = User {
        id: 500,
        name: "Expiring User".into(),
        email: "expiring@example.com".into(),
    };

    // Set entry with very short TTL (2 second stale_until)
    let now = now_ms();
    let stored_entry = StoredEntry::from_serialized(
        serde_json::to_string(&Entry::new(user.clone(), now + 100, now + 2000)).unwrap(),
        now + 100,
        now + 2000,
    );
    store.set("users", &test_key, stored_entry).await.unwrap();

    // Immediately after setting, should be retrievable
    let result = store.get("users", &test_key).await.unwrap();
    assert!(result.is_some(), "Fresh entry should be retrievable");

    // Wait for Redis TTL to expire (2+ seconds)
    tokio::time::sleep(tokio::time::Duration::from_millis(2500)).await;

    // With disable_expiration=false, Redis should have auto-deleted the entry
    let result = store.get("users", &test_key).await.unwrap();
    assert!(
        result.is_none(),
        "Entry should be auto-deleted by Redis after TTL"
    );

    // Cleanup (in case it still exists)
    let _ = store.remove("users", &[&test_key]).await;
}

#[tokio::test]
async fn test_redis_no_expire_returns_expired_data_when_origin_fails() {
    let config = RedisStoreConfig {
        url: "redis://localhost:6379".to_string(),
        disable_expiration: true,
    };
    let store: Arc<dyn Store> = Arc::new(
        RedisStore::new(config)
            .await
            .expect("Failed to connect to Redis"),
    );
    let cache = Cache::new("users", vec![store], 100, 500); // 100ms fresh, 500ms stale

    let test_key = format!("user:fallback_test_{}", now_ms());
    let user = User {
        id: 999,
        name: "Fallback User".into(),
        email: "fallback@example.com".into(),
    };

    // 1. Initial load from origin
    let call_count = Arc::new(AtomicUsize::new(0));
    let call_count_clone = call_count.clone();
    let user_clone = user.clone();

    let result = cache
        .swr(&test_key, move |_| {
            let count = call_count_clone.clone();
            let u = user_clone.clone();
            async move {
                count.fetch_add(1, Ordering::SeqCst);
                Some(u)
            }
        })
        .await
        .unwrap();

    assert_eq!(result.as_ref().unwrap().name, "Fallback User");
    assert_eq!(call_count.load(Ordering::SeqCst), 1);

    // Wait for background caching
    tokio::time::sleep(tokio::time::Duration::from_millis(50)).await;

    // 2. Wait for data to expire (>500ms)
    tokio::time::sleep(tokio::time::Duration::from_millis(500)).await;

    // 3. Origin fails, should return expired data as fallback
    let call_count_clone = call_count.clone();
    let result = cache
        .swr(&test_key, move |_| {
            let count = call_count_clone.clone();
            async move {
                count.fetch_add(1, Ordering::SeqCst);
                None // Origin fails!
            }
        })
        .await
        .unwrap();

    // Should still get the expired data
    assert!(
        result.is_some(),
        "Expired data should be returned as fallback"
    );
    assert_eq!(result.unwrap().name, "Fallback User");
    assert_eq!(call_count.load(Ordering::SeqCst), 2);

    // 4. Origin succeeds, should return fresh data
    let call_count_clone = call_count.clone();
    let updated_user = User {
        id: 999,
        name: "Updated User".into(),
        email: "updated@example.com".into(),
    };
    let updated_clone = updated_user.clone();

    let result = cache
        .swr(&test_key, move |_| {
            let count = call_count_clone.clone();
            let u = updated_clone.clone();
            async move {
                count.fetch_add(1, Ordering::SeqCst);
                Some(u)
            }
        })
        .await
        .unwrap();

    assert_eq!(result.as_ref().unwrap().name, "Updated User");
    assert_eq!(call_count.load(Ordering::SeqCst), 3);

    // Cleanup
    cache.remove(&test_key).await.unwrap();
}
