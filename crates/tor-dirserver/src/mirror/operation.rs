//! Directory Mirror Operation.
//!
//! # Specifications
//!
//! * [Directory cache operation](https://spec.torproject.org/dir-spec/directory-cache-operation.html).
//!
//! # Rationale
//!
//! This module implements the "core operation" of a directory mirror.
//! "Core operation" primarily refers to the logic involved in downloading
//! network documents from an upstream authority and inserting them into the
//! database.  This module notably **DOES NOT** provide any public (in the HTTP
//! sense) endpoints for querying documents.  This is purposely behind a different
//! module, so that the directory authority implementation can also make use of it.
//! You can think of this module as the one implementing the things unique
//! to directory mirrors.

use std::{
    fmt::Debug,
    net::SocketAddr,
    ops::Deref,
    time::{Duration, SystemTime},
};

use r2d2::Pool;
use r2d2_sqlite::SqliteConnectionManager;
use rand::{seq::IndexedRandom, Rng};
use retry_error::RetryError;
use rusqlite::{
    params,
    types::{FromSql, FromSqlResult, ToSqlOutput, ValueRef},
    OptionalExtension, ToSql, Transaction,
};
use tokio::net::TcpStream;
use tokio_util::compat::TokioAsyncReadCompatExt;
use tor_basic_utils::{retry::RetryDelay, RngExt};
use tor_dirclient::request::Requestable;
use tor_dircommon::{
    authority::AuthorityContacts,
    config::{DirTolerance, DownloadScheduleConfig},
};
use tor_error::internal;
use tor_netdoc::doc::netstatus::ConsensusFlavor;
use tor_rtcompat::PreferredRuntime;
use tracing::{debug, warn};

use crate::{
    database::{self, sql, Sha256},
    err::{AuthorityCommunicationError, DatabaseError, FatalError},
};

/// A saturating wrapper around [`SystemTime`].
///
/// This type implements a wrapper around [`SystemTime`] in order to provide
/// implementations for [`SaturatingSystemTime::saturating_add()`] as well as
/// [`SaturatingSystemTime::saturating_sub()`].  Those functions behave similar
/// to [`Duration::saturating_add()`] and [`Duration::saturating_sub()`]
/// respectively.
///
/// Additionally, this type also implements [`Deref`] into [`SystemTime`]
/// alongside [`From<SystemTime>`] in order to allow a seamless interaction
/// between these two types.
///
/// Besides this, there is also a [`From`] implementation, that safely calculates
/// the [`Duration`] since [`SaturatingSystemTime::min()`].
///
/// Also, for convience, this type implements [`FromSql`] and [`ToSql`].
///
/// TODO: In the medium- and long-term, we should make an upstream merge request
/// to Rust, adding these methods to [`SystemTime`] natively; it has been
/// requested by other parties too.
///
/// TODO: Maybe this type is better placed in the [`database`] module, but we
/// can do this in case this needs to be used by other modules in the code.
#[derive(Copy, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
struct SaturatingSystemTime(SystemTime);

impl SaturatingSystemTime {
    /// Returns the maximum value of a [`SaturatingSystemTime`] and
    /// hypothetically `SystemTime::MAX`.
    ///
    /// In POSIX, the maximum system time is represented by the infamous
    /// `struct timespec`, which contains a `time_t` for representing the
    /// seconds since the epoch and another integer for storing the nanons after
    /// the `time_t`.
    ///
    /// This effectively defines the maximum time a system can represent, namely
    /// the upper limit of `time_t` plus `1s - 1ns`.
    ///
    /// In this code, we assume that `time_t` can represent a [`i64::MAX`],
    /// which is assured in unit tests below.  It is possible for `time_t` to
    /// be unsigned, which we accept too, but we will still use the [`i64::MAX`]
    /// limit for all things requiring the above said limit, which is okay,
    /// given that [`i64::MAX`] as well as [`u64::MAX`] are two million years
    /// away from now.
    ///
    /// # Specifications
    ///
    /// * <https://man7.org/linux/man-pages/man3/time_t.3type.html>
    /// * <https://man7.org/linux/man-pages/man3/timespec.3type.html>
    fn max() -> Self {
        Self(
            Self::min()
                .checked_add(Duration::new(i64::MAX as u64, 999_999_999))
                .expect("cannot represent (i64::MAX, 999_999_999 as a SystemTime"),
        )
    }

    /// Returns the minimum value of a [`SaturatingSystemTime`].
    ///
    /// Unfortunately, POSIX does not specify whether `time_t` is signed or not.
    /// The overall consensus happens to be that it is implemented as a signed
    /// 64-but integer, but we have no real gurantee for that.
    ///
    /// To mitigate that, we simply use [`SystemTime::UNIX_EPOCH`] as the
    /// lower-bound, as it is guranteed to be representable with every `time_t`
    /// implementation (i.e. as `0`).  This obviously comes with the downside
    /// of not being able to represent timestamps before the Unix epoch, but
    /// given that this is over 55 years ago at the time of writing, there is
    /// probably little to not practical need to support this.
    ///
    /// # Specifications
    ///
    /// * <https://man7.org/linux/man-pages/man3/time_t.3type.html>
    /// * <https://man7.org/linux/man-pages/man3/timespec.3type.html>
    fn min() -> Self {
        Self(SystemTime::UNIX_EPOCH)
    }

    /// Converts a [`SystemTime`] into a [`SaturatingSystemTime`] saturatingly.
    ///
    /// In other words: This method rounds `value` into
    /// [`SaturatingSystemTime::min()`] and [`SaturatingSystemTime::max()`],
    /// if it lies outside the interval spanned up by those too.
    fn saturating_convert(mut value: SystemTime) -> Self {
        value = std::cmp::max(value, *Self::min());
        value = std::cmp::min(value, *Self::max());
        Self(value)
    }

    /// Securely adds a [`Duration`] to a [`SaturatingSystemTime`].
    ///
    /// This method adds a [`Duration`] to a [`SaturatingSystemTime`], returning
    /// [`SaturatingSystemTime::max()`] in the case of the result being
    /// unrepresentable by a [`SystemTime`].
    fn saturating_add(self, duration: Duration) -> Self {
        // Without the min(), `SystemTime`s second field being `u64` would fail
        // here.
        std::cmp::min(
            Self((*self).checked_add(duration).unwrap_or(*Self::max())),
            Self::max(),
        )
    }

    /// Securely subtracts a [`Duration`] from a [`SaturatingSystemTime`].
    ///
    /// This method removes a [`Duration`] from a [`SaturatingSystemTime`],
    /// returning [`SaturatingSystemTime::min()`] in the case of the result
    /// being unrepresentable by a [`SystemTime`].
    fn saturating_sub(self, duration: Duration) -> Self {
        // Without the max(), `time_t` being `i64` would fail here.
        std::cmp::max(
            Self((*self).checked_sub(duration).unwrap_or(*Self::min())),
            Self::min(),
        )
    }

    /// Converts a [`SaturatingSystemTime`] into a [`u64`] representing the
    /// seconds since the epoch.
    fn into_unix_time(self) -> u64 {
        (*self)
            .duration_since(*Self::min())
            .expect("invalid unix time representation??")
            .as_secs()
    }
}

impl Deref for SaturatingSystemTime {
    type Target = SystemTime;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl FromSql for SaturatingSystemTime {
    fn column_result(value: ValueRef<'_>) -> FromSqlResult<Self> {
        // The `as u64` conversion is safe due to database constraints.
        Ok(Self::min().saturating_add(Duration::from_secs(value.as_i64()? as u64)))
    }
}

impl ToSql for SaturatingSystemTime {
    fn to_sql(&self) -> rusqlite::Result<ToSqlOutput<'_>> {
        Ok(ToSqlOutput::from(self.into_unix_time() as i64))
    }
}

/// Obtains the most recent valid consensus from the database.
///
/// This function queries the database using a [`Transaction`] in order to have
/// a consistent view upon it.  It will return an [`Option`] containing various
/// consensus related timestamps plus the raw consensus itself (more on this
/// below).  In order to obtain a *valid* consensus, a [`SaturatingSystemTime`]
/// plus a [`DirTolerance`] is supplied, which will be used for querying the
/// database.
///
/// # The [`Ok`] Return Value
///
/// In the [`Some`] case, the return value is composed of the following:
/// 1. The `valid-after` timestamp represented by a [`SaturatingSystemTime`].
/// 2. The `fresh-until` timestamp represented by a [`SaturatingSystemTime`].
/// 3. The `valid-until` timestamp represented by a [`SaturatingSystemTime`].
/// 4. The raw consensus reprented by a [`String`].
///
/// The [`None`] case implies that no valid recent consensus has been found,
/// that is, no consensus at all or no consensus whose `valid-before` or
/// `valid-after` lies within the range composed by `now` and `tolerance`.
fn get_recent_consensus(
    tx: &Transaction,
    flavor: ConsensusFlavor,
    tolerance: &DirTolerance,
    now: SaturatingSystemTime,
) -> Result<
    Option<(
        SaturatingSystemTime,
        SaturatingSystemTime,
        SaturatingSystemTime,
        String,
    )>,
    DatabaseError,
> {
    // Select the most recent flavored consensus document from the database.
    //
    // The `valid_after` and `valid_until` cells must be a member of the range:
    // `[valid_after - pre_valid_tolerance; valid_after + post_valid_tolerance]`
    // (inclusively).
    //
    // The query parameters being:
    // ?1: The consensus flavor as a String.
    // ?2: `now` as a Unix timestamp.
    let mut meta_stmt = tx.prepare_cached(sql!(
        "
            SELECT valid_after, fresh_until, valid_until, sha256
            FROM consensus
              WHERE flavor = ?1
                AND ?2 >= valid_after - ?3
                AND ?2 <= valid_until + ?4
              ORDER BY valid_after DESC
              LIMIT 1
            "
    ))?;

    // Actually execute the query; a None is totally valid and considered as
    // no consensus being present in the current database.
    let meta = meta_stmt
        .query_one(
            params![
                flavor.name(),
                now,
                tolerance.pre_valid_tolerance().as_secs(),
                tolerance.post_valid_tolerance().as_secs()
            ],
            |row| {
                Ok((
                    row.get::<_, SaturatingSystemTime>(0)?,
                    row.get::<_, SaturatingSystemTime>(1)?,
                    row.get::<_, SaturatingSystemTime>(2)?,
                    row.get::<_, Sha256>(3)?,
                ))
            },
        )
        .optional()?;
    let (valid_after, fresh_until, valid_until, sha256) = match meta {
        Some(res) => res,
        None => return Ok(None),
    };

    // Query the sha256 obtained above from the content-addressable store table.
    //
    // Because the store table stores content as a BLOB, we also convert it to
    // a String securely, with errors composing a constraint violation.
    let mut content_stmt =
        tx.prepare_cached(sql!("SELECT content FROM store WHERE sha256 = ?1"))?;
    let consensus: Vec<u8> = content_stmt.query_one(params![sha256], |row| row.get(0))?;
    let consensus =
        String::from_utf8(consensus).map_err(|e| internal!("utf-8 contraint violated? {e}"))?;

    Ok(Some((valid_after, fresh_until, valid_until, consensus)))
}

/// Calculates the [`Duration`] to wait before querying the authorities again.
///
/// This function accepts a `fresh-until` and `valid-until`, both hopefully
/// obtained through the [`get_recent_consensus()`] function, and caculates a
/// [`Duration`] relative to `now`, describing the time to wait before querying
/// the authorities again.
///
/// If [`get_recent_consensus()`] returned [`None`], it is safe to skip a call
/// to this function and use [`Duration::ZERO`] instead.
///
/// # Specifications
///
/// * <https://spec.torproject.org/dir-spec/directory-cache-operation.html#download-ns-from-auth>
///
/// TODO DIRMIRROR: Consider not naming this timeout but something like
/// "download interval" or "poll interval".
fn calculate_sync_timeout<R: Rng>(
    fresh_until: SaturatingSystemTime,
    valid_until: SaturatingSystemTime,
    now: SaturatingSystemTime,
    rng: &mut R,
) -> Duration {
    assert!(fresh_until < valid_until);

    let offset = rng
        .gen_range_checked(0..=(valid_until.into_unix_time() - fresh_until.into_unix_time()) / 2)
        .expect("invalid range???");

    Duration::from_secs(
        fresh_until
            .saturating_add(Duration::from_secs(offset))
            .saturating_sub(Duration::from_secs(now.into_unix_time()))
            .into_unix_time(),
    )
}

/// Performs a request of a [`Requestable`] to a single authority.
///
/// A single authority consists of multiple endpoints, which we will try in a
/// concurrent round-robin fashion, taking the first one that succeeds.  This is
/// implemented as a part of [`tokio`], as specified in [`TcpStream::connect()`].
///
/// # Panics
///
/// This function panics if `endpoints` is empty.
async fn request_single<Req: Requestable + Debug>(
    endpoints: &[SocketAddr],
    req: &Req,
) -> Result<Vec<u8>, AuthorityCommunicationError> {
    assert!(!endpoints.is_empty());
    let rt = PreferredRuntime::current().expect("outside of tokio?");

    // Fortunately, Tokio's TcpStream::connect already offers round-robin.
    let stream = TcpStream::connect(&endpoints).await?;
    debug!(
        "connected to {}",
        stream
            .peer_addr()
            .map(|x| x.to_string())
            .unwrap_or("N/A".to_string())
    );
    let mut stream = stream.compat();

    // Perform the actual request.
    match tor_dirclient::send_request(&rt, req, &mut stream, None)
        .await
        .map(|resp| resp.into_output())
    {
        Ok(Ok(resp)) => Ok(resp),
        Ok(Err(e)) => Err(Box::new(e).into()),
        Err(tor_dirclient::Error::RequestFailed(e)) => Err(Box::new(e).into()),
        Err(e) => Err(AuthorityCommunicationError::Bug(internal!("{e}"))),
    }
}

/// Performs a retrying request of a [`Requestable`] to a set of authorities.
///
/// This function performs a request to a randomized set of authorities in a
/// round-robin fashion, retrying with decorrelated jitter timeout in the case
/// of a failure.
///
/// The return value is either the first successful response from an authority
/// to the request or a collection of all errors in a [`RetryError`].
///
/// Connections are made in a round-robin fashion per endpoint, see
/// [`request_single()`] and [`TcpStream::connect()`] respectively for more
/// information on this.
///
/// # Algorithm
///
/// For retrying failed downloads, the [`RetryDelay`] timeout performs the
/// complicated part.  See its documentation.
///
/// # Specifications
///
/// * <https://spec.torproject.org/dir-spec/client-operation.html#retrying-failed-downloads>
/// * <https://aws.amazon.com/blogs/architecture/exponential-backoff-and-jitter/>
#[allow(clippy::cognitive_complexity)]
async fn request_multi<R: Rng, Req: Requestable + Debug>(
    authorities: &[Vec<SocketAddr>],
    req: &Req,
    rng: &mut R,
) -> Result<Vec<u8>, RetryError<AuthorityCommunicationError>> {
    // Because this is a round-robin approach, we want to collect errors.
    let mut err = RetryError::in_attempt_to("request to authority");

    // TODO: Use rand::IndexedRandom::choose_iter once we have rand 0.10.
    let random_auths = authorities.choose_multiple(rng, authorities.len());

    // Use this struct to calculate delays between iterations.
    let mut retry_delay = RetryDelay::default();

    // Go over each authority in randomized ordered until the first succeeds.
    for endpoints in random_auths {
        if endpoints.is_empty() {
            warn!("authority without endpoints?");
            continue;
        }

        match request_single(endpoints, req).await {
            Ok(resp) => {
                debug!(
                    "request {req:?} to {endpoints:?} succeeded: received {} bytes",
                    resp.len()
                );
                return Ok(resp);
            }
            Err(e) => {
                let delay = retry_delay.next_delay(rng);
                debug!(
                    "request {req:?} to {endpoints:?} failed: {e}, trying next in {}s",
                    delay.as_secs()
                );
                err.push(e);
                tokio::time::sleep(delay).await;
            }
        }
    }

    // All attempts have failed, give up and return the errors.
    Err(err)
}

/// Runs forever in the current task, performing the core operation of a directory mirror.
///
/// This function runs forever in the current task, continously downloading
/// network documents from authorities and inserting them into the database,
/// while also performing garbage collection.
///
/// A core principle of this function is to be safe against power-losses, sudden
/// abortions, and such.  This means that re-starting this function will resume
/// seaminglessly from where it stopped.
///
/// # Algorithm
///
/// 1. Call [`get_recent_consensus()`] to obtain the most recent and non-expired
///    consensus from the database.
///     1. If the call returns [`Some`], goto (2).
///     2. If the call returns [`None`], goto (3).
/// 2. Spawn a task backed by [`tokio::time::timeout()`] whose purpose it is to
///    determine all missing network documents referred to by the current
///    consensus and scheduling downloads from the authorities in order to
///    obtain and insert them into the database.
///    TODO DIRMIRROR: Impose a timeout for each download attempt.
/// 3. Download a new consensus from the directory authorities and insert it into
///    the database.
/// 4. Perform a cycle of garbage collection.
/// 5. Goto (1).
///
/// # Specifications
///
/// * <https://spec.torproject.org/dir-spec/directory-cache-operation.html#download-desc-from-auth>
/// * <https://spec.torproject.org/dir-spec/client-operation.html#retrying-failed-downloads>
pub(super) async fn serve<R: Rng, F: Fn() -> SystemTime>(
    pool: Pool<SqliteConnectionManager>,
    flavor: ConsensusFlavor,
    _authorities: AuthorityContacts,
    _schedule: DownloadScheduleConfig,
    tolerance: DirTolerance,
    rng: &mut R,
    now_fn: F,
) -> Result<(), FatalError> {
    loop {
        let now = SaturatingSystemTime::saturating_convert(now_fn());

        // (1) Call get_recent_consensus() to obtain the most recent and non-expired
        // consensus from the database.
        // TODO: Use `Result::flatten` once MSRV is 1.89.0.
        let res = database::read_tx(&pool, {
            let tolerance = tolerance.clone();
            move |tx| get_recent_consensus(tx, flavor, &tolerance, now)
        });
        let res = match res {
            Ok(Ok(res)) => res,
            Err(e) | Ok(Err(e)) => {
                return Err(FatalError::ConsensusSelection(e));
            }
        };

        // (1.1) If the call returns Some, goto (2).
        if let Some((_valid_after, fresh_until, valid_until, _consensus)) = res {
            // (2) Run a closure backed by tokio::time::timeout() with a lifetime
            // returned by by calculate_sync_timeout, whose purpose it is to
            // determine all missing network documents reffered to by the current
            // consensus and scheduling downloads from the authorities in order
            // to obtain and insert them into the database.
            let sync_timeout = calculate_sync_timeout(fresh_until, valid_until, now, rng);
            tokio::time::timeout(sync_timeout, async {
                // TODO DIRMIRROR: Actually download descriptors.
                // Ensure a good timeout to protect against malicious
                // authorities transmitting data extra slow; a download should
                // probably not take `sync_timeout` but a few minutes instead.
                // This implies a nested timeout.  The `sync_timeout` for the
                // outer layer and a smaller timeout for each download, in order
                // to ensure that each download actually gets the same amount
                // of time to exist, instead of just the first ones having
                // the full-time, with subsequent ones having a smaller and
                // smaller time.
                let _ = std::future::pending::<()>().await;
            })
            .await
            .expect_err("std::future::pending returned?");
        }

        // (3) Download a new consensus from the directory authorities and insert
        // it into the database.
        //
        // At this stage we also have to ask ourselves what happens in the highly
        // unlikely but still possible case that a new consensus could not be
        // retrieved, due to connectivity-loss (actually likely) or the even
        // more unlikely case of the directory authorities all being down and/or
        // unable to compute a new consensus.
        //
        // The specification is fairly clean on that (see the link above).
        // Downloads are retried with a variation of the "decorrelated jitter"
        // algorithm; that is, determining a certain amount of time before trying
        // again from an authority we have not tried yet.
        //
        // TODO: Actually download from authorities.

        // (4) Perform a cycle of garbage collection.
        // TODO: Actually perform a cycle of garbage collection.
    }
}

#[cfg(test)]
mod test {
    // @@ begin test lint list maintained by maint/add_warning @@
    #![allow(clippy::bool_assert_comparison)]
    #![allow(clippy::clone_on_copy)]
    #![allow(clippy::dbg_macro)]
    #![allow(clippy::mixed_attributes_style)]
    #![allow(clippy::print_stderr)]
    #![allow(clippy::print_stdout)]
    #![allow(clippy::single_char_pattern)]
    #![allow(clippy::unwrap_used)]
    #![allow(clippy::unchecked_duration_subtraction)]
    #![allow(clippy::useless_vec)]
    #![allow(clippy::needless_pass_by_value)]
    //! <!-- @@ end test lint list maintained by maint/add_warning @@ -->
    use std::{
        io::ErrorKind,
        sync::{
            atomic::{AtomicUsize, Ordering},
            Arc,
        },
    };

    use crate::database;

    use super::*;
    use lazy_static::lazy_static;
    use tokio::{
        io::{AsyncReadExt, AsyncWriteExt},
        net::TcpListener,
    };
    use tor_basic_utils::test_rng::testing_rng;
    use tor_dirclient::{request::ConsensusRequest, RequestError};
    use tor_dircommon::config::DirToleranceBuilder;

    lazy_static! {
    /// Wed Jan 01 2020 00:00:00 GMT+0000
    static ref VALID_AFTER: SaturatingSystemTime =
        SaturatingSystemTime::min().saturating_add(Duration::from_secs(1577836800));

    /// Wed Jan 01 2020 01:00:00 GMT+0000
    static ref FRESH_UNTIL: SaturatingSystemTime =
        VALID_AFTER.saturating_add(Duration::from_secs(60 * 60));

    /// Wed Jan 01 2020 02:00:00 GMT+0000
    static ref FRESH_UNTIL_HALF: SaturatingSystemTime =
        FRESH_UNTIL.saturating_add(Duration::from_secs(60 * 60));

    /// Wed Jan 01 2020 03:00:00 GMT+0000
    static ref VALID_UNTIL: SaturatingSystemTime =
        FRESH_UNTIL.saturating_add(Duration::from_secs(60 * 60 * 2));
    }

    const CONTENT: &str = "Lorem ipsum dolor sit amet.";
    const SHA256: &str = "DD14CBBF0E74909AAC7F248A85D190AFD8DA98265CEF95FC90DFDDABEA7C2E66";

    fn create_dummy_db() -> Pool<SqliteConnectionManager> {
        let pool = database::open("").unwrap();
        database::rw_tx(&pool, |tx| {
            tx.execute(
                sql!("INSERT INTO store (sha256, content) VALUES (?1, ?2)"),
                params![
                    "DD14CBBF0E74909AAC7F248A85D190AFD8DA98265CEF95FC90DFDDABEA7C2E66",
                    "Lorem ipsum dolor sit amet.".as_bytes()
                ],
            )
            .unwrap();

            tx.execute(
                sql!(
                    "
                    INSERT INTO consensus
                    (sha256, unsigned_sha3_256, flavor, valid_after, fresh_until, valid_until)
                    VALUES
                    (?1, ?2, ?3, ?4, ?5, ?6)
                    "
                ),
                params![
                    "DD14CBBF0E74909AAC7F248A85D190AFD8DA98265CEF95FC90DFDDABEA7C2E66",
                    "0000000000000000000000000000000000000000000000000000000000000000", // not the correct hash
                    ConsensusFlavor::Plain.name(),
                    *VALID_AFTER,
                    *FRESH_UNTIL,
                    *VALID_UNTIL,
                ],
            )
            .unwrap();
        })
        .unwrap();

        pool
    }

    #[test]
    fn saturating_system_time() {
        // Assert that the maximum is actually the maximum.
        assert_eq!(
            SaturatingSystemTime::max().saturating_add(Duration::new(0, 1)),
            SaturatingSystemTime::max()
        );

        // Assert that the minimum is actually the minimum.
        assert_eq!(
            SaturatingSystemTime::min().saturating_sub(Duration::new(0, 1)),
            SaturatingSystemTime::min()
        );

        // Check the we can always convert everything into an absolute Unix.
        assert_eq!(
            SaturatingSystemTime::max()
                .duration_since(*SaturatingSystemTime::min())
                .unwrap()
                .as_nanos(),
            9223372036854775807999999999
        );

        // Same as above but with our own function
        assert_eq!(
            SaturatingSystemTime::max().into_unix_time(),
            9223372036854775807
        );
        assert_eq!(SaturatingSystemTime::min().into_unix_time(), 0);
    }

    #[test]
    fn recent_consensus() {
        let pool = create_dummy_db();
        let no_tolerance = DirToleranceBuilder::default()
            .pre_valid_tolerance(Duration::ZERO)
            .post_valid_tolerance(Duration::ZERO)
            .build()
            .unwrap();
        let liberal_tolerance = DirToleranceBuilder::default()
            .pre_valid_tolerance(Duration::from_secs(60 * 60)) // 1h before
            .post_valid_tolerance(Duration::from_secs(60 * 60)) // 1h after
            .build()
            .unwrap();

        database::read_tx(&pool, move |tx| {
            // Get None by being way before valid-after.
            assert!(get_recent_consensus(
                tx,
                ConsensusFlavor::Plain,
                &no_tolerance,
                SaturatingSystemTime::min(),
            )
            .unwrap()
            .is_none());

            // Get None by being way behind valid-until.
            assert!(get_recent_consensus(
                tx,
                ConsensusFlavor::Plain,
                &no_tolerance,
                VALID_UNTIL.saturating_add(Duration::from_secs(60 * 60 * 24 * 365)),
            )
            .unwrap()
            .is_none());

            // Get None by being minimally before valid-after.
            assert!(get_recent_consensus(
                tx,
                ConsensusFlavor::Plain,
                &no_tolerance,
                VALID_AFTER.saturating_sub(Duration::from_secs(1)),
            )
            .unwrap()
            .is_none());

            // Get None by being minimally behind valid-until.
            assert!(get_recent_consensus(
                tx,
                ConsensusFlavor::Plain,
                &no_tolerance,
                VALID_UNTIL.saturating_add(Duration::from_secs(1)),
            )
            .unwrap()
            .is_none());

            // Get a valid consensus by being in the interval.
            let res1 =
                get_recent_consensus(tx, ConsensusFlavor::Plain, &no_tolerance, *VALID_AFTER)
                    .unwrap()
                    .unwrap();
            let res2 =
                get_recent_consensus(tx, ConsensusFlavor::Plain, &no_tolerance, *VALID_UNTIL)
                    .unwrap()
                    .unwrap();
            let res3 = get_recent_consensus(
                tx,
                ConsensusFlavor::Plain,
                &no_tolerance,
                VALID_AFTER.saturating_add(Duration::from_secs(60 * 30)),
            )
            .unwrap()
            .unwrap();
            assert_eq!(
                res1,
                (
                    *VALID_AFTER,
                    *FRESH_UNTIL,
                    *VALID_UNTIL,
                    CONTENT.to_string(),
                )
            );
            assert_eq!(res1, res2);
            assert_eq!(res2, res3);

            // Get a valid consensus using a liberal dir tolerance.
            let res1 = get_recent_consensus(
                tx,
                ConsensusFlavor::Plain,
                &liberal_tolerance,
                VALID_AFTER.saturating_sub(Duration::from_secs(60 * 30)),
            )
            .unwrap()
            .unwrap();
            let res2 = get_recent_consensus(
                tx,
                ConsensusFlavor::Plain,
                &liberal_tolerance,
                VALID_UNTIL.saturating_add(Duration::from_secs(60 * 30)),
            )
            .unwrap()
            .unwrap();
            assert_eq!(
                res1,
                (
                    *VALID_AFTER,
                    *FRESH_UNTIL,
                    *VALID_UNTIL,
                    CONTENT.to_string(),
                )
            );
            assert_eq!(res1, res2);
        })
        .unwrap();
    }

    #[test]
    fn sync_timeout() {
        // We repeat the tests a few thousand times to go over many random values.
        for _ in 0..10000 {
            let now = SaturatingSystemTime::min().saturating_add(Duration::from_secs(42));

            let dur = calculate_sync_timeout(*FRESH_UNTIL, *VALID_UNTIL, now, &mut testing_rng());
            assert!(
                dur.as_secs()
                    >= FRESH_UNTIL
                        .saturating_sub(Duration::from_secs(now.into_unix_time()))
                        .into_unix_time()
            );
            assert!(
                dur.as_secs()
                    <= FRESH_UNTIL_HALF
                        .saturating_sub(Duration::from_secs(now.into_unix_time()))
                        .into_unix_time()
            );
        }
    }

    /// Testing a request that is immediately successful.
    #[tokio::test]
    async fn request_legit() {
        let server = TcpListener::bind("[::]:0").await.unwrap();
        let server_addr = server.local_addr().unwrap();

        tokio::task::spawn(async move {
            let mut conn = server.accept().await.unwrap().0;
            let mut buf = vec![0; 1024];
            let _ = conn.read(&mut buf).await.unwrap();
            conn.write_all(b"HTTP/1.0 200 OK\r\nContent-Length: 3\r\n\r\nfoo")
                .await
                .unwrap();
        });

        let resp = request_multi(
            &vec![vec![server_addr]],
            &ConsensusRequest::new(ConsensusFlavor::Plain),
            &mut testing_rng(),
        )
        .await
        .unwrap();

        assert_eq!(resp, b"foo");
    }

    /// Testing for a request that initially fails by returning a 404 but later succeeds.
    #[tokio::test]
    async fn request_fail_but_succeed() {
        let mut server_addrs = Vec::new();
        let requ_counter = Arc::new(AtomicUsize::new(0));
        for _ in 0..2 {
            let server = TcpListener::bind("[::]:0").await.unwrap();
            let server_addr = server.local_addr().unwrap();
            let requ_counter = requ_counter.clone();
            server_addrs.push(vec![server_addr]);

            tokio::task::spawn(async move {
                loop {
                    let (mut conn, _) = server.accept().await.unwrap();

                    // This read is important!
                    // Otherwise this server will terminate the connection with
                    // RST instead of FIN, causing everything to fail.
                    let mut buf = vec![0; 1024];
                    let _ = conn.read(&mut buf).await.unwrap();

                    let cur_req = requ_counter.fetch_add(1, Ordering::AcqRel);

                    if cur_req == 0 {
                        // Send a failure.
                        conn.write_all(b"HTTP/1.0 404 Not Found\r\n\r\n")
                            .await
                            .unwrap();
                    } else if cur_req == 1 {
                        // Send a success.
                        conn.write_all(b"HTTP/1.0 200 OK\r\nContent-Length: 3\r\n\r\nfoo")
                            .await
                            .unwrap();
                    } else {
                        unreachable!()
                    }
                }
            });
        }

        let resp = request_multi(
            &server_addrs,
            &ConsensusRequest::new(ConsensusFlavor::Plain),
            &mut testing_rng(),
        )
        .await
        .unwrap();

        assert_eq!(resp, b"foo");
    }

    /// Request that fails all the time.
    #[tokio::test]
    async fn request_fail_ultimately() {
        let mut server_addrs = Vec::new();
        for _ in 0..2 {
            let server = TcpListener::bind("[::]:0").await.unwrap();
            let server_addr = server.local_addr().unwrap();
            server_addrs.push(vec![server_addr]);

            tokio::task::spawn(async move {
                loop {
                    let _ = server.accept().await.unwrap();
                }
            });
        }

        let errs = request_multi(
            &server_addrs,
            &ConsensusRequest::new(ConsensusFlavor::Plain),
            &mut testing_rng(),
        )
        .await
        .unwrap_err();

        // This is just a longer loop to assert all errors are connection resets.
        for err in errs {
            match err {
                AuthorityCommunicationError::RequestFailed(e) => match e.error {
                    RequestError::IoError(io) => match io.kind() {
                        ErrorKind::ConnectionReset => {}
                        e => unreachable!("{e}"),
                    },
                    e => unreachable!("{e}"),
                },
                e => unreachable!("{e}"),
            };
        }
    }

    /// Stress out the retry algorithm by letting a timeout kill it.
    #[tokio::test]
    async fn request_fail_timeout() {
        let mut servers = Vec::new();
        let mut addrs = Vec::new();

        for _ in 0..8 {
            let server = TcpListener::bind("[::]:0").await.unwrap();
            addrs.push(vec![server.local_addr().unwrap()]);
            servers.push(server);
        }

        let _elapsed = tokio::time::timeout(
            Duration::from_secs(5),
            request_multi(
                &addrs,
                &ConsensusRequest::new(ConsensusFlavor::Plain),
                &mut testing_rng(),
            ),
        )
        .await
        .unwrap_err();
    }

    #[tokio::test]
    #[should_panic]
    async fn empty_endpoints() {
        let _ = request_single(&Vec::new(), &ConsensusRequest::new(ConsensusFlavor::Plain)).await;
    }
}
