use std::{
    rc::Weak,
    sync::{Arc, Mutex},
};

use tor_rtcompat::Runtime;

use crate::TrafficRateLimit;

// Approximate number of bytes needed to write a cell to the network, including
// TLS overhead.
//
// Does not need to be precise; this is an overestimate.
const BYTES_PER_CELL: usize = 576;

struct LimiterInner {
    // How much time must pass before we are ready to add a single cell to the
    // queue?
    dur_per_cell: coarsetime::Duration,
    // How many bytes are we willing to send per burst?
    bytes_burst: usize,

    // How many bytes of capacity do we have?
    //
    // NOTE: In reality, it would make sense to use a GCRA algorithm here like
    // governor does.  We need to decide whether the GCRA algorithm is based on
    // bytes or on cells.  If it's based on cells, we should retain an
    // additional "slop" for unused bytes above or below the cell increment.
    cur_level: usize,

    // An event that we'll use to notify waiters when we have more capacity.
    event: event_listener::Event,
}

pub(crate) struct BwLimiter {
    inner: Mutex<LimiterInner>,
}

impl BwLimiter {
    pub fn new(lim: TrafficRateLimit) -> Arc<Self> {
        let dur_per_cell = coarsetime::Duration::from_secs(1) * (BYTES_PER_CELL as u32)
            / (lim.max_bytes_per_sec as u32); // XXXX this cast is totally wrong.

        Arc::new(BwLimiter {
            inner: Mutex::new(LimiterInner {
                dur_per_cell,
                bytes_burst: lim.max_bytes_burst as usize, // XXXX also a bad cast.
                cur_level: 0,
                event: event_listener::Event::new(),
            }),
        })
    }

    pub fn launch_background_task<R: Runtime>(self: &Arc<Self>, _runtime: R) {
        todo!()
    }

    /// Return `n` unused bytes to the current bucket.
    ///
    /// This can violate our limits unless you have previously received
    /// permission to read this many bytes.
    fn put_back(&self, n: usize) {
        let mut inner = self.inner.lock().expect("poisoned lock");
        inner.cur_level = inner.cur_level.saturating_add(n).min(inner.bytes_burst);
    }

    /// Submit a request to consume `n` bytes.  On success, return a number of
    /// bytes no greater than `n` which we may consume.  On failure,
    /// return an event that we should wait for before asking again.
    pub(crate) fn take_bytes(&self, n: usize) -> Result<Permit<'_>, event_listener::EventListener> {
        let mut inner = self.inner.lock().expect("poisoned lock");
        if n <= inner.cur_level {
            // If we can satisfy all of this request, then do so.
            //
            // XXXX (but if there is contention, we might not want to do return
            // more then BYTES_PER_CELL!)
            inner.cur_level -= n;
            Ok(Permit { n, limiter: self })
        } else if BYTES_PER_CELL <= inner.cur_level {
            // If we can satisfy BYTES_PER_CELL, then do so.
            debug_assert!(BYTES_PER_CELL < n);
            inner.cur_level -= BYTES_PER_CELL;
            Ok(Permit {
                n: BYTES_PER_CELL,
                limiter: self,
            })
        } else {
            // Otherwise, the request wants to write at least BYTES_PER_CELL,
            // and we don't have that much quota.

            // TODO: Perhaps, tell the background task in this case that it
            // should start its timer if it has not done so already.

            // TODO: This returns an event_listener, which is heap-allocated.
            // It might be better to have take_bytes function take a cx as an argument
            // and store a Waker.
            Err(inner.event.listen())
        }
    }
}

/// Permission to consume a certain number of bytes from the BwLimiter.
///
/// On failure, the caller should call `unused()` to "give back" these bytes.
///
/// On partial success, the caller should call `used()` to "give back" the
/// unused portion of these bytes.
pub(crate) struct Permit<'a> {
    pub(crate) n: usize,
    limiter: &'a BwLimiter,
}

impl<'a> Permit<'a> {
    // Report the amount of bytes from this permit that have actually been used.
    pub(crate) fn used(mut self, used: usize) {
        debug_assert!(used < self.n);
        self.limiter.put_back(self.n - used);
        self.n = 0;
    }

    // Report that no amount of this permit was actually used.
    pub(crate) fn unused(mut self) {
        self.limiter.put_back(self.n);
        self.n = 0;
    }
}

fn background_task(_limiter: Weak<BwLimiter>) {
    loop {
        // - as time elapses, refill buffer
        // - notify waiters.
        // - When waiting, be sure to wait at least for as long as dur_per_cell.
        todo!()
    }
}
