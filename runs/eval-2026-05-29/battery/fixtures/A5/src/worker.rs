use crate::config::MAX_RETRIES;

/// Counts how many retry attempts would be made.
pub fn attempt_count() -> u32 {
    let mut count = 0;
    for _ in 0..MAX_RETRIES {
        count += 1;
    }
    count
}
