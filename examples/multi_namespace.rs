//! Example demonstrating type-agnostic stores with multiple Cache instances.
//!
//! This shows how to create reusable stores that can be shared across
//! multiple Cache instances with different value types. Each Cache maintains
//! type safety while stores remain type-agnostic.

use serde::{Deserialize, Serialize};
use std::sync::Arc;
use swr_cache::{Cache, MokaStore, MokaStoreConfig, RedisStore, RedisStoreConfig};

#[derive(Clone, Debug, Serialize, Deserialize)]
struct User {
    id: String,
    name: String,
    email: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
struct ApiKey {
    key: String,
    user_id: String,
    created_at: i64,
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Create type-agnostic stores that can be reused across different types
    let l1_store = Arc::new(MokaStore::new(MokaStoreConfig::default()));
    let l2_store = Arc::new(
        RedisStore::new(RedisStoreConfig {
            url: "redis://localhost:6379".to_string(),
            disable_expiration: false,
        })
        .await?,
    );

    // You could add more stores here (e.g., Redis for L2)
    // let l2_store = Arc::new(RedisStore::new(RedisStoreConfig { ... }).await?);

    // Create separate Cache instances for different types
    // These caches can share the same store instances!
    let user_cache: Cache<User> = Cache::new(
        "users",
        vec![l1_store.clone(), l2_store],
        60_000,  // fresh for 60 seconds
        300_000, // stale for 5 minutes
    );

    let apikey_cache: Cache<ApiKey> = Cache::new(
        "apikeys",
        vec![l1_store.clone()], // Same store instance!
        120_000,                // fresh for 2 minutes
        600_000,                // stale for 10 minutes
    );

    // Use the user cache with SWR pattern
    let user = user_cache
        .swr("chronark", |id| async move {
            println!("Loading user from database: {}", id);
            Some(User {
                id: id.clone(),
                name: "Andreas".to_string(),
                email: "andreas@example.com".to_string(),
            })
        })
        .await?;

    println!("User: {:?}", user);

    // Wait for background caching to complete
    tokio::time::sleep(tokio::time::Duration::from_millis(50)).await;

    // Second call hits cache
    let user2 = user_cache
        .swr("chronark", |id| async move {
            println!("This won't be called - using cached value");
            Some(User {
                id,
                name: "Should not appear".to_string(),
                email: "no@example.com".to_string(),
            })
        })
        .await?;

    println!("User (cached): {:?}", user2);

    // Use the apikey cache (different type, same store!)
    apikey_cache
        .set(
            "key_123",
            ApiKey {
                key: "sk_test_123".to_string(),
                user_id: "chronark".to_string(),
                created_at: 1234567890,
            },
        )
        .await?;

    // ApiKey is in apikey cache
    let api_key = apikey_cache.get("key_123").await?;
    println!("ApiKey in apikey cache: {:?}", api_key);

    // But not in user cache (different namespace and type)
    let not_found = user_cache.get("key_123").await?;
    println!("key_123 in user cache: {:?}", not_found); // None

    println!("\n✅ Both caches are using the same underlying store instance!");
    println!("   This saves memory and connection resources while maintaining type safety.");

    Ok(())
}
