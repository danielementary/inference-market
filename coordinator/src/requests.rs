//! `POST /requests`: a client submits the prompt of an on-chain request; the response is the
//! SSE stream of that request.

use std::{convert::Infallible, sync::Arc};

use axum::{
    extract::State,
    http::StatusCode,
    response::{
        sse::{Event, KeepAlive},
        Sse,
    },
    Json,
};
use common::{
    client::types::RequestStatus,
    ix,
    prompt::{count_tokens, prompt_hash},
    protocol::{PromptRequest, StreamEvent},
    Address,
};
use futures_util::{Stream, StreamExt};
use tokio::sync::mpsc;
use tokio_stream::wrappers::ReceiverStream;

use crate::{
    matching,
    state::{App, Pending},
};

/// Prompts above this size are refused, so a client can't exhaust the coordinator's memory.
const MAX_PROMPT_BYTES: usize = 64 * 1024;

type Rejection = (StatusCode, String);

pub async fn submit(
    State(app): State<Arc<App>>,
    Json(body): Json<PromptRequest>,
) -> Result<Sse<impl Stream<Item = Result<Event, Infallible>>>, Rejection> {
    let pending = check(&app, body).await?;
    let (events, rx) = mpsc::channel(256);
    let _ = events.send(StreamEvent::Queued).await;
    let request = pending.request;
    if !app.enqueue(Pending { events, ..pending }) {
        return Err((
            StatusCode::CONFLICT,
            format!("request {request} was already submitted"),
        ));
    }
    app.dispatch();

    let stream = ReceiverStream::new(rx)
        .map(|event| Ok(Event::default().json_data(event).expect("events serialize")));
    Ok(Sse::new(stream).keep_alive(KeepAlive::default()))
}

/// Validates the prompt against its on-chain request before queueing it.
async fn check(app: &App, body: PromptRequest) -> Result<Pending, Rejection> {
    let reject = |status: StatusCode, msg: String| Err((status, msg));
    if body.prompt.len() > MAX_PROMPT_BYTES {
        return reject(
            StatusCode::PAYLOAD_TOO_LARGE,
            format!("prompt above {MAX_PROMPT_BYTES} bytes"),
        );
    }
    let Ok(client) = body.client.parse::<Address>() else {
        return reject(StatusCode::BAD_REQUEST, "invalid client address".into());
    };
    let request = ix::request_pda(&client, body.nonce);
    let terms = match app.chain.request(&client, body.nonce).await {
        Ok(Some(terms)) => terms,
        Ok(None) => {
            return reject(
                StatusCode::NOT_FOUND,
                format!("no request {request} on-chain"),
            )
        }
        Err(e) => return reject(StatusCode::BAD_GATEWAY, format!("RPC error: {e:#}")),
    };
    if terms.status != RequestStatus::Open {
        return reject(
            StatusCode::CONFLICT,
            format!("request {request} is not open"),
        );
    }
    if prompt_hash(&body.salt, &body.prompt) != terms.prompt_hash {
        return reject(
            StatusCode::FORBIDDEN,
            "prompt and salt don't match the on-chain prompt_hash".into(),
        );
    }

    // Refuse right away when no registered worker could ever serve these terms.
    let in_tokens = count_tokens(&body.prompt);
    let workers = app
        .chain
        .workers()
        .await
        .map_err(|e| (StatusCode::BAD_GATEWAY, format!("RPC error: {e:#}")))?;
    if !workers
        .iter()
        .any(|(_, w)| matching::compatible(w, &terms, in_tokens))
    {
        return reject(
            StatusCode::UNPROCESSABLE_ENTITY,
            "no registered worker matches the model, throughput, prices and budget".into(),
        );
    }

    // `events` is replaced by the caller once the request is accepted.
    let (events, _) = mpsc::channel(1);
    Ok(Pending {
        request,
        client,
        nonce: body.nonce,
        terms,
        prompt: body.prompt,
        in_tokens,
        events,
    })
}
