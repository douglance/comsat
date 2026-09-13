use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

use comsat_types::{ErrorClass, SourceError, SourceId};
use web_time::{Duration, Instant};

pub trait Clock: Send + Sync {
    fn now(&self) -> Instant;
}

#[derive(Debug)]
pub struct SystemClock;

impl Clock for SystemClock {
    fn now(&self) -> Instant {
        Instant::now()
    }
}

pub struct Cooldowns {
    clock: Arc<dyn Clock>,
    until: Mutex<BTreeMap<&'static str, Instant>>,
}

impl Default for Cooldowns {
    fn default() -> Self {
        Self::new(Arc::new(SystemClock))
    }
}

impl Cooldowns {
    pub fn new(clock: Arc<dyn Clock>) -> Self {
        Self {
            clock,
            until: Mutex::default(),
        }
    }

    pub fn check(&self, source: SourceId, operation: &'static str) -> Result<(), SourceError> {
        let now = self.clock.now();
        let until = self
            .until
            .lock()
            .expect("cooldown mutex poisoned")
            .get(operation)
            .copied();
        let Some(until) = until else {
            return Ok(());
        };
        if until <= now {
            return Ok(());
        }
        let remaining = until.duration_since(now).as_secs().max(1);
        let mut error = SourceError::new(
            source,
            ErrorClass::RateLimit,
            format!("GitHub {operation} is cooling down after a rate limit"),
        );
        error.retry_after_seconds = Some(remaining);
        Err(error)
    }

    pub fn observe_error(&self, operation: &'static str, error: &SourceError) {
        if !matches!(error.class, ErrorClass::RateLimit) {
            return;
        }
        self.set(operation, error.retry_after_seconds.unwrap_or(60));
    }

    fn set(&self, operation: &'static str, seconds: u64) {
        self.until.lock().expect("cooldown mutex poisoned").insert(
            operation,
            self.clock.now() + Duration::from_secs(seconds.max(1)),
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct FakeClock {
        now: Mutex<Instant>,
    }

    impl Clock for FakeClock {
        fn now(&self) -> Instant {
            *self.now.lock().unwrap()
        }
    }

    #[test]
    fn cooldown_uses_injected_clock() {
        let start = Instant::now();
        let clock = Arc::new(FakeClock {
            now: Mutex::new(start),
        });
        let cooldowns = Cooldowns::new(clock.clone());
        let mut error = SourceError::new(
            SourceId::new("github").unwrap(),
            ErrorClass::RateLimit,
            "limited",
        );
        error.retry_after_seconds = Some(5);
        cooldowns.observe_error("search", &error);

        assert!(
            cooldowns
                .check(SourceId::new("github").unwrap(), "search")
                .is_err()
        );
        *clock.now.lock().unwrap() = start + Duration::from_secs(5);
        assert!(
            cooldowns
                .check(SourceId::new("github").unwrap(), "search")
                .is_ok()
        );
    }
}
