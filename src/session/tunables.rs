use std::time::Duration;

use crate::error::ConfigValidationError;
use crate::protocol::constants;

#[derive(Debug, Clone)]
pub struct SessionTunables {
    pub ack_queue_capacity: usize,
    pub reliable_window: usize,
    pub split_ttl: Duration,
    pub max_split_parts: u32,
    pub max_concurrent_splits: usize,
    pub max_ordering_channels: usize,
    pub max_ordered_pending_per_channel: usize,
    pub max_order_gap: u32,
    pub resend_rto: Duration,
    pub min_resend_rto: Duration,
    pub max_resend_rto: Duration,
    pub initial_congestion_window: f64,
    pub min_congestion_window: f64,
    pub max_congestion_window: f64,
    pub congestion_slow_start_threshold: f64,
    pub congestion_additive_gain: f64,
    pub congestion_multiplicative_decrease_nack: f64,
    pub congestion_multiplicative_decrease_timeout: f64,
    pub congestion_high_rtt_threshold_ms: f64,
    pub congestion_high_rtt_additive_scale: f64,
    pub congestion_nack_backoff_cooldown: Duration,
    pub pacing_enabled: bool,
    pub pacing_start_full: bool,
    pub pacing_gain: f64,
    pub pacing_min_rate_bytes_per_sec: f64,
    pub pacing_max_rate_bytes_per_sec: f64,
    pub pacing_max_burst_bytes: usize,
    pub outgoing_queue_max_frames: usize,
    pub outgoing_queue_max_bytes: usize,
    pub outgoing_queue_soft_ratio: f64,
}

impl Default for SessionTunables {
    fn default() -> Self {
        Self {
            ack_queue_capacity: 1024,
            reliable_window: constants::MAX_ACK_SEQUENCES as usize,
            split_ttl: Duration::from_millis(constants::SPLIT_REASSEMBLY_TTL_MS),
            max_split_parts: constants::MAX_SPLIT_PARTS,
            max_concurrent_splits: constants::MAX_INFLIGHT_SPLIT_COMPOUNDS_PER_PEER,
            max_ordering_channels: 16,
            max_ordered_pending_per_channel: 2048,
            max_order_gap: constants::MAX_ACK_SEQUENCES as u32,
            resend_rto: Duration::from_millis(250),
            min_resend_rto: Duration::from_millis(80),
            max_resend_rto: Duration::from_millis(2_000),
            initial_congestion_window: 64.0,
            min_congestion_window: 8.0,
            max_congestion_window: 1024.0,
            congestion_slow_start_threshold: 128.0,
            congestion_additive_gain: 1.0,
            congestion_multiplicative_decrease_nack: 0.85,
            congestion_multiplicative_decrease_timeout: 0.6,
            congestion_high_rtt_threshold_ms: 180.0,
            congestion_high_rtt_additive_scale: 0.6,
            congestion_nack_backoff_cooldown: Duration::from_millis(50),
            pacing_enabled: true,
            pacing_start_full: true,
            pacing_gain: 1.0,
            pacing_min_rate_bytes_per_sec: 24.0 * 1024.0,
            pacing_max_rate_bytes_per_sec: 32.0 * 1024.0 * 1024.0,
            pacing_max_burst_bytes: 128 * 1024,
            outgoing_queue_max_frames: 8192,
            outgoing_queue_max_bytes: 8 * 1024 * 1024,
            outgoing_queue_soft_ratio: 0.85,
        }
    }
}

impl SessionTunables {
    pub fn validate(&self) -> Result<(), ConfigValidationError> {
        if self.ack_queue_capacity == 0 {
            return Err(ConfigValidationError::new(
                "SessionTunables",
                "ack_queue_capacity",
                "must be >= 1",
            ));
        }
        if self.reliable_window == 0 {
            return Err(ConfigValidationError::new(
                "SessionTunables",
                "reliable_window",
                "must be >= 1",
            ));
        }
        if self.split_ttl.is_zero() {
            return Err(ConfigValidationError::new(
                "SessionTunables",
                "split_ttl",
                "must be > 0",
            ));
        }
        if self.max_split_parts == 0 {
            return Err(ConfigValidationError::new(
                "SessionTunables",
                "max_split_parts",
                "must be >= 1",
            ));
        }
        if self.max_concurrent_splits == 0 {
            return Err(ConfigValidationError::new(
                "SessionTunables",
                "max_concurrent_splits",
                "must be >= 1",
            ));
        }
        if self.max_ordering_channels == 0 {
            return Err(ConfigValidationError::new(
                "SessionTunables",
                "max_ordering_channels",
                "must be >= 1",
            ));
        }
        if self.max_ordered_pending_per_channel == 0 {
            return Err(ConfigValidationError::new(
                "SessionTunables",
                "max_ordered_pending_per_channel",
                "must be >= 1",
            ));
        }
        if self.max_order_gap == 0 {
            return Err(ConfigValidationError::new(
                "SessionTunables",
                "max_order_gap",
                "must be >= 1",
            ));
        }
        if self.min_resend_rto.is_zero() {
            return Err(ConfigValidationError::new(
                "SessionTunables",
                "min_resend_rto",
                "must be > 0",
            ));
        }
        if self.max_resend_rto.is_zero() {
            return Err(ConfigValidationError::new(
                "SessionTunables",
                "max_resend_rto",
                "must be > 0",
            ));
        }
        if self.min_resend_rto > self.max_resend_rto {
            return Err(ConfigValidationError::new(
                "SessionTunables",
                "min_resend_rto",
                "must be <= max_resend_rto",
            ));
        }
        if self.resend_rto < self.min_resend_rto || self.resend_rto > self.max_resend_rto {
            return Err(ConfigValidationError::new(
                "SessionTunables",
                "resend_rto",
                "must be within [min_resend_rto, max_resend_rto]",
            ));
        }

        validate_positive_f64(self.initial_congestion_window, "initial_congestion_window")?;
        validate_positive_f64(self.min_congestion_window, "min_congestion_window")?;
        validate_positive_f64(self.max_congestion_window, "max_congestion_window")?;
        if self.min_congestion_window > self.max_congestion_window {
            return Err(ConfigValidationError::new(
                "SessionTunables",
                "min_congestion_window",
                "must be <= max_congestion_window",
            ));
        }
        if self.initial_congestion_window < self.min_congestion_window
            || self.initial_congestion_window > self.max_congestion_window
        {
            return Err(ConfigValidationError::new(
                "SessionTunables",
                "initial_congestion_window",
                "must be within [min_congestion_window, max_congestion_window]",
            ));
        }
        if self.congestion_slow_start_threshold < self.min_congestion_window
            || self.congestion_slow_start_threshold > self.max_congestion_window
        {
            return Err(ConfigValidationError::new(
                "SessionTunables",
                "congestion_slow_start_threshold",
                "must be within [min_congestion_window, max_congestion_window]",
            ));
        }
        validate_positive_f64(self.congestion_additive_gain, "congestion_additive_gain")?;
        validate_fraction(
            self.congestion_multiplicative_decrease_nack,
            "congestion_multiplicative_decrease_nack",
        )?;
        validate_fraction(
            self.congestion_multiplicative_decrease_timeout,
            "congestion_multiplicative_decrease_timeout",
        )?;
        validate_positive_f64(
            self.congestion_high_rtt_threshold_ms,
            "congestion_high_rtt_threshold_ms",
        )?;
        if !self.congestion_high_rtt_additive_scale.is_finite()
            || self.congestion_high_rtt_additive_scale <= 0.0
            || self.congestion_high_rtt_additive_scale > 1.0
        {
            return Err(ConfigValidationError::new(
                "SessionTunables",
                "congestion_high_rtt_additive_scale",
                "must be finite and within (0, 1]",
            ));
        }
        if self.congestion_nack_backoff_cooldown.is_zero() {
            return Err(ConfigValidationError::new(
                "SessionTunables",
                "congestion_nack_backoff_cooldown",
                "must be > 0",
            ));
        }

        validate_positive_f64(self.pacing_gain, "pacing_gain")?;
        validate_positive_f64(
            self.pacing_min_rate_bytes_per_sec,
            "pacing_min_rate_bytes_per_sec",
        )?;
        validate_positive_f64(
            self.pacing_max_rate_bytes_per_sec,
            "pacing_max_rate_bytes_per_sec",
        )?;
        if self.pacing_min_rate_bytes_per_sec > self.pacing_max_rate_bytes_per_sec {
            return Err(ConfigValidationError::new(
                "SessionTunables",
                "pacing_min_rate_bytes_per_sec",
                "must be <= pacing_max_rate_bytes_per_sec",
            ));
        }
        if self.pacing_max_burst_bytes == 0 {
            return Err(ConfigValidationError::new(
                "SessionTunables",
                "pacing_max_burst_bytes",
                "must be >= 1",
            ));
        }
        if self.outgoing_queue_max_frames == 0 {
            return Err(ConfigValidationError::new(
                "SessionTunables",
                "outgoing_queue_max_frames",
                "must be >= 1",
            ));
        }
        if self.outgoing_queue_max_bytes == 0 {
            return Err(ConfigValidationError::new(
                "SessionTunables",
                "outgoing_queue_max_bytes",
                "must be >= 1",
            ));
        }
        if !self.outgoing_queue_soft_ratio.is_finite()
            || self.outgoing_queue_soft_ratio <= 0.0
            || self.outgoing_queue_soft_ratio >= 1.0
        {
            return Err(ConfigValidationError::new(
                "SessionTunables",
                "outgoing_queue_soft_ratio",
                "must be finite and within (0, 1)",
            ));
        }

        Ok(())
    }
}

fn validate_positive_f64(value: f64, field: &'static str) -> Result<(), ConfigValidationError> {
    if !value.is_finite() || value <= 0.0 {
        return Err(ConfigValidationError::new(
            "SessionTunables",
            field,
            "must be finite and > 0",
        ));
    }
    Ok(())
}

fn validate_fraction(value: f64, field: &'static str) -> Result<(), ConfigValidationError> {
    if !value.is_finite() || value <= 0.0 || value >= 1.0 {
        return Err(ConfigValidationError::new(
            "SessionTunables",
            field,
            "must be finite and within (0, 1)",
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::SessionTunables;

    #[test]
    fn validate_accepts_default_values() {
        SessionTunables::default()
            .validate()
            .expect("default tunables must be valid");
    }

    #[test]
    fn validate_rejects_zero_ack_queue_capacity() {
        let tunables = SessionTunables {
            ack_queue_capacity: 0,
            ..SessionTunables::default()
        };
        let err = tunables
            .validate()
            .expect_err("ack_queue_capacity=0 must be rejected");
        assert_eq!(err.config, "SessionTunables");
        assert_eq!(err.field, "ack_queue_capacity");
    }
}
