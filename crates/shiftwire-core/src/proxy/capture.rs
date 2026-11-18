//! Capturing bodies while they stream through, without slowing them down.

use std::pin::Pin;
use std::sync::{Arc, Mutex};
use std::task::{Context, Poll};

use bytes::Bytes;
use hyper::body::{Body, Frame, SizeHint};

/// The first `limit` bytes of a body plus its true total size.
#[derive(Debug, Default)]
pub(crate) struct BodyCapture {
    pub data: Vec<u8>,
    pub total: u64,
    limit: usize,
}

impl BodyCapture {
    pub fn new(limit: usize) -> Self {
        Self {
            data: Vec::new(),
            total: 0,
            limit,
        }
    }

    pub fn push(&mut self, chunk: &[u8]) {
        self.total += chunk.len() as u64;
        let room = self.limit.saturating_sub(self.data.len());
        self.data.extend_from_slice(&chunk[..chunk.len().min(room)]);
    }

    pub fn truncated(&self) -> bool {
        self.total > self.data.len() as u64
    }
}

pub(crate) type Shared = Arc<Mutex<BodyCapture>>;
type OnEnd = Box<dyn FnOnce(Option<String>) + Send>;

/// Wraps a body: frames pass through unchanged while a copy is kept in `capture`.
/// `on_end` runs exactly once, with an error if the body did not finish cleanly.
pub(crate) struct Tee<B> {
    inner: B,
    capture: Shared,
    on_end: Option<OnEnd>,
}

impl<B> Tee<B> {
    pub fn new(inner: B, capture: Shared, on_end: Option<OnEnd>) -> Self {
        Self {
            inner,
            capture,
            on_end,
        }
    }

    fn finish(&mut self, error: Option<String>) {
        if let Some(on_end) = self.on_end.take() {
            on_end(error);
        }
    }
}

impl<B> Body for Tee<B>
where
    B: Body<Data = Bytes> + Unpin,
    B::Error: std::fmt::Display,
{
    type Data = Bytes;
    type Error = B::Error;

    fn poll_frame(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
    ) -> Poll<Option<Result<Frame<Bytes>, B::Error>>> {
        let this = &mut *self;
        let polled = Pin::new(&mut this.inner).poll_frame(cx);
        match &polled {
            Poll::Ready(Some(Ok(frame))) => {
                if let Some(data) = frame.data_ref() {
                    this.capture.lock().expect("capture lock").push(data);
                }
                // With a Content-Length, hyper stops polling after the last byte and
                // never sees `None`, so finish as soon as the body says it is done.
                if this.inner.is_end_stream() {
                    this.finish(None);
                }
            }
            Poll::Ready(Some(Err(e))) => this.finish(Some(e.to_string())),
            Poll::Ready(None) => this.finish(None),
            Poll::Pending => {}
        }
        polled
    }

    fn is_end_stream(&self) -> bool {
        self.inner.is_end_stream()
    }

    fn size_hint(&self) -> SizeHint {
        self.inner.size_hint()
    }
}

impl<B> Drop for Tee<B> {
    fn drop(&mut self) {
        // Reached when the client disconnects mid-body. (Empty bodies, which the
        // server may never poll, are finished up front by the caller.)
        if self.on_end.is_some() {
            self.finish(Some("connection closed before the body finished".into()));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn capture_keeps_prefix_and_true_size() {
        let mut c = BodyCapture::new(4);
        c.push(b"abc");
        c.push(b"defg");
        assert_eq!(c.data, b"abcd");
        assert_eq!(c.total, 7);
        assert!(c.truncated());
    }
}
