use std::collections::{HashMap, HashSet};
use std::net::IpAddr;
use std::time::{Duration, Instant};

const MIN_RATE_WINDOW: Duration = Duration::from_millis(1);
const MIN_BLOCK_DURATION: Duration = Duration::from_millis(1);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BlockReason {
    RateExceeded,
    Manual,
    HandshakeHeuristic,
    CookieMismatchGuard,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RateLimitDecision {
    Allow,
    GlobalLimit,
    IpBlocked {
        newly_blocked: bool,
        reason: BlockReason,
    },
}

#[derive(Debug, Clone, Copy, Default)]
struct IpWindowState {
    count: usize,
}

#[derive(Debug, Clone, Copy)]
struct BlockState {
    blocked_until: Option<Instant>,
    reason: BlockReason,
}

#[derive(Debug, Clone, Copy, Default)]
struct RateLimiterMetrics {
    global_limit_hits: u64,
    ip_block_hits: u64,
    ip_block_hits_rate_exceeded: u64,
    ip_block_hits_manual: u64,
    ip_block_hits_handshake_heuristic: u64,
    ip_block_hits_cookie_mismatch_guard: u64,
    addresses_blocked: u64,
    addresses_blocked_rate_exceeded: u64,
    addresses_blocked_manual: u64,
    addresses_blocked_handshake_heuristic: u64,
    addresses_blocked_cookie_mismatch_guard: u64,
    addresses_unblocked: u64,
}

#[derive(Debug, Clone, Copy, Default)]
pub struct RateLimiterMetricsSnapshot {
    pub global_limit_hits: u64,
    pub ip_block_hits: u64,
    pub ip_block_hits_rate_exceeded: u64,
    pub ip_block_hits_manual: u64,
    pub ip_block_hits_handshake_heuristic: u64,
    pub ip_block_hits_cookie_mismatch_guard: u64,
    pub addresses_blocked: u64,
    pub addresses_blocked_rate_exceeded: u64,
    pub addresses_blocked_manual: u64,
    pub addresses_blocked_handshake_heuristic: u64,
    pub addresses_blocked_cookie_mismatch_guard: u64,
    pub addresses_unblocked: u64,
    pub blocked_addresses: usize,
    pub exception_addresses: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RateLimiterConfigSnapshot {
    pub per_ip_limit: usize,
    pub global_limit: usize,
    pub window: Duration,
    pub block_duration: Duration,
}

pub struct RateLimiter {
    window_start: Instant,
    window_count: usize,
    per_ip_limit: usize,
    global_limit: usize,
    window: Duration,
    block_duration: Duration,
    ip_state: HashMap<IpAddr, IpWindowState>,
    blocked: HashMap<IpAddr, BlockState>,
    exceptions: HashSet<IpAddr>,
    metrics: RateLimiterMetrics,
}

impl RateLimiter {
    pub fn new(
        per_ip_limit: usize,
        global_limit: usize,
        window: Duration,
        block_duration: Duration,
    ) -> Self {
        let (per_ip_limit, global_limit, window, block_duration) =
            normalize_limits(per_ip_limit, global_limit, window, block_duration);
        Self {
            window_start: Instant::now(),
            window_count: 0,
            per_ip_limit,
            global_limit,
            window,
            block_duration,
            ip_state: HashMap::new(),
            blocked: HashMap::new(),
            exceptions: HashSet::new(),
            metrics: RateLimiterMetrics::default(),
        }
    }

    pub fn tick(&mut self, now: Instant) {
        self.rotate_window_if_needed(now);
        self.prune_expired_blocks(now);
    }

    pub fn check(&mut self, ip: IpAddr, now: Instant) -> RateLimitDecision {
        self.tick(now);

        if self.exceptions.contains(&ip) {
            self.window_count += 1;
            return RateLimitDecision::Allow;
        }

        if let Some(reason) = self.blocked.get(&ip).map(|state| state.reason) {
            self.record_ip_block_hit(reason);
            return RateLimitDecision::IpBlocked {
                newly_blocked: false,
                reason,
            };
        }

        if self.window_count >= self.global_limit {
            self.metrics.global_limit_hits = self.metrics.global_limit_hits.saturating_add(1);
            return RateLimitDecision::GlobalLimit;
        }

        let state = self.ip_state.entry(ip).or_default();
        if state.count >= self.per_ip_limit {
            let reason = BlockReason::RateExceeded;
            let newly_blocked =
                self.block_address_for_with_reason(ip, now, self.block_duration, reason);
            self.record_ip_block_hit(reason);
            return RateLimitDecision::IpBlocked {
                newly_blocked,
                reason,
            };
        }

        state.count += 1;
        self.window_count += 1;
        RateLimitDecision::Allow
    }

    pub fn add_exception(&mut self, ip: IpAddr) {
        self.exceptions.insert(ip);
        if self.blocked.remove(&ip).is_some() {
            self.metrics.addresses_unblocked = self.metrics.addresses_unblocked.saturating_add(1);
        }
    }

    pub fn remove_exception(&mut self, ip: IpAddr) {
        self.exceptions.remove(&ip);
    }

    pub fn set_per_ip_limit(&mut self, per_ip_limit: usize) {
        self.per_ip_limit = per_ip_limit.max(1);
        self.reset_window_state(Instant::now());
    }

    pub fn set_global_limit(&mut self, global_limit: usize) {
        self.global_limit = global_limit.max(1);
        self.reset_window_state(Instant::now());
    }

    pub fn set_window(&mut self, window: Duration) {
        self.window = normalize_duration(window, MIN_RATE_WINDOW);
        self.reset_window_state(Instant::now());
    }

    pub fn set_block_duration(&mut self, block_duration: Duration) {
        self.block_duration = normalize_duration(block_duration, MIN_BLOCK_DURATION);
    }

    pub fn update_limits(
        &mut self,
        per_ip_limit: usize,
        global_limit: usize,
        window: Duration,
        block_duration: Duration,
    ) {
        let (per_ip_limit, global_limit, window, block_duration) =
            normalize_limits(per_ip_limit, global_limit, window, block_duration);
        self.per_ip_limit = per_ip_limit;
        self.global_limit = global_limit;
        self.window = window;
        self.block_duration = block_duration;
        self.reset_window_state(Instant::now());
    }

    pub fn config_snapshot(&self) -> RateLimiterConfigSnapshot {
        RateLimiterConfigSnapshot {
            per_ip_limit: self.per_ip_limit,
            global_limit: self.global_limit,
            window: self.window,
            block_duration: self.block_duration,
        }
    }

    pub fn block_address(&mut self, ip: IpAddr) -> bool {
        self.block_address_with_reason(ip, BlockReason::Manual)
    }

    pub fn block_address_with_reason(&mut self, ip: IpAddr, reason: BlockReason) -> bool {
        if self.exceptions.contains(&ip) {
            return false;
        }

        let newly_blocked = !self.blocked.contains_key(&ip);
        self.blocked.insert(
            ip,
            BlockState {
                blocked_until: None,
                reason,
            },
        );
        if newly_blocked {
            self.record_new_block(reason);
        }
        newly_blocked
    }

    pub fn block_address_for(&mut self, ip: IpAddr, now: Instant, duration: Duration) -> bool {
        self.block_address_for_with_reason(ip, now, duration, BlockReason::Manual)
    }

    pub fn block_address_for_with_reason(
        &mut self,
        ip: IpAddr,
        now: Instant,
        duration: Duration,
        reason: BlockReason,
    ) -> bool {
        if self.exceptions.contains(&ip) {
            return false;
        }

        let duration = normalize_duration(duration, MIN_BLOCK_DURATION);
        let newly_blocked = !self.blocked.contains_key(&ip);
        self.blocked.insert(
            ip,
            BlockState {
                blocked_until: Some(now + duration),
                reason,
            },
        );
        if newly_blocked {
            self.record_new_block(reason);
        }
        newly_blocked
    }

    pub fn unblock_address(&mut self, ip: IpAddr) -> bool {
        if self.blocked.remove(&ip).is_some() {
            self.metrics.addresses_unblocked = self.metrics.addresses_unblocked.saturating_add(1);
            return true;
        }
        false
    }

    pub fn metrics_snapshot(&self) -> RateLimiterMetricsSnapshot {
        RateLimiterMetricsSnapshot {
            global_limit_hits: self.metrics.global_limit_hits,
            ip_block_hits: self.metrics.ip_block_hits,
            ip_block_hits_rate_exceeded: self.metrics.ip_block_hits_rate_exceeded,
            ip_block_hits_manual: self.metrics.ip_block_hits_manual,
            ip_block_hits_handshake_heuristic: self.metrics.ip_block_hits_handshake_heuristic,
            ip_block_hits_cookie_mismatch_guard: self.metrics.ip_block_hits_cookie_mismatch_guard,
            addresses_blocked: self.metrics.addresses_blocked,
            addresses_blocked_rate_exceeded: self.metrics.addresses_blocked_rate_exceeded,
            addresses_blocked_manual: self.metrics.addresses_blocked_manual,
            addresses_blocked_handshake_heuristic: self
                .metrics
                .addresses_blocked_handshake_heuristic,
            addresses_blocked_cookie_mismatch_guard: self
                .metrics
                .addresses_blocked_cookie_mismatch_guard,
            addresses_unblocked: self.metrics.addresses_unblocked,
            blocked_addresses: self.blocked.len(),
            exception_addresses: self.exceptions.len(),
        }
    }

    fn record_ip_block_hit(&mut self, reason: BlockReason) {
        self.metrics.ip_block_hits = self.metrics.ip_block_hits.saturating_add(1);
        match reason {
            BlockReason::RateExceeded => {
                self.metrics.ip_block_hits_rate_exceeded =
                    self.metrics.ip_block_hits_rate_exceeded.saturating_add(1);
            }
            BlockReason::Manual => {
                self.metrics.ip_block_hits_manual =
                    self.metrics.ip_block_hits_manual.saturating_add(1);
            }
            BlockReason::HandshakeHeuristic => {
                self.metrics.ip_block_hits_handshake_heuristic = self
                    .metrics
                    .ip_block_hits_handshake_heuristic
                    .saturating_add(1);
            }
            BlockReason::CookieMismatchGuard => {
                self.metrics.ip_block_hits_cookie_mismatch_guard = self
                    .metrics
                    .ip_block_hits_cookie_mismatch_guard
                    .saturating_add(1);
            }
        }
    }

    fn record_new_block(&mut self, reason: BlockReason) {
        self.metrics.addresses_blocked = self.metrics.addresses_blocked.saturating_add(1);
        match reason {
            BlockReason::RateExceeded => {
                self.metrics.addresses_blocked_rate_exceeded = self
                    .metrics
                    .addresses_blocked_rate_exceeded
                    .saturating_add(1);
            }
            BlockReason::Manual => {
                self.metrics.addresses_blocked_manual =
                    self.metrics.addresses_blocked_manual.saturating_add(1);
            }
            BlockReason::HandshakeHeuristic => {
                self.metrics.addresses_blocked_handshake_heuristic = self
                    .metrics
                    .addresses_blocked_handshake_heuristic
                    .saturating_add(1);
            }
            BlockReason::CookieMismatchGuard => {
                self.metrics.addresses_blocked_cookie_mismatch_guard = self
                    .metrics
                    .addresses_blocked_cookie_mismatch_guard
                    .saturating_add(1);
            }
        }
    }

    fn rotate_window_if_needed(&mut self, now: Instant) {
        if now.duration_since(self.window_start) < self.window {
            return;
        }

        self.reset_window_state(now);
    }

    fn reset_window_state(&mut self, now: Instant) {
        self.window_start = now;
        self.window_count = 0;
        self.ip_state.clear();
    }

    fn prune_expired_blocks(&mut self, now: Instant) {
        let mut unblocked = 0u64;
        self.blocked.retain(|_, state| {
            let Some(until) = state.blocked_until else {
                return true;
            };
            if now >= until {
                unblocked = unblocked.saturating_add(1);
                return false;
            }
            true
        });

        self.metrics.addresses_unblocked =
            self.metrics.addresses_unblocked.saturating_add(unblocked);
    }
}

fn normalize_limits(
    per_ip_limit: usize,
    global_limit: usize,
    window: Duration,
    block_duration: Duration,
) -> (usize, usize, Duration, Duration) {
    (
        per_ip_limit.max(1),
        global_limit.max(1),
        normalize_duration(window, MIN_RATE_WINDOW),
        normalize_duration(block_duration, MIN_BLOCK_DURATION),
    )
}

fn normalize_duration(value: Duration, minimum: Duration) -> Duration {
    if value.is_zero() { minimum } else { value }
}

#[cfg(test)]
mod tests {
    use std::net::{IpAddr, Ipv4Addr};
    use std::time::{Duration, Instant};

    use super::{BlockReason, RateLimitDecision, RateLimiter};

    #[test]
    fn per_ip_limit_transitions_to_blocked() {
        let mut limiter = RateLimiter::new(1, 100, Duration::from_secs(1), Duration::from_secs(5));
        let now = Instant::now();
        let ip = IpAddr::V4(Ipv4Addr::new(127, 0, 0, 1));

        assert_eq!(limiter.check(ip, now), RateLimitDecision::Allow);
        assert_eq!(
            limiter.check(ip, now),
            RateLimitDecision::IpBlocked {
                newly_blocked: true,
                reason: BlockReason::RateExceeded,
            }
        );
        assert_eq!(
            limiter.check(ip, now),
            RateLimitDecision::IpBlocked {
                newly_blocked: false,
                reason: BlockReason::RateExceeded,
            }
        );
    }

    #[test]
    fn global_limit_applies_across_ips() {
        let mut limiter = RateLimiter::new(10, 1, Duration::from_secs(1), Duration::from_secs(5));
        let now = Instant::now();

        assert_eq!(
            limiter.check(IpAddr::V4(Ipv4Addr::new(10, 0, 0, 1)), now),
            RateLimitDecision::Allow
        );
        assert_eq!(
            limiter.check(IpAddr::V4(Ipv4Addr::new(10, 0, 0, 2)), now),
            RateLimitDecision::GlobalLimit
        );
    }

    #[test]
    fn exception_bypasses_per_ip_limit() {
        let mut limiter = RateLimiter::new(1, 100, Duration::from_secs(1), Duration::from_secs(5));
        let now = Instant::now();
        let ip = IpAddr::V4(Ipv4Addr::new(127, 0, 0, 1));
        limiter.add_exception(ip);

        assert_eq!(limiter.check(ip, now), RateLimitDecision::Allow);
        assert_eq!(limiter.check(ip, now), RateLimitDecision::Allow);
        let metrics = limiter.metrics_snapshot();
        assert_eq!(metrics.exception_addresses, 1);
        assert_eq!(metrics.blocked_addresses, 0);
    }

    #[test]
    fn scheduled_unblock_happens_on_tick() {
        let mut limiter = RateLimiter::new(1, 100, Duration::from_secs(1), Duration::from_secs(2));
        let now = Instant::now();
        let ip = IpAddr::V4(Ipv4Addr::new(10, 0, 0, 1));

        assert_eq!(limiter.check(ip, now), RateLimitDecision::Allow);
        assert_eq!(
            limiter.check(ip, now),
            RateLimitDecision::IpBlocked {
                newly_blocked: true,
                reason: BlockReason::RateExceeded,
            }
        );
        assert_eq!(limiter.metrics_snapshot().blocked_addresses, 1);

        limiter.tick(now + Duration::from_secs(3));
        assert_eq!(limiter.metrics_snapshot().blocked_addresses, 0);
    }

    #[test]
    fn explicit_unblock_clears_block_state() {
        let mut limiter = RateLimiter::new(10, 100, Duration::from_secs(1), Duration::from_secs(5));
        let now = Instant::now();
        let ip = IpAddr::V4(Ipv4Addr::new(192, 168, 1, 9));

        assert!(limiter.block_address_for(ip, now, Duration::from_secs(30)));
        assert_eq!(limiter.metrics_snapshot().blocked_addresses, 1);
        assert!(limiter.unblock_address(ip));
        assert_eq!(limiter.metrics_snapshot().blocked_addresses, 0);
    }

    #[test]
    fn window_rotation_resets_global_and_per_ip_counters() {
        let mut limiter =
            RateLimiter::new(1, 3, Duration::from_secs(1), Duration::from_millis(400));
        let start = Instant::now();
        let ip_a = IpAddr::V4(Ipv4Addr::new(10, 1, 0, 1));
        let ip_b = IpAddr::V4(Ipv4Addr::new(10, 1, 0, 2));
        let ip_c = IpAddr::V4(Ipv4Addr::new(10, 1, 0, 3));

        assert_eq!(limiter.check(ip_a, start), RateLimitDecision::Allow);
        assert_eq!(limiter.check(ip_b, start), RateLimitDecision::Allow);
        assert_eq!(
            limiter.check(ip_b, start),
            RateLimitDecision::IpBlocked {
                newly_blocked: true,
                reason: BlockReason::RateExceeded,
            }
        );

        let next_window = start + Duration::from_secs(2);
        assert_eq!(limiter.check(ip_b, next_window), RateLimitDecision::Allow);
        assert_eq!(limiter.check(ip_c, next_window), RateLimitDecision::Allow);
    }

    #[test]
    fn removing_exception_restores_per_ip_enforcement() {
        let mut limiter = RateLimiter::new(1, 100, Duration::from_secs(1), Duration::from_secs(5));
        let start = Instant::now();
        let ip = IpAddr::V4(Ipv4Addr::new(172, 16, 0, 9));

        limiter.add_exception(ip);
        assert_eq!(limiter.check(ip, start), RateLimitDecision::Allow);
        assert_eq!(limiter.check(ip, start), RateLimitDecision::Allow);

        limiter.remove_exception(ip);
        let next_window = start + Duration::from_secs(2);
        assert_eq!(limiter.check(ip, next_window), RateLimitDecision::Allow);
        assert_eq!(
            limiter.check(ip, next_window),
            RateLimitDecision::IpBlocked {
                newly_blocked: true,
                reason: BlockReason::RateExceeded,
            }
        );
    }

    #[test]
    fn adding_exception_unblocks_address_and_updates_metrics() {
        let mut limiter = RateLimiter::new(1, 100, Duration::from_secs(1), Duration::from_secs(30));
        let now = Instant::now();
        let ip = IpAddr::V4(Ipv4Addr::new(203, 0, 113, 10));

        assert_eq!(limiter.check(ip, now), RateLimitDecision::Allow);
        assert_eq!(
            limiter.check(ip, now),
            RateLimitDecision::IpBlocked {
                newly_blocked: true,
                reason: BlockReason::RateExceeded,
            }
        );

        limiter.add_exception(ip);
        let metrics = limiter.metrics_snapshot();
        assert_eq!(metrics.blocked_addresses, 0);
        assert_eq!(metrics.exception_addresses, 1);
        assert_eq!(metrics.addresses_blocked, 1);
        assert_eq!(metrics.addresses_unblocked, 1);

        assert_eq!(limiter.check(ip, now), RateLimitDecision::Allow);
    }

    #[test]
    fn update_limits_clamps_zero_values_and_resets_window_state() {
        let mut limiter =
            RateLimiter::new(100, 100, Duration::from_secs(2), Duration::from_secs(5));
        let start = Instant::now();
        let ip = IpAddr::V4(Ipv4Addr::new(10, 2, 0, 1));

        assert_eq!(limiter.check(ip, start), RateLimitDecision::Allow);

        limiter.update_limits(0, 0, Duration::ZERO, Duration::ZERO);
        let cfg = limiter.config_snapshot();
        assert_eq!(cfg.per_ip_limit, 1);
        assert_eq!(cfg.global_limit, 1);
        assert_eq!(cfg.window, Duration::from_millis(1));
        assert_eq!(cfg.block_duration, Duration::from_millis(1));

        assert_eq!(limiter.check(ip, start), RateLimitDecision::Allow);
        assert_eq!(limiter.check(ip, start), RateLimitDecision::GlobalLimit);
    }

    #[test]
    fn permanent_block_stays_until_explicit_unblock() {
        let mut limiter =
            RateLimiter::new(10, 10, Duration::from_millis(10), Duration::from_secs(1));
        let ip = IpAddr::V4(Ipv4Addr::new(203, 0, 113, 42));
        let now = Instant::now();

        assert!(limiter.block_address(ip));
        assert_eq!(
            limiter.check(ip, now),
            RateLimitDecision::IpBlocked {
                newly_blocked: false,
                reason: BlockReason::Manual,
            }
        );

        limiter.tick(now + Duration::from_secs(60));
        assert_eq!(
            limiter.check(ip, now + Duration::from_secs(61)),
            RateLimitDecision::IpBlocked {
                newly_blocked: false,
                reason: BlockReason::Manual,
            }
        );

        assert!(limiter.unblock_address(ip));
        assert_eq!(
            limiter.check(ip, now + Duration::from_secs(62)),
            RateLimitDecision::Allow
        );
    }

    #[test]
    fn reason_specific_block_metrics_are_tracked() {
        let mut limiter = RateLimiter::new(1, 100, Duration::from_secs(1), Duration::from_secs(5));
        let now = Instant::now();

        let rate_ip = IpAddr::V4(Ipv4Addr::new(10, 20, 0, 1));
        let manual_ip = IpAddr::V4(Ipv4Addr::new(10, 20, 0, 2));
        let handshake_ip = IpAddr::V4(Ipv4Addr::new(10, 20, 0, 3));
        let cookie_ip = IpAddr::V4(Ipv4Addr::new(10, 20, 0, 4));

        assert_eq!(limiter.check(rate_ip, now), RateLimitDecision::Allow);
        assert_eq!(
            limiter.check(rate_ip, now),
            RateLimitDecision::IpBlocked {
                newly_blocked: true,
                reason: BlockReason::RateExceeded,
            }
        );

        assert!(limiter.block_address(manual_ip));
        assert_eq!(
            limiter.check(manual_ip, now),
            RateLimitDecision::IpBlocked {
                newly_blocked: false,
                reason: BlockReason::Manual,
            }
        );

        assert!(limiter.block_address_for_with_reason(
            handshake_ip,
            now,
            Duration::from_secs(30),
            BlockReason::HandshakeHeuristic
        ));
        assert_eq!(
            limiter.check(handshake_ip, now),
            RateLimitDecision::IpBlocked {
                newly_blocked: false,
                reason: BlockReason::HandshakeHeuristic,
            }
        );

        assert!(limiter.block_address_for_with_reason(
            cookie_ip,
            now,
            Duration::from_secs(30),
            BlockReason::CookieMismatchGuard
        ));
        assert_eq!(
            limiter.check(cookie_ip, now),
            RateLimitDecision::IpBlocked {
                newly_blocked: false,
                reason: BlockReason::CookieMismatchGuard,
            }
        );

        let metrics = limiter.metrics_snapshot();
        assert_eq!(metrics.ip_block_hits, 4);
        assert_eq!(metrics.ip_block_hits_rate_exceeded, 1);
        assert_eq!(metrics.ip_block_hits_manual, 1);
        assert_eq!(metrics.ip_block_hits_handshake_heuristic, 1);
        assert_eq!(metrics.ip_block_hits_cookie_mismatch_guard, 1);
        assert_eq!(metrics.addresses_blocked, 4);
        assert_eq!(metrics.addresses_blocked_rate_exceeded, 1);
        assert_eq!(metrics.addresses_blocked_manual, 1);
        assert_eq!(metrics.addresses_blocked_handshake_heuristic, 1);
        assert_eq!(metrics.addresses_blocked_cookie_mismatch_guard, 1);
    }
}
