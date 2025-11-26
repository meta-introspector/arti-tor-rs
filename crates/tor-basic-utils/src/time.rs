//! An enhancement to [`std::time`] for operations not offered by it.

use std::{
    cmp,
    ops::Deref,
    sync::LazyLock,
    time::{Duration, SystemTime},
};

/// The maximum value of [`SystemTime`] for this platform.
///
/// There is an active effort to upstream this into Rust STD:
/// * <https://github.com/rust-lang/rust/pull/148825>
/// * <https://gitlab.torproject.org/tpo/core/arti/-/issues/2248>
pub static MAX_SYSTEM_TIME: LazyLock<SystemTime> =
    LazyLock::new(|| find_system_time_limit(SystemTime::checked_add));

/// The minimum value of [`SystemTime`] for this platform.
///
/// There is an active effort to upstream this into Rust STD:
/// * <https://github.com/rust-lang/rust/pull/148825>
/// * <https://gitlab.torproject.org/tpo/core/arti/-/issues/2248>
pub static MIN_SYSTEM_TIME: LazyLock<SystemTime> =
    LazyLock::new(|| find_system_time_limit(SystemTime::checked_sub));

/// An algorithm that calulates the maximum/minimum [`SystemTime`].
///
/// It works by ± a large duration onto [`SystemTime::UNIX_EPOCH`], until this
/// operation fails, in which case this large duration will be halved, until it
/// reached `1ns`, in which case the algorithm will terminate if it another ±
/// fails.
fn find_system_time_limit<F>(f: F) -> SystemTime
where
    F: Fn(&SystemTime, Duration) -> Option<SystemTime>,
{
    const INITIAL_STEP: Duration = Duration::new(1_000_000_000_000_000_000, 0);
    const ONE_NS: Duration = Duration::new(0, 1);

    let mut step = INITIAL_STEP;
    let mut limit = SystemTime::UNIX_EPOCH;
    loop {
        match f(&limit, step) {
            Some(st) => limit = st,
            None => {
                if step == ONE_NS {
                    break;
                } else {
                    step = cmp::max(step / 2, ONE_NS);
                }
            }
        }
    }

    limit
}

/// A saturating wrapper around [`SystemTime`].
///
/// This type implements a wrapper around [`SystemTime`] in order to provide
/// implementations for [`SaturatingSystemTime::saturating_add()`] as well as
/// [`SaturatingSystemTime::saturating_sub()`].  Those functions behave similar
/// to [`Duration::saturating_add()`] and [`Duration::saturating_sub()`]
/// respectively.
///
/// Additionally, this type also implements [`Deref`] into [`SystemTime`]
/// alongside [`From`] for [`SystemTime`] in order to allow a seamless
/// interaction between these two types.
#[derive(Copy, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub struct SaturatingSystemTime(SystemTime);

impl SaturatingSystemTime {
    /// Securely adds a [`Duration`] to a [`SaturatingSystemTime`].
    ///
    /// This method adds a [`Duration`] to a [`SaturatingSystemTime`], returning
    /// [`SaturatingSystemTime::max()`] in the case of the result being
    /// unrepresentable by a [`SystemTime`].
    pub fn saturating_add(self, duration: Duration) -> Self {
        Self(self.0.checked_add(duration).unwrap_or(*MAX_SYSTEM_TIME))
    }

    /// Securely subtracts a [`Duration`] from a [`SaturatingSystemTime`].
    ///
    /// This method removes a [`Duration`] from a [`SaturatingSystemTime`],
    /// returning [`SaturatingSystemTime::min()`] in the case of the result
    /// being unrepresentable by a [`SystemTime`].
    pub fn saturating_sub(self, duration: Duration) -> Self {
        Self(self.0.checked_sub(duration).unwrap_or(*MIN_SYSTEM_TIME))
    }

    /// Converts a [`SaturatingSystemTime`] into a [`i64`] representing the
    /// seconds since epoch.
    ///
    /// Nanoseconds are chopped off and rounded down, i.e. `0.999_999_999` will
    /// become `0`.  Likewise `-1.9` will become `-1`.
    ///
    /// If the [`SaturatingSystemTime`] is before the epoch, it will try to
    /// return a negative value or [`i64::MIN`] as a fallback.
    pub fn into_unix_time(self) -> i64 {
        match self.0.duration_since(SystemTime::UNIX_EPOCH) {
            Ok(unix) => unix.as_secs().try_into().unwrap_or(i64::MAX),
            Err(e) => match TryInto::<i64>::try_into(e.duration().as_secs()) {
                Ok(unix) => -unix,
                Err(_) => i64::MIN,
            },
        }
    }
}

impl Deref for SaturatingSystemTime {
    type Target = SystemTime;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl From<SystemTime> for SaturatingSystemTime {
    fn from(value: SystemTime) -> Self {
        Self(value)
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
    #![allow(clippy::unchecked_time_subtraction)]
    #![allow(clippy::useless_vec)]
    #![allow(clippy::needless_pass_by_value)]
    //! <!-- @@ end test lint list maintained by maint/add_warning @@ -->
    use super::*;

    #[test]
    fn min_max_systemtime() {
        let max = *MAX_SYSTEM_TIME;
        let min = *MIN_SYSTEM_TIME;

        // First, test everything with checked_* and Duration::ZERO.
        assert_eq!(max.checked_add(Duration::ZERO), Some(max));
        assert_eq!(max.checked_sub(Duration::ZERO), Some(max));
        assert_eq!(min.checked_add(Duration::ZERO), Some(min));
        assert_eq!(min.checked_sub(Duration::ZERO), Some(min));

        // Now do the same again with checked_* but try by ± a single nanosecond.
        assert!(max.checked_add(Duration::new(0, 1)).is_none());
        assert!(max.checked_sub(Duration::new(0, 1)).is_some());
        assert!(min.checked_add(Duration::new(0, 1)).is_some());
        assert!(min.checked_sub(Duration::new(0, 1)).is_none());
    }

    #[test]
    #[cfg(target_family = "unix")]
    fn unix_min_max_systemtime() {
        let max = *MAX_SYSTEM_TIME;
        let min = *MIN_SYSTEM_TIME;

        assert_eq!(
            max.duration_since(SystemTime::UNIX_EPOCH).unwrap(),
            Duration::new(i64::MAX as u64, 999_999_999)
        );
        assert_eq!(
            min.duration_since(SystemTime::UNIX_EPOCH)
                .unwrap_err()
                .duration(),
            Duration::new(i64::MAX as u64 + 1, 0)
        );
    }

    #[test]
    fn saturating_system_time() {
        let max: SaturatingSystemTime = (*MAX_SYSTEM_TIME).into();
        let min: SaturatingSystemTime = (*MIN_SYSTEM_TIME).into();

        assert!(max.checked_add(Duration::new(0, 1)).is_none());
        assert!(min.checked_sub(Duration::new(0, 1)).is_none());
        assert!(max.checked_add(Duration::ZERO).is_some());
        assert!(min.checked_sub(Duration::ZERO).is_some());

        assert_eq!(max.saturating_add(Duration::new(0, 1)), max);
        assert_eq!(min.saturating_sub(Duration::new(0, 1)), min);
        assert_eq!(max.saturating_add(Duration::ZERO), max);
        assert_eq!(max.saturating_add(Duration::ZERO), max);

        assert_eq!(
            max.saturating_sub(Duration::new(0, 1)),
            max.checked_sub(Duration::new(0, 1)).unwrap().into()
        );
        assert_eq!(
            min.saturating_add(Duration::new(0, 1)),
            min.checked_add(Duration::new(0, 1)).unwrap().into()
        );
    }

    #[test]
    #[cfg(target_family = "unix")]
    fn unix_into_unix_time() {
        let epoch: SaturatingSystemTime = SystemTime::UNIX_EPOCH.into();
        let max: SaturatingSystemTime = (*MAX_SYSTEM_TIME).into();
        let min: SaturatingSystemTime = (*MIN_SYSTEM_TIME).into();

        assert_eq!(epoch.into_unix_time(), 0);
        assert_eq!(
            epoch.saturating_add(Duration::new(64, 42)).into_unix_time(),
            64
        );
        assert_eq!(
            epoch.saturating_sub(Duration::new(0, 1)).into_unix_time(),
            -0
        );
        assert_eq!(
            epoch.saturating_sub(Duration::new(64, 0)).into_unix_time(),
            -64
        );
        assert_eq!(
            epoch.saturating_sub(Duration::new(64, 42)).into_unix_time(),
            -64
        );

        assert_eq!(max.into_unix_time(), i64::MAX);
        assert_eq!(
            min.saturating_add(Duration::from_secs(1)).into_unix_time(),
            i64::MIN + 1
        );
        assert_eq!(min.into_unix_time(), i64::MIN);
    }
}
