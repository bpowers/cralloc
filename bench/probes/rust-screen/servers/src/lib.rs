//! An order service shared by the axum and actix-web servers: POST /orders parses an order,
//! prices it, keeps it in a sharded in-memory store that holds the most recent `KEEP` orders,
//! and answers with the stored order as JSON. Old orders are dropped by whichever worker
//! inserts the order that displaces them, so blocks cross threads as they do in a cache.
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

pub const KEEP: u64 = 100_000;
const SHARDS: usize = 64;

#[derive(Deserialize, Serialize, Clone)]
pub struct Item {
    pub sku: String,
    pub qty: u32,
    pub price_cents: u64,
    pub tags: Vec<String>,
}

#[derive(Deserialize)]
pub struct NewOrder {
    pub customer: String,
    pub items: Vec<Item>,
    pub note: Option<String>,
}

#[derive(Serialize)]
pub struct Order {
    pub id: u64,
    pub customer: String,
    pub items: Vec<Item>,
    pub total_cents: u64,
    pub note: Option<String>,
    pub summary: String,
}

pub struct Store {
    shards: Vec<Mutex<HashMap<u64, Arc<Order>>>>,
    next: AtomicU64,
}

impl Default for Store {
    fn default() -> Self {
        Store { shards: (0..SHARDS).map(|_| Mutex::new(HashMap::new())).collect(), next: AtomicU64::new(0) }
    }
}

impl Store {
    pub fn create(&self, new: NewOrder) -> String {
        let id = self.next.fetch_add(1, Ordering::Relaxed);
        let total_cents = new.items.iter().map(|i| i.price_cents * i.qty as u64).sum();
        let mut skus: Vec<&str> = new.items.iter().map(|i| i.sku.as_str()).collect();
        skus.sort_unstable();
        let summary = format!("{} items for {}: {}", new.items.len(), new.customer, skus.join(","));
        let order =
            Arc::new(Order { id, customer: new.customer, items: new.items, total_cents, note: new.note, summary });
        let body = serde_json::to_string(&*order).expect("serialise");
        self.shards[id as usize % SHARDS].lock().unwrap().insert(id, order);
        if id >= KEEP {
            let old = id - KEEP;
            let gone = self.shards[old as usize % SHARDS].lock().unwrap().remove(&old);
            drop(gone);
        }
        body
    }
}

pub fn port() -> u16 {
    std::env::args().nth(1).and_then(|p| p.parse().ok()).unwrap_or(18080)
}
