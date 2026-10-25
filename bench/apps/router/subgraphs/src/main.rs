//! The four subgraphs of the Router job, in one process: Apollo's federation demo (accounts,
//! inventory, products, reviews) with a catalogue large enough that a query returns tens of
//! kilobytes, written with async-graphql's federation support.
//!
//!   subgraphs
//!
//! Each subgraph answers GraphQL over HTTP/1.1 POST at its own path (`/accounts`,
//! `/inventory`, `/products`, `/reviews`) on one loopback port, which it prints as
//! `BENCH_LISTEN addr` once it listens. The data is generated from fixed rules as it starts,
//! so every run serves the same answers.
//!
//! This process is not what the job measures, and it must keep up with the Router on fewer
//! CPUs, so it remembers each answer by the request's body and sends it again when the same
//! request comes back: the load is a fixed list of requests, so after the first pass through
//! it every subgraph request is a lookup and a write. A request whose answer carries GraphQL
//! errors is not remembered, and is counted on stderr.
use async_graphql::{EmptyMutation, EmptySubscription, ID, Object, Schema};
use axum::body::Bytes;
use axum::extract::State;
use axum::http::{StatusCode, header};
use axum::response::{IntoResponse, Response};
use axum::routing::post;
use std::collections::HashMap;
use std::future::Future;
use std::pin::Pin;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, OnceLock, RwLock};

const PRODUCTS: usize = 10_000;
const USERS: usize = 5_000;
/// Answers remembered per subgraph, at most: the load's distinct requests are a few hundred.
const MEMO_MAX: usize = 100_000;

/// The catalogue every subgraph reads.
struct Data {
    products: Vec<ProductRow>,
    reviews: Vec<ReviewRow>,
    /// Review indices by product and by author.
    by_product: Vec<Vec<usize>>,
    by_author: Vec<Vec<usize>>,
}

struct ProductRow {
    name: String,
    price: i32,
    weight: i32,
    description: String,
    in_stock: bool,
}

struct ReviewRow {
    id: String,
    body: String,
    rating: i32,
    author: usize,
    product: usize,
}

const WORDS: &[&str] = &[
    "oak", "linen", "brass", "walnut", "wool", "steel", "maple", "cotton", "glass", "copper", "cedar", "felt",
    "sturdy", "light", "quiet", "classic", "modern", "compact", "deep", "soft", "bright", "solid", "simple", "table",
    "chair", "lamp", "shelf", "couch", "desk", "rug", "stool", "cabinet", "bench", "mirror", "frame", "arrived",
    "quickly", "assembly", "took", "minutes", "looks", "great", "in", "the", "room", "would", "buy", "again", "color",
    "is", "warmer", "than", "pictured", "and", "the", "finish", "feels", "durable", "after", "months", "of", "daily",
    "use",
];

/// A deterministic sentence of about `len` bytes.
fn text(seed: u64, len: usize) -> String {
    let mut x = seed.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
    let mut s = String::with_capacity(len + 12);
    while s.len() < len {
        x = x.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
        if !s.is_empty() {
            s.push(' ');
        }
        s.push_str(WORDS[(x >> 33) as usize % WORDS.len()]);
    }
    s
}

impl Data {
    fn build() -> Data {
        let products: Vec<ProductRow> = (0..PRODUCTS)
            .map(|i| ProductRow {
                name: format!("{} {}", text(i as u64, 12), i + 1),
                price: ((i * 37) % 1000 + 1) as i32,
                weight: ((i * 13) % 100 + 1) as i32,
                description: text(1_000_000 + i as u64, 120 + (i * 7) % 200),
                in_stock: i % 3 != 0,
            })
            .collect();
        let mut reviews = Vec::new();
        let mut by_product = vec![Vec::new(); PRODUCTS];
        let mut by_author = vec![Vec::new(); USERS];
        for (p, list) in by_product.iter_mut().enumerate() {
            for k in 0..(p % 5) + 1 {
                let author = (p * 7 + k * 13) % USERS;
                let r = reviews.len();
                reviews.push(ReviewRow {
                    id: format!("{}-{}", p + 1, k + 1),
                    body: text(2_000_000 + r as u64, 80 + (r * 11) % 300),
                    rating: ((p + k) % 5 + 1) as i32,
                    author,
                    product: p,
                });
                list.push(r);
                by_author[author].push(r);
            }
        }
        Data { products, reviews, by_product, by_author }
    }
}

fn data() -> &'static Data {
    static D: OnceLock<Data> = OnceLock::new();
    D.get_or_init(Data::build)
}

/// A product's upc is its index plus one, as a string.
fn product_index(upc: &str) -> Option<usize> {
    upc.parse::<usize>().ok().filter(|&n| (1..=PRODUCTS).contains(&n)).map(|n| n - 1)
}

fn user_index(id: &str) -> Option<usize> {
    id.parse::<usize>().ok().filter(|&n| (1..=USERS).contains(&n)).map(|n| n - 1)
}

fn review_index(id: &str) -> Option<usize> {
    let (p, k) = id.split_once('-')?;
    let p = product_index(p)?;
    let k: usize = k.parse().ok()?;
    data().by_product[p].get(k.checked_sub(1)?).copied()
}

mod products {
    use super::*;

    pub struct Product(pub usize);

    #[Object]
    impl Product {
        async fn upc(&self) -> String {
            (self.0 + 1).to_string()
        }
        async fn name(&self) -> &str {
            &data().products[self.0].name
        }
        async fn price(&self) -> i32 {
            data().products[self.0].price
        }
        async fn weight(&self) -> i32 {
            data().products[self.0].weight
        }
        async fn description(&self) -> &str {
            &data().products[self.0].description
        }
    }

    pub struct Query;

    #[Object]
    impl Query {
        async fn top_products(
            &self,
            #[graphql(default = 5)] first: i32,
            #[graphql(default = 0)] offset: i32,
        ) -> Vec<Product> {
            let start = (offset.max(0) as usize).min(PRODUCTS);
            let end = (start + first.max(0) as usize).min(PRODUCTS);
            (start..end).map(Product).collect()
        }

        #[graphql(entity)]
        async fn find_product_by_upc(&self, upc: String) -> Option<Product> {
            product_index(&upc).map(Product)
        }
    }
}

mod inventory {
    use super::*;

    pub struct Product {
        pub index: usize,
        pub price: Option<i32>,
        pub weight: Option<i32>,
    }

    #[Object]
    impl Product {
        async fn upc(&self) -> String {
            (self.index + 1).to_string()
        }
        // External: the products subgraph resolves these, and the Router never asks this one
        // for them. Declared with the products subgraph's type, as composition requires.
        #[graphql(external)]
        async fn weight(&self) -> i32 {
            self.weight.unwrap_or_default()
        }
        #[graphql(external)]
        async fn price(&self) -> i32 {
            self.price.unwrap_or_default()
        }
        async fn in_stock(&self) -> bool {
            data().products[self.index].in_stock
        }
        /// Free over 500, else half the weight: from the fields the Router fetched from the
        /// products subgraph, as the demo's inventory computes it.
        #[graphql(requires = "price weight")]
        async fn shipping_estimate(&self) -> Option<i32> {
            let (price, weight) = (self.price?, self.weight?);
            Some(if price > 500 { 0 } else { weight / 2 })
        }
    }

    pub struct Query;

    #[Object]
    impl Query {
        #[graphql(entity)]
        async fn find_product_by_upc(
            &self,
            #[graphql(key)] upc: String,
            price: Option<i32>,
            weight: Option<i32>,
        ) -> Option<Product> {
            product_index(&upc).map(|index| Product { index, price, weight })
        }
    }
}

mod reviews {
    use super::*;

    pub struct Review(pub usize);

    #[Object]
    impl Review {
        async fn id(&self) -> ID {
            ID(data().reviews[self.0].id.clone())
        }
        async fn body(&self) -> &str {
            &data().reviews[self.0].body
        }
        async fn rating(&self) -> i32 {
            data().reviews[self.0].rating
        }
        #[graphql(provides = "username")]
        async fn author(&self) -> User {
            User(data().reviews[self.0].author)
        }
        async fn product(&self) -> Product {
            Product(data().reviews[self.0].product)
        }
    }

    pub struct User(pub usize);

    #[Object]
    impl User {
        async fn id(&self) -> ID {
            ID((self.0 + 1).to_string())
        }
        #[graphql(external)]
        async fn username(&self) -> String {
            format!("@user{}", self.0 + 1)
        }
        async fn reviews(&self) -> Vec<Review> {
            data().by_author[self.0].iter().map(|&r| Review(r)).collect()
        }
    }

    pub struct Product(pub usize);

    #[Object]
    impl Product {
        async fn upc(&self) -> String {
            (self.0 + 1).to_string()
        }
        async fn reviews(&self) -> Vec<Review> {
            data().by_product[self.0].iter().map(|&r| Review(r)).collect()
        }
    }

    pub struct Query;

    #[Object]
    impl Query {
        #[graphql(entity)]
        async fn find_user_by_id(&self, id: ID) -> Option<User> {
            user_index(&id).map(User)
        }
        #[graphql(entity)]
        async fn find_product_by_upc(&self, upc: String) -> Option<Product> {
            product_index(&upc).map(Product)
        }
        #[graphql(entity)]
        async fn find_review_by_id(&self, id: ID) -> Option<Review> {
            review_index(&id).map(Review)
        }
    }
}

mod accounts {
    use super::*;

    pub struct User(pub usize);

    #[Object]
    impl User {
        async fn id(&self) -> ID {
            ID((self.0 + 1).to_string())
        }
        async fn name(&self) -> String {
            format!("{} {}", text(3_000_000 + self.0 as u64, 6), self.0 + 1)
        }
        async fn username(&self) -> String {
            format!("@user{}", self.0 + 1)
        }
    }

    pub struct Product(pub usize);

    #[Object]
    impl Product {
        async fn upc(&self) -> String {
            (self.0 + 1).to_string()
        }
    }

    pub struct Query;

    #[Object]
    impl Query {
        async fn me(&self) -> User {
            User(0)
        }
        /// Twenty products spread over the catalogue.
        async fn recommended_products(&self) -> Vec<Product> {
            (0..20).map(|k| Product((k * 487 + 11) % PRODUCTS)).collect()
        }
        #[graphql(entity)]
        async fn find_user_by_id(&self, id: ID) -> Option<User> {
            user_index(&id).map(User)
        }
        #[graphql(entity)]
        async fn find_product_by_upc(&self, upc: String) -> Option<Product> {
            product_index(&upc).map(Product)
        }
    }
}

type Exec =
    dyn Fn(async_graphql::Request) -> Pin<Box<dyn Future<Output = async_graphql::Response> + Send>> + Send + Sync;

/// One subgraph: its schema, and the answers it remembers.
struct Subgraph {
    name: &'static str,
    exec: Box<Exec>,
    memo: RwLock<HashMap<Bytes, Bytes>>,
    errors: AtomicU64,
}

fn subgraph<Q: async_graphql::ObjectType + 'static>(name: &'static str, query: Q) -> Arc<Subgraph> {
    let schema = Schema::build(query, EmptyMutation, EmptySubscription).enable_federation().finish();
    let exec: Box<Exec> = Box::new(move |req| {
        let schema = schema.clone();
        Box::pin(async move { schema.execute(req).await })
    });
    Arc::new(Subgraph { name, exec, memo: RwLock::new(HashMap::new()), errors: AtomicU64::new(0) })
}

async fn answer(State(sub): State<Arc<Subgraph>>, body: Bytes) -> Response {
    if let Some(r) = sub.memo.read().unwrap().get(&body).cloned() {
        return json(r);
    }
    let req: async_graphql::Request = match serde_json::from_slice(&body) {
        Ok(r) => r,
        Err(e) => return (StatusCode::BAD_REQUEST, format!("not a GraphQL request: {e}")).into_response(),
    };
    let resp = (sub.exec)(req).await;
    let ok = resp.errors.is_empty();
    let out = Bytes::from(serde_json::to_vec(&resp).expect("a response serializes"));
    if ok {
        let mut m = sub.memo.write().unwrap();
        if m.len() < MEMO_MAX {
            m.insert(body, out.clone());
        }
    } else {
        let n = sub.errors.fetch_add(1, Ordering::Relaxed) + 1;
        if n.is_power_of_two() {
            eprintln!("subgraphs: {}: {n} answers with errors, latest {:?}", sub.name, resp.errors);
        }
    }
    json(out)
}

fn json(body: Bytes) -> Response {
    ([(header::CONTENT_TYPE, "application/json")], body).into_response()
}

fn main() {
    data();
    let threads = std::thread::available_parallelism().map_or(1, |n| n.get());
    let rt = tokio::runtime::Builder::new_multi_thread().worker_threads(threads).enable_all().build().unwrap();
    rt.block_on(async {
        let app = axum::Router::new()
            .route("/accounts", post(answer).with_state(subgraph("accounts", accounts::Query)))
            .route("/inventory", post(answer).with_state(subgraph("inventory", inventory::Query)))
            .route("/products", post(answer).with_state(subgraph("products", products::Query)))
            .route("/reviews", post(answer).with_state(subgraph("reviews", reviews::Query)));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.expect("bind");
        println!("BENCH_LISTEN {}", listener.local_addr().expect("local addr"));
        use std::io::Write;
        let _ = std::io::stdout().flush();
        axum::serve(listener, app).await.expect("serve");
    });
}
