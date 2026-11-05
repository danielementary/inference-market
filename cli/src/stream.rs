//! Submits a prompt to the coordinator and reads back its server-sent events.

use std::collections::VecDeque;

use anyhow::{bail, Result};
use common::protocol::{PromptRequest, StreamEvent};
use futures_util::{stream::BoxStream, StreamExt};

/// Events of one request, in order, until the coordinator closes the stream.
pub struct EventStream {
    bytes: BoxStream<'static, reqwest::Result<bytes::Bytes>>,
    parser: SseParser,
    ready: VecDeque<StreamEvent>,
}

/// `POST {coordinator}/requests`. Refusals (no matching offer, wrong hash, …) come back as
/// errors carrying the coordinator's message.
pub async fn submit(
    http: &reqwest::Client,
    coordinator: &str,
    body: &PromptRequest,
) -> Result<EventStream> {
    let response = http
        .post(format!("{coordinator}/requests"))
        .json(body)
        .send()
        .await?;
    let status = response.status();
    if !status.is_success() {
        bail!(
            "coordinator refused the prompt ({status}): {}",
            response.text().await?
        );
    }
    Ok(EventStream {
        bytes: response.bytes_stream().boxed(),
        parser: SseParser::default(),
        ready: VecDeque::new(),
    })
}

impl EventStream {
    pub async fn next(&mut self) -> Option<Result<StreamEvent>> {
        loop {
            if let Some(event) = self.ready.pop_front() {
                return Some(Ok(event));
            }
            match self.bytes.next().await? {
                Ok(chunk) => match self.parser.push(&chunk) {
                    Ok(events) => self.ready.extend(events),
                    Err(e) => return Some(Err(e)),
                },
                Err(e) => return Some(Err(e.into())),
            }
        }
    }
}

/// Splits a byte stream into SSE blocks (separated by a blank line) and decodes their `data:`
/// lines. Blocks may arrive split across network packets.
#[derive(Default)]
pub struct SseParser {
    buffer: String,
}

impl SseParser {
    pub fn push(&mut self, bytes: &[u8]) -> Result<Vec<StreamEvent>> {
        self.buffer.push_str(&String::from_utf8_lossy(bytes));
        let mut events = Vec::new();
        while let Some(end) = self.buffer.find("\n\n") {
            let block: String = self.buffer.drain(..end + 2).collect();
            for data in block.lines().filter_map(|line| line.strip_prefix("data:")) {
                events.push(serde_json::from_str(data.trim())?);
            }
        }
        Ok(events)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_blocks_split_across_packets() {
        let mut parser = SseParser::default();
        assert!(parser
            .push(b"data: {\"type\":\"queued\"}\n")
            .unwrap()
            .is_empty());
        let events = parser
            .push(b"\ndata: {\"type\":\"chunk\",\"token\":\"hi\"}\n\n: keep-alive\n\n")
            .unwrap();
        assert_eq!(
            events,
            [
                StreamEvent::Queued,
                StreamEvent::Chunk { token: "hi".into() }
            ]
        );
    }

    #[test]
    fn rejects_malformed_data() {
        assert!(SseParser::default().push(b"data: nope\n\n").is_err());
    }
}
