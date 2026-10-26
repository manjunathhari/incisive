use std::time::{Duration, SystemTime, UNIX_EPOCH};

use crate::ClaudeError;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RetryPolicy {
    pub max_attempts: u32,
    pub base_delay: Duration,
    pub max_delay: Duration,
}

impl Default for RetryPolicy {
    fn default() -> Self {
        RetryPolicy {
            max_attempts: 3,
            base_delay: Duration::from_millis(500),
            max_delay: Duration::from_secs(8),
        }
    }
}

const RETRYABLE_STATUSES: [u16; 6] = [429, 500, 502, 503, 504, 529];

/// Whether another attempt could plausibly succeed.
pub(crate) fn is_retryable(err: &ClaudeError) -> bool {
    match err {
        ClaudeError::RateLimited { .. } | ClaudeError::Overloaded => true,
        ClaudeError::Api { status, .. } | ClaudeError::Decode { status, .. } => {
            RETRYABLE_STATUSES.contains(status)
        }
        ClaudeError::Transport(e) => e.is_timeout() || e.is_connect(),
    }
}

/// HTTP status to report in retry logs, if the error has one.
pub(crate) fn status_of(err: &ClaudeError) -> Option<u16> {
    match err {
        ClaudeError::Api { status, .. } | ClaudeError::Decode { status, .. } => Some(*status),
        ClaudeError::RateLimited { .. } => Some(429),
        ClaudeError::Overloaded => Some(529),
        ClaudeError::Transport(_) => None,
    }
}

impl RetryPolicy {
    /// Delay before the next attempt. `attempt` is the 1-based number of the
    /// attempt that just failed.
    pub(crate) fn delay_for(&self, attempt: u32, retry_after: Option<Duration>) -> Duration {
        if let Some(d) = retry_after
            && d <= self.max_delay
        {
            return d;
        }
        let exp = attempt.saturating_sub(1).min(20);
        let raw = self.base_delay.saturating_mul(1u32 << exp);
        jittered(raw, jitter_unit()).min(self.max_delay)
    }
}

/// Scale `d` by a factor in [0.8, 1.2] chosen by `unit` in [0, 1].
fn jittered(d: Duration, unit: f64) -> Duration {
    d.mul_f64(0.8 + 0.4 * unit.clamp(0.0, 1.0))
}

/// Cheap non-cryptographic value in [0, 1) from the clock; jitter needs no more.
fn jitter_unit() -> f64 {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.subsec_nanos())
        .unwrap_or(0);
    f64::from(nanos % 1000) / 1000.0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_match_spec() {
        let p = RetryPolicy::default();
        assert_eq!(p.max_attempts, 3);
        assert_eq!(p.base_delay, Duration::from_millis(500));
        assert_eq!(p.max_delay, Duration::from_secs(8));
    }

    #[test]
    fn retry_after_within_cap_is_used() {
        let p = RetryPolicy::default();
        assert_eq!(
            p.delay_for(1, Some(Duration::from_secs(2))),
            Duration::from_secs(2)
        );
    }

    #[test]
    fn retry_after_above_cap_falls_back_to_backoff() {
        let p = RetryPolicy::default();
        let d = p.delay_for(1, Some(Duration::from_secs(60)));
        assert!(d >= Duration::from_millis(400) && d <= Duration::from_millis(600));
    }

    #[test]
    fn backoff_grows_and_is_capped() {
        let p = RetryPolicy::default();
        let d2 = p.delay_for(2, None);
        assert!(d2 >= Duration::from_millis(800) && d2 <= Duration::from_millis(1200));
        assert_eq!(p.delay_for(10, None), p.max_delay);
    }

    #[test]
    fn jitter_bounds() {
        let d = Duration::from_secs(1);
        assert_eq!(jittered(d, 0.0), Duration::from_millis(800));
        assert_eq!(jittered(d, 1.0), Duration::from_millis(1200));
    }

    #[test]
    fn classification() {
        assert!(is_retryable(&ClaudeError::Overloaded));
        assert!(is_retryable(&ClaudeError::Decode {
            status: 502,
            body_snippet: String::new()
        }));
        assert!(!is_retryable(&ClaudeError::Api {
            status: 400,
            error_type: "invalid_request_error".into(),
            message: String::new()
        }));
        assert!(!is_retryable(&ClaudeError::Decode {
            status: 200,
            body_snippet: String::new()
        }));
    }
}
