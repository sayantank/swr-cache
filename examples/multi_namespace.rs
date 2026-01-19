//! Example demonstrating multi-namespace cache with CacheBuilder API.
//!
//! This shows how to create a cache with multiple namespaces, each with
//! their own configuration and stores. Note that all namespaces in a single
//! cache must store the same value type.

use std::sync::Arc;
use swr_cache::{CacheBuilder, HashMapStore, HashMapStoreConfig, Namespace};

#[derive(Clone, Debug)]
#[allow(dead_code)]
struct User {
    id: String,
    name: String,
    email: String,
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Create shared memory store
    let memory = Arc::new(HashMapStore::new(HashMapStoreConfig::default()));

    // Create separate namespaces - they're isolated even though they share the store
    let users_ns = Namespace::new("users", vec![memory.clone()], 60_000, 300_000);
    let admins_ns = Namespace::new("admins", vec![memory], 120_000, 600_000); // Different TTL

    // Build cache with multiple namespaces
    let cache = CacheBuilder::new()
        .add("users", users_ns)
        .add("admins", admins_ns)
        .build();

    // Access users namespace
    let users = cache.namespace("users");

    // Use SWR pattern - callback receives the actual key
    let user = users
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
    let user2 = users
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

    // Access admins namespace (isolated from users)
    let admins = cache.namespace("admins");

    admins
        .set(
            "admin:1",
            User {
                id: "admin:1".to_string(),
                name: "Super Admin".to_string(),
                email: "admin@example.com".to_string(),
            },
        )
        .await?;

    // Admin is not in users namespace
    let not_found = users.get("admin:1").await?;
    println!("Admin in users namespace: {:?}", not_found); // None

    // But exists in admins namespace
    let admin = admins.get("admin:1").await?;
    println!("Admin in admins namespace: {:?}", admin);

    Ok(())
}
