use crate::{CreatorError, Result};
use synapse_core::AuthorizationClock;
use synapse_core::SystemAuthorizationClock;

pub(crate) struct ProtocolTime {
    pub(crate) timestamp: String,
    pub(crate) unix_nanos: i64,
    seconds: i64,
    subsec_nanos: u32,
}

impl ProtocolTime {
    pub(crate) fn after_seconds(&self, delta: i64) -> Result<String> {
        let seconds = self
            .seconds
            .checked_add(delta)
            .ok_or_else(|| CreatorError::Clock("protocol timestamp overflow".into()))?;
        format_timestamp(seconds, self.subsec_nanos)
    }
}

#[derive(Default)]
pub(crate) struct RecordingClock {
    last_unix_nanos: Option<i128>,
}

impl RecordingClock {
    pub(crate) fn tick(&mut self) -> Result<ProtocolTime> {
        let observed = SystemAuthorizationClock
            .now_unix_nanos()
            .map_err(CreatorError::Clock)?;
        let logical = self
            .last_unix_nanos
            .and_then(|last| last.checked_add(1))
            .map_or(observed, |next| observed.max(next));
        self.last_unix_nanos = Some(logical);
        let unix_nanos = i64::try_from(logical).map_err(|_| {
            CreatorError::Clock("system time exceeds reflog nanosecond range".into())
        })?;
        let seconds = unix_nanos.div_euclid(1_000_000_000);
        let subsec_nanos = u32::try_from(unix_nanos.rem_euclid(1_000_000_000))
            .expect("nanosecond remainder is within u32");
        Ok(ProtocolTime {
            timestamp: format_timestamp(seconds, subsec_nanos)?,
            unix_nanos,
            seconds,
            subsec_nanos,
        })
    }
}

pub(crate) fn format_timestamp(seconds: i64, nanos: u32) -> Result<String> {
    if seconds < 0 {
        return Err(CreatorError::Clock(
            "creator Pilot requires a system clock after the Unix epoch".into(),
        ));
    }
    let days = seconds.div_euclid(86_400);
    let seconds_of_day = seconds.rem_euclid(86_400);
    let (year, month, day) = civil_from_days(days);
    if !(0..=9_999).contains(&year) {
        return Err(CreatorError::Clock(
            "system time is outside the four-digit protocol year range".into(),
        ));
    }
    let hour = seconds_of_day / 3_600;
    let minute = (seconds_of_day % 3_600) / 60;
    let second = seconds_of_day % 60;
    Ok(format!(
        "{year:04}-{month:02}-{day:02}T{hour:02}:{minute:02}:{second:02}.{nanos:09}Z"
    ))
}

pub(crate) fn civil_from_days(days_since_epoch: i64) -> (i64, i64, i64) {
    let z = days_since_epoch + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let day_of_era = z - era * 146_097;
    let year_of_era =
        (day_of_era - day_of_era / 1_460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let mut year = year_of_era + era * 400;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let month_prime = (5 * day_of_year + 2) / 153;
    let day = day_of_year - (153 * month_prime + 2) / 5 + 1;
    let month = month_prime + if month_prime < 10 { 3 } else { -9 };
    year += i64::from(month <= 2);
    (year, month, day)
}
