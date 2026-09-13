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
            format!("Stack Exchange {operation} is cooling down after API backoff"),
        );
        error.retry_after_seconds = Some(remaining);
        Err(error)
    }

    pub fn observe_backoff(&self, operation: &'static str, seconds: Option<u64>) {
        let Some(seconds) = seconds else {
            return;
        };
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
        cooldowns.observe_backoff("search", Some(5));

        assert!(
            cooldowns
                .check(SourceId::new("stack-exchange").unwrap(), "search")
                .is_err()
        );
        *clock.now.lock().unwrap() = start + Duration::from_secs(5);
        assert!(
            cooldowns
                .check(SourceId::new("stack-exchange").unwrap(), "search")
                .is_ok()
        );
    }
}
