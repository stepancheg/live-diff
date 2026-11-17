use std::time::SystemTime;
use std::time::UNIX_EPOCH;

/// Whole seconds since the Unix epoch.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct TimestampsSecs(pub(crate) i64);

impl TimestampsSecs {
    pub(crate) fn now() -> TimestampsSecs {
        match SystemTime::now().duration_since(UNIX_EPOCH) {
            Ok(d) => match i64::try_from(d.as_secs()) {
                Ok(secs) => TimestampsSecs(secs),
                Err(_) => TimestampsSecs(i64::MAX),
            },
            Err(err) => match i64::try_from(err.duration().as_secs()) {
                Ok(secs) => TimestampsSecs(secs.saturating_neg()),
                Err(_) => TimestampsSecs(i64::MIN),
            },
        }
    }

    /// `self - other` as a jiff duration. Overflow saturates to
    /// [`jiff::SignedDuration::MAX`] or [`jiff::SignedDuration::MIN`].
    pub(crate) fn saturating_sub(self, other: TimestampsSecs) -> jiff::SignedDuration {
        match jiff::SignedDuration::from_secs(self.0)
            .checked_sub(jiff::SignedDuration::from_secs(other.0))
        {
            Some(duration) => duration,
            None => {
                if self.0 >= other.0 {
                    jiff::SignedDuration::MAX
                } else {
                    jiff::SignedDuration::MIN
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::util::time::ts_secs::TimestampsSecs;

    #[test]
    fn test_saturating_sub_clamps_overflow() {
        let hi = TimestampsSecs(i64::MAX);
        let lo = TimestampsSecs(i64::MIN);
        assert_eq!(jiff::SignedDuration::MAX, hi.saturating_sub(lo));
        assert_eq!(jiff::SignedDuration::MIN, lo.saturating_sub(hi));
        assert_eq!(
            jiff::SignedDuration::from_secs(5),
            TimestampsSecs(10).saturating_sub(TimestampsSecs(5))
        );
    }
}
