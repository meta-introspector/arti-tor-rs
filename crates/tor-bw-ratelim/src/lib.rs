//! Partial implementation sketch of rate-limiting using a centralized "limiter"
//! object and a notification task.
//!
//! TODO: this is just a sketch; see notes below about why we're going to come
//! back to this later.
//!
//! ## Design
//!
//! We face several considerations that influence our design:
//!
//! * In Tor, there isn't much point in reading or writing less than a single
//!   cell.
//! * With relays, we expect that sometimes we will have thousands of channels
//!   and TCP streams that need to be placed under a single limit.
//! * With relays, it is not unusual for the bandwidth limit to be saturated
//!   much of them.
//! * Under write contention, we will eventually want to implement "KIST-lite",
//!   which requires us to make fairness decisions among _circuits_, not
//!   _channels_.
//! * For relays, we'll need a limiter that can apply a single limit to channels
//!   and exit TCP streams.
//! * With TCP operations, we don't necessarily know how many bytes the kernel
//!   will let us read or write at any given moment: so even if the rate limiter
//!   gives the channel permission to write N bytes, the kernel can return
//!   EAGAIN, or say that there was only enough space to write M bytes. For this
//!   situation case, the rate limiter needs some way to "reclaim" the bytes
//!   that it had previously given out permission to read or write.
//!
//! With that in mind, we need an implementation that is efficient under
//! contention with thousands of connections, that can be cell-oriented or
//! byte-oriented, and that can be adapted to have a complex fairness algorithm.
//!
//! These requirements preclude a naive usage of the `async-speed-limit` or
//! `governor` crates: under high contention, their default behavior is to let
//! each data stream race with one another.
//!
//! (Additionally, `async-speed-limit` works by letting its bucket level go
//! negative, which is adequate to enforce average long-term speed, but not
//! adequate to enforce a burst limit. With a large number of simultaneous
//! connections, the burst can get quite large.)
//!
//! So instead I'm sketching this design:
//!
//! * Each data stream has a reference to a Limiter object from which it asks
//!   permission to consume a number of bytes.
//!   * Having received permission to consume, this permission can be _returned_
//!     to the limiter if it is not used.
//! * When a stream asks permission to consume bytes, but permission is denied,
//!   the limiter gives it a future to  wait on.  The limiter has a background
//!   task that is responsible for waking up these futures as more bytes become
//!   available.
//!
//! ## Postponement
//!
//! NOTE: We are postponing the rest of this for now, since a real
//! implementation here will want to take "KIST" or "KIST-lite" into account.
//! Those algorithms use circuit-based implementation to decide which channel(s)
//! get to write next when there is contention.
//!
//! Because of that, any non-KIST aware implementation work here is likely to be
//! temporary at best.

#![allow(clippy::let_unit_value, dead_code)]

mod io;
mod limiter;

use std::sync::Arc;

pub use io::LimitedRead;
pub(crate) use limiter::BwLimiter;

#[derive(Copy, Clone, Debug)]
pub struct TrafficRateLimit {
    max_bytes_per_sec: u64,
    max_bytes_burst: u64,
}

#[derive(Clone, Debug)]
pub struct LimiterConfig {
    upload_limit: TrafficRateLimit,
    download_limit: TrafficRateLimit,
}

pub struct Limiter {
    r: Arc<BwLimiter>,
    w: Arc<BwLimiter>,
}

impl Limiter {
    /// This might need to take a Runtime, a clock type, or who
    /// knows what else. Maybe we need a generalization of SleepProvider
    /// that provides its own Instant and Duration types.
    ///
    /// Ack; I think what we need is a generalization of a SleepProvider that defines its own Instant and Duration types.
    pub fn new(cfg: &LimiterConfig) -> Arc<Self> {
        Arc::new(Self {
            r: BwLimiter::new(cfg.download_limit),
            w: BwLimiter::new(cfg.upload_limit),
        })
    }
    /* TODO
    pub fn reconfigure(&self, cfg: &LimitConfig) -> Result<(), ReconfigError> { todo!() }
    */

    /// All `LimitIo` from the same `Limiter` interact,
    /// sharing the limit and using from kthe same quota.
    pub fn limit_read<T>(&self, io: T) -> LimitedRead<T> {
        LimitedRead::new(self.r.clone(), io)
    }
}
