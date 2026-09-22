//! In-memory sliding-window rate limiter keyed by client IP.

use std::collections::HashMap;
use std::net::IpAddr;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

const PURGE_THRESHOLD: usize = 4096;

#[derive(Debug, Clone)]
pub struct RateLimiter {
    window: Duration,
    max: usize,
    hits: Arc<Mutex<HashMap<IpAddr, Vec<Instant>>>>,
}

impl RateLimiter {
    pub fn new(window: Duration, max: usize) -> Self {
        Self {
            window,
            max,
            hits: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    /// Record a hit for `ip`. `Err` means the client is over the limit.
    /// Fails open if the internal lock is poisoned.
    pub fn check(&self, ip: IpAddr) -> Result<(), ()> {
        let now = Instant::now();
        let mut map = match self.hits.lock() {
            Ok(g) => g,
            Err(_) => return Ok(()),
        };

        if map.len() > PURGE_THRESHOLD {
            let window = self.window;
            map.retain(|_, times| times.iter().any(|t| now.duration_since(*t) < window));
        }

        let entry = map.entry(ip).or_default();
        entry.retain(|t| now.duration_since(*t) < self.window);
        if entry.len() >= self.max {
            return Err(());
        }
        entry.push(now);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::{IpAddr, Ipv4Addr};

    fn ip(n: u8) -> IpAddr {
        IpAddr::V4(Ipv4Addr::new(127, 0, 0, n))
    }

    #[test]
    fn allows_up_to_max_then_blocks() {
        let limiter = RateLimiter::new(Duration::from_secs(60), 3);
        let addr = ip(1);
        assert!(limiter.check(addr).is_ok());
        assert!(limiter.check(addr).is_ok());
        assert!(limiter.check(addr).is_ok());
        assert!(limiter.check(addr).is_err());
    }

    #[test]
    fn keys_are_per_ip() {
        let limiter = RateLimiter::new(Duration::from_secs(60), 1);
        assert!(limiter.check(ip(1)).is_ok());
        assert!(limiter.check(ip(1)).is_err());
        assert!(limiter.check(ip(2)).is_ok());
    }

    #[test]
    fn window_expiry_frees_budget() {
        let limiter = RateLimiter::new(Duration::from_millis(10), 1);
        let addr = ip(1);
        assert!(limiter.check(addr).is_ok());
        assert!(limiter.check(addr).is_err());
        std::thread::sleep(Duration::from_millis(20));
        assert!(limiter.check(addr).is_ok());
    }
}
