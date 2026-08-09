//! A socket that is two channels — what `beck_rt::session::Socket` was written for.
//!
//! "The upgraded socket in the server, and an in-memory duplex in the tests": this is the second
//! half of that sentence, shared by every harness that drives `beck_rt::session::run` rather than
//! calling into the runtime. Both rendering modes land in that function, and both are driven
//! through this.

#![allow(dead_code)] // each test binary uses the half of this it needs

use std::pin::Pin;
use std::task::{Context, Poll};

use futures_util::{Sink, Stream};
use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender};
use tokio_tungstenite::tungstenite::{Error as WsError, Message};

pub struct Duplex {
    pub out: UnboundedSender<Message>,
    pub inbox: UnboundedReceiver<Message>,
}

impl Sink<Message> for Duplex {
    type Error = WsError;
    fn poll_ready(self: Pin<&mut Self>, _: &mut Context<'_>) -> Poll<Result<(), WsError>> {
        Poll::Ready(Ok(()))
    }
    fn start_send(self: Pin<&mut Self>, item: Message) -> Result<(), WsError> {
        let _ = self.out.send(item);
        Ok(())
    }
    fn poll_flush(self: Pin<&mut Self>, _: &mut Context<'_>) -> Poll<Result<(), WsError>> {
        Poll::Ready(Ok(()))
    }
    fn poll_close(self: Pin<&mut Self>, _: &mut Context<'_>) -> Poll<Result<(), WsError>> {
        Poll::Ready(Ok(()))
    }
}

impl Stream for Duplex {
    type Item = Result<Message, WsError>;
    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        self.inbox.poll_recv(cx).map(|m| m.map(Ok))
    }
}

/// Everything the server has said so far, as JSON.
pub async fn drain(rx: &mut UnboundedReceiver<Message>) -> Vec<serde_json::Value> {
    let mut out = Vec::new();
    // The loop is cooperative, so a short yield is enough for the session task to make progress
    // without a sleep long enough to be a flake.
    for _ in 0..64 {
        while let Ok(Message::Text(t)) = rx.try_recv() {
            if let Ok(v) = serde_json::from_str(&t) {
                out.push(v);
            }
        }
        tokio::task::yield_now().await;
    }
    out
}
