use std::time::{Duration, Instant};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CaptureHealth {
    Running,
    NoSignal(String),
    DeviceDisconnected(String),
    Reconnecting { attempt: u32, at: Instant },
}

#[derive(Debug, Clone)]
pub struct RecoveryController {
    pub state: CaptureHealth,
    attempts: u32,
}

impl Default for RecoveryController {
    fn default() -> Self {
        Self {
            state: CaptureHealth::Running,
            attempts: 0,
        }
    }
}

impl RecoveryController {
    pub fn signal_lost(&mut self, message: impl Into<String>) {
        self.state = CaptureHealth::NoSignal(message.into());
    }
    pub fn disconnected(&mut self, message: impl Into<String>) {
        self.state = CaptureHealth::DeviceDisconnected(message.into());
    }
    pub fn schedule_retry(&mut self, now: Instant) -> Instant {
        self.attempts = self.attempts.saturating_add(1);
        let seconds = 1u64 << self.attempts.min(4);
        let at = now + Duration::from_secs(seconds);
        self.state = CaptureHealth::Reconnecting {
            attempt: self.attempts,
            at,
        };
        at
    }
    pub fn recovered(&mut self) {
        self.attempts = 0;
        self.state = CaptureHealth::Running;
    }
}
