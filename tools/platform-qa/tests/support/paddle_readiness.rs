//! Bound each independent native operation as well as the complete QA fixture.
use std::time::{Duration, Instant};

const STAGES: [&str; 8] = [
    "initializing",
    "system-language",
    "auto-image",
    "mixed-extraction",
    "encrypted-user-extraction",
    "encrypted-owner-extraction",
    "rejected-password",
    "ready",
];

pub struct ReadinessBudget {
    started: Instant,
    phase_started: Instant,
    stage: usize,
    operation_limit: Duration,
}

impl ReadinessBudget {
    pub fn new(started: Instant, operation_limit: Duration) -> Self {
        Self {
            started,
            phase_started: started,
            stage: 0,
            operation_limit,
        }
    }

    pub fn within_deadline(&mut self, now: Instant, phase: Option<&str>) -> bool {
        // Four independent OCR/extraction calls, plus one allowance for runtime
        // setup and the language/password checks. Progress cannot extend this
        // absolute ceiling or revive an operation that has already timed out.
        if now.duration_since(self.started) >= self.operation_limit * 5
            || now.duration_since(self.phase_started) >= self.operation_limit
        {
            return false;
        }
        if let Some(stage) = phase.and_then(|phase| STAGES.iter().position(|s| *s == phase)) {
            if stage > self.stage {
                self.stage = stage;
                self.phase_started = now;
            }
        }
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recorded_mac_progress_gets_independent_operation_budgets() {
        let start = Instant::now();
        let mut budget = ReadinessBudget::new(start, Duration::from_secs(180));
        for (millis, phase) in [
            (918, "system-language"),
            (1117, "auto-image"),
            (113556, "mixed-extraction"),
            (176340, "encrypted-user-extraction"),
        ] {
            assert!(budget.within_deadline(start + Duration::from_millis(millis), Some(phase)));
        }
        // The old single 180-second deadline killed the valid next operation.
        assert!(budget.within_deadline(start + Duration::from_secs(180), None));
        // A stalled encrypted operation still gets exactly 180 seconds.
        assert!(!budget.within_deadline(start + Duration::from_millis(356340), Some("ready")));
    }

    #[test]
    fn stale_unknown_or_missing_phases_cannot_keep_a_stalled_operation_alive() {
        for phase in [None, Some("initializing"), Some("unknown")] {
            let start = Instant::now();
            let mut budget = ReadinessBudget::new(start, Duration::from_secs(180));
            assert!(budget.within_deadline(start + Duration::from_secs(100), phase));
            assert!(!budget.within_deadline(start + Duration::from_secs(180), phase));
        }
        let start = Instant::now();
        let mut budget = ReadinessBudget::new(start, Duration::from_secs(180));
        assert!(budget.within_deadline(start + Duration::from_secs(1), Some("auto-image")));
        assert!(budget.within_deadline(start + Duration::from_secs(100), Some("system-language")));
        assert!(!budget.within_deadline(start + Duration::from_secs(181), Some("mixed-extraction")));
    }

    #[test]
    fn even_forward_progress_cannot_exceed_the_absolute_ceiling() {
        let start = Instant::now();
        let mut budget = ReadinessBudget::new(start, Duration::from_secs(180));
        for (index, phase) in STAGES.iter().skip(1).take(5).enumerate() {
            assert!(budget.within_deadline(
                start + Duration::from_secs(179 * (index as u64 + 1)),
                Some(phase),
            ));
        }
        assert!(!budget.within_deadline(start + Duration::from_secs(900), Some("ready")));
    }
}
