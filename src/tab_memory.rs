use std::time::{Duration, Instant};
use windows::Win32::System::SystemInformation::{GlobalMemoryStatusEx, MEMORYSTATUSEX};

const GIB: u64 = 1024 * 1024 * 1024;
const RETRY_DELAY: Duration = Duration::from_secs(2);
const REQUEST_TIMEOUT: Duration = Duration::from_secs(5);
const CHECK_INTERVAL: Duration = Duration::from_secs(2);

#[derive(Clone, Copy)]
pub struct MemorySample {
    pub available: u64,
    pub load_percent: u32,
}

pub fn memory_sample() -> Option<MemorySample> {
    let mut status = MEMORYSTATUSEX {
        dwLength: std::mem::size_of::<MEMORYSTATUSEX>() as u32,
        ..Default::default()
    };
    unsafe { GlobalMemoryStatusEx(&mut status).ok()? };
    (status.dwMemoryLoad <= 100).then_some(MemorySample {
        available: status.ullAvailPhys,
        load_percent: status.dwMemoryLoad,
    })
}

pub fn keeps_running(downloads: usize, playing_audio: bool) -> bool {
    downloads > 0 || playing_audio
}

#[derive(Default)]
pub struct MemoryPolicy {
    pressured: bool,
}

impl MemoryPolicy {
    pub fn observe(&mut self, sample: Option<MemorySample>) {
        self.pressured = match sample {
            None => true,
            Some(sample) if self.pressured => {
                !(sample.available > 2 * GIB && sample.load_percent <= 90)
            }
            Some(sample) => sample.available < GIB || sample.load_percent >= 95,
        };
    }

    pub fn limit(&self, suspension_failed: bool) -> usize {
        if self.pressured || suspension_failed {
            2
        } else {
            5
        }
    }

    pub fn needs_eviction(&self, ordinary_views: usize, suspension_failed: bool) -> bool {
        ordinary_views > self.limit(suspension_failed)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SuspensionTag {
    pub view: u64,
    pub attempt: u64,
}

#[derive(Clone, Copy, Debug)]
pub enum SuspensionOutcome {
    Request(bool),
    NativeState(bool),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SuspensionAction {
    None,
    Resume,
    Suspend(SuspensionTag),
    Check(SuspensionTag),
}

#[derive(Clone, Copy)]
enum Phase {
    Active,
    Pending(Instant),
    Sleeping(Instant),
    Retry(Instant),
    Fallback,
}

pub struct Suspension {
    tag: SuspensionTag,
    failures: u8,
    phase: Phase,
}

impl Default for Suspension {
    fn default() -> Self {
        Self {
            tag: SuspensionTag {
                view: 0,
                attempt: 0,
            },
            failures: 0,
            phase: Phase::Active,
        }
    }
}

impl Suspension {
    pub fn new_view(&mut self) {
        self.tag.view = self.tag.view.wrapping_add(1);
        self.tag.attempt = 0;
        self.failures = 0;
        self.phase = Phase::Active;
    }

    fn failed(&mut self, now: Instant) {
        self.failures += 1;
        self.phase = if self.failures >= 2 {
            Phase::Fallback
        } else {
            Phase::Retry(now + RETRY_DELAY)
        };
    }

    pub fn action(&mut self, protected: bool, now: Instant) -> SuspensionAction {
        if protected {
            if matches!(self.phase, Phase::Active) {
                return SuspensionAction::None;
            }
            // Invalidate a queued completion before calling COM or showing a tab.
            self.tag.attempt = self.tag.attempt.wrapping_add(1);
            self.failures = 0;
            self.phase = Phase::Active;
            return SuspensionAction::Resume;
        }
        match self.phase {
            Phase::Active => {}
            Phase::Retry(when) if now >= when => {}
            Phase::Pending(since) if now.duration_since(since) >= REQUEST_TIMEOUT => {
                self.failed(now);
                return SuspensionAction::None;
            }
            Phase::Sleeping(when) if now >= when => {
                self.phase = Phase::Sleeping(now + CHECK_INTERVAL);
                return SuspensionAction::Check(self.tag);
            }
            _ => return SuspensionAction::None,
        }
        self.tag.attempt = self.tag.attempt.wrapping_add(1);
        self.phase = Phase::Pending(now);
        SuspensionAction::Suspend(self.tag)
    }

    pub fn complete(&mut self, tag: SuspensionTag, outcome: SuspensionOutcome, now: Instant) {
        if tag != self.tag {
            return;
        }
        match (self.phase, outcome) {
            (Phase::Pending(_), SuspensionOutcome::Request(true)) => {
                self.phase = Phase::Sleeping(now + CHECK_INTERVAL);
            }
            (Phase::Sleeping(_), SuspensionOutcome::NativeState(true)) => {}
            (Phase::Pending(_), SuspensionOutcome::Request(false))
            | (Phase::Sleeping(_), SuspensionOutcome::NativeState(false)) => self.failed(now),
            _ => {}
        }
    }

    pub fn needs_fallback(&self) -> bool {
        matches!(self.phase, Phase::Fallback)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample(available: u64, load_percent: u32) -> Option<MemorySample> {
        Some(MemorySample {
            available,
            load_percent,
        })
    }

    fn request(state: &mut Suspension, now: Instant) -> SuspensionTag {
        match state.action(false, now) {
            SuspensionAction::Suspend(tag) => tag,
            action => panic!("expected suspend, got {action:?}"),
        }
    }

    #[test]
    fn memory_thresholds_have_hysteresis_and_conservative_query_failure() {
        let mut policy = MemoryPolicy::default();
        policy.observe(sample(GIB, 94));
        assert_eq!(policy.limit(false), 5);
        policy.observe(sample(GIB - 1, 40));
        assert_eq!(policy.limit(false), 2);
        policy.observe(sample(2 * GIB, 90));
        assert_eq!(policy.limit(false), 2);
        policy.observe(sample(2 * GIB + 1, 91));
        assert_eq!(policy.limit(false), 2);
        policy.observe(sample(2 * GIB + 1, 90));
        assert_eq!(policy.limit(false), 5);
        policy.observe(sample(4 * GIB, 95));
        assert_eq!(policy.limit(false), 2);
        policy.observe(None);
        policy.observe(sample(2 * GIB, 80));
        assert_eq!(policy.limit(false), 2);
        policy.observe(sample(3 * GIB, 80));
        assert_eq!(policy.limit(false), 5);
    }

    #[test]
    fn suspension_failure_uses_old_budget_without_destroying_two_retained_views() {
        let policy = MemoryPolicy::default();
        assert!(!policy.needs_eviction(5, false));
        assert!(policy.needs_eviction(6, false));
        assert!(policy.needs_eviction(5, true));
        assert!(policy.needs_eviction(3, true));
        assert!(!policy.needs_eviction(2, true));
        assert!(!policy.needs_eviction(1, true));
    }

    #[test]
    fn suspension_requests_are_bounded_and_success_is_confirmed() {
        let now = Instant::now();
        let mut state = Suspension::default();
        state.new_view();
        let tag = request(&mut state, now);
        assert_eq!(state.action(false, now), SuspensionAction::None);
        state.complete(tag, SuspensionOutcome::Request(true), now);
        assert_eq!(
            state.action(false, now + CHECK_INTERVAL),
            SuspensionAction::Check(tag)
        );
        state.complete(
            tag,
            SuspensionOutcome::NativeState(true),
            now + CHECK_INTERVAL,
        );
        assert!(!state.needs_fallback());
        assert_eq!(
            state.action(true, now + CHECK_INTERVAL),
            SuspensionAction::Resume
        );
    }

    #[test]
    fn failed_or_timed_out_requests_retry_once_then_use_bounded_fallback() {
        let now = Instant::now();
        let mut state = Suspension::default();
        let first = request(&mut state, now);
        state.complete(first, SuspensionOutcome::Request(false), now);
        assert_eq!(
            state.action(false, now + RETRY_DELAY - Duration::from_millis(1)),
            SuspensionAction::None
        );
        let second = request(&mut state, now + RETRY_DELAY);
        assert_ne!(first, second);
        state.action(false, now + RETRY_DELAY + REQUEST_TIMEOUT);
        assert!(state.needs_fallback());
        assert_eq!(
            state.action(false, now + Duration::from_secs(60)),
            SuspensionAction::None
        );
    }

    #[test]
    fn native_refusal_after_success_does_not_leave_hidden_scripts_unbounded() {
        let now = Instant::now();
        let mut state = Suspension::default();
        let first = request(&mut state, now);
        state.complete(first, SuspensionOutcome::Request(true), now);
        state.complete(
            first,
            SuspensionOutcome::NativeState(false),
            now + CHECK_INTERVAL,
        );
        let second = request(&mut state, now + CHECK_INTERVAL + RETRY_DELAY);
        state.complete(
            second,
            SuspensionOutcome::Request(true),
            now + CHECK_INTERVAL + RETRY_DELAY,
        );
        state.complete(
            second,
            SuspensionOutcome::NativeState(false),
            now + CHECK_INTERVAL + RETRY_DELAY,
        );
        assert!(state.needs_fallback());
    }

    #[test]
    fn reactivation_or_download_protection_invalidates_inflight_completion() {
        let now = Instant::now();
        let mut state = Suspension::default();
        let old = request(&mut state, now);
        assert_eq!(state.action(true, now), SuspensionAction::Resume);
        state.complete(old, SuspensionOutcome::Request(false), now);
        assert!(!state.needs_fallback());
        let current = request(&mut state, now);
        state.complete(old, SuspensionOutcome::Request(true), now);
        assert_eq!(state.action(false, now), SuspensionAction::None);
        state.complete(current, SuspensionOutcome::Request(true), now);
        assert_eq!(
            state.action(false, now + CHECK_INTERVAL),
            SuspensionAction::Check(current)
        );
    }

    #[test]
    fn confirmed_audio_and_downloads_are_protected_but_unknown_audio_is_not() {
        assert!(keeps_running(0, true));
        assert!(keeps_running(1, false));
        assert!(!keeps_running(0, false));
        let now = Instant::now();
        let mut state = Suspension::default();
        let tag = request(&mut state, now);
        assert_eq!(
            state.action(keeps_running(0, true), now),
            SuspensionAction::Resume
        );
        state.complete(tag, SuspensionOutcome::Request(false), now);
        assert!(!state.needs_fallback());
        assert_eq!(
            state.action(keeps_running(0, true), now),
            SuspensionAction::None
        );
        assert!(matches!(
            state.action(keeps_running(0, false), now),
            SuspensionAction::Suspend(_)
        ));
    }

    #[test]
    fn recreated_views_reject_old_callbacks_and_duplicate_results() {
        let now = Instant::now();
        let mut state = Suspension::default();
        state.new_view();
        let old = request(&mut state, now);
        state.new_view();
        let current = request(&mut state, now);
        state.complete(old, SuspensionOutcome::Request(false), now);
        state.complete(current, SuspensionOutcome::Request(false), now);
        state.complete(current, SuspensionOutcome::Request(false), now);
        assert!(
            !state.needs_fallback(),
            "duplicate callback cannot consume both attempts"
        );
        let retry = request(&mut state, now + RETRY_DELAY);
        state.complete(retry, SuspensionOutcome::Request(false), now + RETRY_DELAY);
        assert!(state.needs_fallback());
        assert_eq!(state.action(true, now), SuspensionAction::Resume);
        assert!(!state.needs_fallback());
    }
}
