//! Conversion of Windows file times at host boundaries.

pub fn from_filetime(ticks: u64) -> Result<jiff::Timestamp, jiff::Error> {
    let ticks = i128::from(ticks) - 116_444_736_000_000_000;
    jiff::Timestamp::new((ticks / 10_000_000) as i64, ((ticks % 10_000_000) * 100) as i32)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn filetime_epoch_and_subsecond_ticks_are_exact() {
        assert_eq!(from_filetime(116_444_736_000_000_000).unwrap().as_nanosecond(), 0);
        assert_eq!(from_filetime(116_444_736_000_000_001).unwrap().as_nanosecond(), 100);
        assert_eq!(from_filetime(116_444_735_999_999_999).unwrap().as_nanosecond(), -100);
        assert!(from_filetime(u64::MAX).is_err());
    }
}
