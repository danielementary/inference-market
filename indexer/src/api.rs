//! Read-only REST API over the index.

use std::sync::{Arc, Mutex};

use axum::{
    extract::{Path, Query, State},
    http::StatusCode,
    routing::get,
    Json, Router,
};
use common::history::{Earnings, RequestRecord};
use serde::Deserialize;

use crate::index::Index;

type Shared = Arc<Mutex<Index>>;

pub fn router(index: Shared) -> Router {
    Router::new()
        .route("/health", get(health))
        .route("/requests", get(requests))
        .route("/workers/{owner}/earnings", get(earnings))
        .with_state(index)
}

/// Last indexed signature.
async fn health(State(index): State<Shared>) -> Json<Option<String>> {
    Json(index.lock().unwrap().last_signature().map(String::from))
}

#[derive(Deserialize)]
struct Filter {
    client: Option<String>,
    worker: Option<String>,
}

/// `GET /requests?client=…&worker=…`, newest first.
async fn requests(
    State(index): State<Shared>,
    Query(f): Query<Filter>,
) -> Json<Vec<RequestRecord>> {
    Json(
        index
            .lock()
            .unwrap()
            .requests(f.client.as_deref(), f.worker.as_deref()),
    )
}

/// `GET /workers/{owner}/earnings`.
async fn earnings(
    State(index): State<Shared>,
    Path(owner): Path<String>,
) -> Result<Json<Earnings>, (StatusCode, String)> {
    match index.lock().unwrap().earnings(&owner) {
        Some(e) => Ok(Json(e)),
        None => Err((
            StatusCode::NOT_FOUND,
            format!("no worker registered by {owner}"),
        )),
    }
}
