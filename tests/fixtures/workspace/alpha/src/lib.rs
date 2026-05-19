mod shared;

use std::collections::HashMap;

use axum::{Router, extract::Query, routing::get};

use crate::shared::HandlerParams;

async fn handler(_q: Query<HandlerParams>) {}

pub fn build_router() -> Router {
    Router::new().route("/", get(handler))
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Untouched {
    pub x: i32,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct PartiallyUsed {
    pub name: String,
}

#[derive(Debug, Clone)]
pub struct AllUsed {
    pub label: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum Kind {
    A,
    B,
}

pub fn demo() -> HashMap<Kind, AllUsed> {
    let used = AllUsed { label: "hi".into() };
    println!("{:?}", used);
    let cloned = used.clone();

    let partial = PartiallyUsed { name: "p".into() };
    let _ = partial.clone();

    let mut map = HashMap::new();
    map.insert(Kind::A, cloned);
    map
}
