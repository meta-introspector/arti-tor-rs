use futures::{io::AsyncRead, ready, Future};
use pin_project::pin_project;
use std::io::Result as IoResult;
use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll};

use crate::limiter::BwLimiter;

// XXXX We also need a LimitedWrite.  And possibly a split-able LimitedIo.
#[pin_project]
pub struct LimitedRead<T> {
    limiter: Arc<crate::BwLimiter>,

    #[pin]
    waiting_for: Option<event_listener::EventListener>,

    #[pin]
    inner: T,
}

impl<T: AsyncRead> AsyncRead for LimitedRead<T> {
    fn poll_read(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut [u8],
    ) -> Poll<IoResult<usize>> {
        let mut this = self.project();

        loop {
            {
                let waiting_for = this.waiting_for.as_mut().as_pin_mut();
                if let Some(waiting_for) = waiting_for {
                    let () = ready!(waiting_for.poll(cx)); // return if waiting.
                }
                // no longer waiting for anybody!
                *this.waiting_for = None;
            }

            match this.limiter.take_bytes(buf.len()) {
                Ok(permit) => match this.inner.poll_read(cx, &mut buf[0..permit.n]) {
                    Poll::Ready(Ok(n_actually_read)) => {
                        permit.used(n_actually_read);
                        return Poll::Ready(Ok(n_actually_read));
                    }
                    Poll::Ready(Err(e)) => {
                        permit.unused();
                        return Poll::Ready(Err(e));
                    }
                    Poll::Pending => {
                        permit.unused();
                        return Poll::Pending;
                    }
                },
                Err(wait) => {
                    *this.waiting_for = Some(wait);
                    continue; // loop here to ensure that we poll the event if we just added it.
                }
            }
        }
    }
}

impl<T> LimitedRead<T> {
    // XXXX TODO naming on these two methods.
    pub fn inner_pinned<'a>(self: Pin<&'a mut Self>) -> Pin<&'a mut T> {
        self.project().inner
    }
    pub fn inner(&mut self) -> &mut T {
        &mut self.inner
    }
    pub fn into_inner(self) -> T {
        self.inner
    }

    pub(crate) fn new(limiter: Arc<BwLimiter>, io: T) -> Self {
        Self {
            limiter,
            waiting_for: None,
            inner: io,
        }
    }
}
