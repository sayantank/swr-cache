//! Integration tests for swr-cache SWR functionality with Memory and Redis stores.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fmt::Display;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use swr_cache::{
    Entry, HashMapStore, HashMapStoreConfig, MokaStore, MokaStoreConfig, Namespace, RedisStore,
    RedisStoreConfig, Store, TieredStore,
};

// ============================================================================
// Test Types
// ============================================================================

#[derive(Clone, Eq, PartialEq, Hash, Debug)]
enum TestNamespace {
    Users,
    #[allow(dead_code)]
    Products,
}

impl Display for TestNamespace {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            TestNamespace::Users => write!(f, "users"),
            TestNamespace::Products => write!(f, "products"),
        }
    }
}

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

async fn create_redis_store() -> RedisStore<TestNamespace, User> {
    let config = RedisStoreConfig {
        url: "redis://localhost:6379".to_string(),
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
    let store: Arc<dyn Store<TestNamespace, User>> =
        Arc::new(HashMapStore::new(HashMapStoreConfig::default()));
    let cache = Namespace::new(vec![store], 60_000, 300_000);

    let db = fake_user_db();
    let call_count = Arc::new(AtomicUsize::new(0));
    let call_count_clone = call_count.clone();

    let result = cache
        .swr(TestNamespace::Users, "user:1", move |key| {
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
    let store: Arc<dyn Store<TestNamespace, User>> =
        Arc::new(HashMapStore::new(HashMapStoreConfig::default()));
    let cache = Namespace::new(vec![store], 60_000, 300_000);

    let db = fake_user_db();
    let call_count = Arc::new(AtomicUsize::new(0));

    // First call - cache miss
    let call_count_clone = call_count.clone();
    let db_clone = db.clone();
    let _ = cache
        .swr(TestNamespace::Users, "user:2", move |key| {
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
        .swr(TestNamespace::Users, "user:2", move |key| {
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
    let store: Arc<dyn Store<TestNamespace, User>> =
        Arc::new(HashMapStore::new(HashMapStoreConfig::default()));
    let cache = Namespace::new(vec![store], 60_000, 300_000);

    let user = User {
        id: 99,
        name: "Test User".into(),
        email: "test@example.com".into(),
    };

    // Set
    cache
        .set(TestNamespace::Users, "user:99", user.clone())
        .await
        .unwrap();

    // Get
    let result = cache.get(TestNamespace::Users, "user:99").await.unwrap();
    assert_eq!(result, Some(user));

    // Remove
    cache.remove(TestNamespace::Users, "user:99").await.unwrap();

    // Get after remove
    let result = cache.get(TestNamespace::Users, "user:99").await.unwrap();
    assert!(result.is_none());
}

// ============================================================================
// Moka Store Tests
// ============================================================================

#[tokio::test]
async fn test_moka_store_swr_cache_miss_loads_from_origin() {
    let store: Arc<dyn Store<TestNamespace, User>> =
        Arc::new(MokaStore::new(MokaStoreConfig::default()));
    let cache = Namespace::new(vec![store], 60_000, 300_000);

    let db = fake_user_db();
    let call_count = Arc::new(AtomicUsize::new(0));
    let call_count_clone = call_count.clone();

    let result = cache
        .swr(TestNamespace::Users, "user:1", move |key| {
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
    let store: Arc<dyn Store<TestNamespace, User>> =
        Arc::new(MokaStore::new(MokaStoreConfig::default()));
    let cache = Namespace::new(vec![store], 60_000, 300_000);

    let db = fake_user_db();
    let call_count = Arc::new(AtomicUsize::new(0));

    // First call - cache miss
    let call_count_clone = call_count.clone();
    let db_clone = db.clone();
    let _ = cache
        .swr(TestNamespace::Users, "user:2", move |key| {
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
        .swr(TestNamespace::Users, "user:2", move |key| {
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
    let store: Arc<dyn Store<TestNamespace, User>> =
        Arc::new(MokaStore::new(MokaStoreConfig::default()));
    let cache = Namespace::new(vec![store], 60_000, 300_000);

    let user = User {
        id: 99,
        name: "Moka Test User".into(),
        email: "moka@example.com".into(),
    };

    // Set
    cache
        .set(TestNamespace::Users, "user:99", user.clone())
        .await
        .unwrap();

    // Get
    let result = cache.get(TestNamespace::Users, "user:99").await.unwrap();
    assert_eq!(result, Some(user));

    // Remove
    cache.remove(TestNamespace::Users, "user:99").await.unwrap();

    // Get after remove
    let result = cache.get(TestNamespace::Users, "user:99").await.unwrap();
    assert!(result.is_none());
}

// ============================================================================
// Redis Store Tests
// ============================================================================

#[tokio::test]
async fn test_redis_store_swr_cache_miss_loads_from_origin() {
    let store: Arc<dyn Store<TestNamespace, User>> = Arc::new(create_redis_store().await);
    let cache = Namespace::new(vec![store], 60_000, 300_000);

    // Use unique key to avoid conflicts with other tests
    let test_key = format!("user:redis_test_{}", now_ms());

    let db = fake_user_db();
    let call_count = Arc::new(AtomicUsize::new(0));
    let call_count_clone = call_count.clone();

    let result = cache
        .swr(TestNamespace::Users, &test_key, move |_key| {
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
    cache.remove(TestNamespace::Users, &test_key).await.unwrap();
}

#[tokio::test]
async fn test_redis_store_swr_cache_hit_does_not_call_origin() {
    let store: Arc<dyn Store<TestNamespace, User>> = Arc::new(create_redis_store().await);
    let cache = Namespace::new(vec![store], 60_000, 300_000);

    let test_key = format!("user:redis_hit_test_{}", now_ms());

    let db = fake_user_db();
    let call_count = Arc::new(AtomicUsize::new(0));

    // First call - cache miss
    let call_count_clone = call_count.clone();
    let db_clone = db.clone();
    let _ = cache
        .swr(TestNamespace::Users, &test_key, move |_key| {
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
        .swr(TestNamespace::Users, &test_key, move |_key| {
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
    cache.remove(TestNamespace::Users, &test_key).await.unwrap();
}

#[tokio::test]
async fn test_redis_store_get_set_remove() {
    let store: Arc<dyn Store<TestNamespace, User>> = Arc::new(create_redis_store().await);
    let cache = Namespace::new(vec![store], 60_000, 300_000);

    let test_key = format!("user:redis_crud_{}", now_ms());

    let user = User {
        id: 100,
        name: "Redis Test User".into(),
        email: "redis@example.com".into(),
    };

    // Set
    cache
        .set(TestNamespace::Users, &test_key, user.clone())
        .await
        .unwrap();

    // Get
    let result = cache.get(TestNamespace::Users, &test_key).await.unwrap();
    assert_eq!(result, Some(user));

    // Remove
    cache.remove(TestNamespace::Users, &test_key).await.unwrap();

    // Get after remove
    let result = cache.get(TestNamespace::Users, &test_key).await.unwrap();
    assert!(result.is_none());
}

// ============================================================================
// Tiered Store Tests (Memory L1 + Redis L2)
// ============================================================================

#[tokio::test]
async fn test_tiered_store_l2_hit_populates_l1() {
    let hashmap_store: Arc<dyn Store<TestNamespace, User>> =
        Arc::new(HashMapStore::new(HashMapStoreConfig::default()));
    let redis_store: Arc<dyn Store<TestNamespace, User>> = Arc::new(create_redis_store().await);

    let test_key = format!("user:tiered_test_{}", now_ms());

    // Set value directly in Redis (L2)
    let user = User {
        id: 200,
        name: "Tiered User".into(),
        email: "tiered@example.com".into(),
    };
    let now = now_ms();
    let entry = Entry::new(user.clone(), now + 60_000, now + 300_000);
    redis_store
        .set(TestNamespace::Users, &test_key, entry)
        .await
        .unwrap();

    // Memory (L1) should be empty
    let l1_result = hashmap_store
        .get(TestNamespace::Users, &test_key)
        .await
        .unwrap();
    assert!(l1_result.is_none());

    // Create tiered store: HashMap (L1) -> Redis (L2)
    let tiered: Arc<dyn Store<TestNamespace, User>> = Arc::new(TieredStore::from_stores(vec![
        hashmap_store.clone(),
        redis_store.clone(),
    ]));

    // Get from tiered - should find in L2 (Redis)
    let result = tiered.get(TestNamespace::Users, &test_key).await.unwrap();
    assert!(result.is_some());
    assert_eq!(result.unwrap().value.name, "Tiered User");

    // Wait for background L1 population
    tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;

    // Memory (L1) should now have the value
    let l1_result = hashmap_store
        .get(TestNamespace::Users, &test_key)
        .await
        .unwrap();
    assert!(l1_result.is_some());
    assert_eq!(l1_result.unwrap().value.name, "Tiered User");

    // Cleanup
    redis_store
        .remove(TestNamespace::Users, &[&test_key])
        .await
        .unwrap();
}

#[tokio::test]
async fn test_tiered_store_with_namespace_swr() {
    let hashmap_store: Arc<dyn Store<TestNamespace, User>> =
        Arc::new(HashMapStore::new(HashMapStoreConfig::default()));
    let redis_store: Arc<dyn Store<TestNamespace, User>> = Arc::new(create_redis_store().await);

    let cache = Namespace::new(
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
        .swr(TestNamespace::Users, &test_key, move |_key| {
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
        .swr(TestNamespace::Users, &test_key, move |_key| {
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
    cache.remove(TestNamespace::Users, &test_key).await.unwrap();
}
