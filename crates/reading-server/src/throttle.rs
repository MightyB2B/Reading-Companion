//! Slowing down guessing.
//!
//! argon2id already makes each attempt expensive — that is most of the
//! defence, and the reason a leaked database is not immediately a list of
//! passwords. What it does not stop is somebody working through the obvious
//! guesses against one address at whatever rate the server will answer.
//!
//! So: five attempts free, then a wait that doubles. The first failures cost
//! nothing, because the person who mistypes their own password is far more
//! common than an attacker and should not be punished for it. By the tenth
//! attempt the wait is minutes, and a list of a million passwords would take
//! longer than anyone will sit for.
//!
//! Counted per address *and* per source, separately, because the two attacks
//! are different: one account guessed from many places, and many accounts
//! guessed from one. Either counter alone leaves the other unbounded.
//!
//! Held in memory. A restart forgives everyone, which is the right trade for
//! a personal library: this is a speed bump for guessing, not an account
//! lockout, and an attacker who can restart the server has already won.

use std::collections::HashMap;
use std::net::IpAddr;
use std::sync::Mutex;
use std::time::{Duration, Instant};

/// Failures allowed against one account before any wait is imposed.
pub const FREE_ATTEMPTS: u32 = 5;

/// Failures allowed from one address before any wait is imposed.
///
/// Much higher than the per-account allowance, and it has to be. A household
/// behind one router, or two people sharing a machine, are one address to
/// this server — so a tight limit here would let one person mistyping their
/// own password lock out everybody else. The per-account counter is what
/// stops focused guessing; this one only catches somebody working through
/// many accounts from one place, which needs far more attempts to be worth
/// doing and so is still caught well before it succeeds.
pub const FREE_ATTEMPTS_PER_SOURCE: u32 = 30;

/// The wait after the first attempt beyond the allowance.
const BASE_DELAY: Duration = Duration::from_secs(2);

/// Doubling stops here. Beyond about ten minutes the difference between
/// "slow" and "slower" is academic, and an unbounded value would eventually
/// overflow the arithmetic below.
const MAX_DELAY: Duration = Duration::from_secs(600);

/// How long a quiet key is remembered. Forgetting sooner would let an
/// attacker reset the count by pausing; forgetting later serves nobody, since
/// a person who got it wrong an hour ago is not the one guessing.
const FORGET_AFTER: Duration = Duration::from_secs(3600);

#[derive(Debug, Clone, Copy)]
struct Record {
    failures: u32,
    /// When the next attempt is allowed. Stored rather than derived so the
    /// wait is fixed at the moment of failure and cannot be shortened by
    /// asking again.
    open_at: Instant,
    last_seen: Instant,
}

/// What the caller must do about an attempt.
#[derive(Debug, PartialEq, Eq)]
pub enum Verdict {
    Allow,
    /// Refuse, and tell them how long to wait.
    Wait(Duration),
}

#[derive(Debug, Default)]
pub struct Throttle {
    entries: Mutex<HashMap<String, Record>>,
}

impl Throttle {
    pub fn new() -> Self {
        Self::default()
    }

    /// May this attempt proceed?
    ///
    /// Checked before the password is verified, so a throttled request never
    /// reaches argon2 — otherwise the throttle would cost the server the
    /// very work it exists to avoid.
    pub fn check(&self, keys: &[String]) -> Verdict {
        let now = Instant::now();
        let mut entries = self.entries.lock().unwrap_or_else(|e| e.into_inner());

        // The longest wait among the keys wins: being under the limit for
        // your address does not help if the address you are guessing at is
        // itself under attack.
        let mut longest = Duration::ZERO;
        for key in keys {
            if let Some(record) = entries.get(key) {
                if record.open_at > now {
                    longest = longest.max(record.open_at - now);
                }
            }
        }

        if longest > Duration::ZERO {
            return Verdict::Wait(longest);
        }

        // Cheap enough to do on the way past, and it keeps the map from
        // growing without bound on a server nobody is attacking.
        if entries.len() > 512 {
            entries.retain(|_, r| now.duration_since(r.last_seen) < FORGET_AFTER);
        }

        Verdict::Allow
    }

    /// A failed attempt. Returns the wait now imposed, if any.
    pub fn record_failure(&self, keys: &[String]) -> Option<Duration> {
        let now = Instant::now();
        let mut entries = self.entries.lock().unwrap_or_else(|e| e.into_inner());
        let mut longest = None;

        for key in keys {
            let record = entries.entry(key.clone()).or_insert(Record {
                failures: 0,
                open_at: now,
                last_seen: now,
            });

            // A key that has been quiet long enough starts again, so an
            // honest mistake last week does not count against today.
            if now.duration_since(record.last_seen) >= FORGET_AFTER {
                record.failures = 0;
            }

            record.failures += 1;
            record.last_seen = now;

            let delay = delay_for_key(key, record.failures);
            record.open_at = now + delay;

            if delay > Duration::ZERO {
                longest = Some(longest.map_or(delay, |d: Duration| d.max(delay)));
            }
        }

        longest
    }

    /// A successful sign-in clears the slate for these keys.
    pub fn record_success(&self, keys: &[String]) {
        let mut entries = self.entries.lock().unwrap_or_else(|e| e.into_inner());
        for key in keys {
            entries.remove(key);
        }
    }
}

/// How many failures a key gets for free, by what it counts.
fn allowance(key: &str) -> u32 {
    if key.starts_with("from:") {
        FREE_ATTEMPTS_PER_SOURCE
    } else {
        FREE_ATTEMPTS
    }
}

fn delay_for_key(key: &str, failures: u32) -> Duration {
    delay_after(allowance(key), failures)
}

/// Nothing until the allowance is used up, then 2s doubling to a ten-minute
/// ceiling.
fn delay_after(free: u32, failures: u32) -> Duration {
    if failures <= free {
        return Duration::ZERO;
    }

    // Shifting rather than multiplying, and saturating, so a very long run of
    // failures cannot overflow into a small number.
    let steps = failures - free - 1;
    let factor = 1u64.checked_shl(steps.min(32)).unwrap_or(u64::MAX);
    let delay = BASE_DELAY
        .checked_mul(factor.min(u32::MAX as u64) as u32)
        .unwrap_or(MAX_DELAY);

    delay.min(MAX_DELAY)
}

/// The keys an attempt counts against: the account being tried, and where it
/// is being tried from.
///
/// Loopback is deliberately not counted as a source. A request from the
/// machine itself is either the reader running server and application
/// together, or somebody who already has local access and does not need to
/// guess passwords — so counting it protects nothing while making the
/// ordinary single-machine setup throttle itself. The per-account limit still
/// applies, which is the one that stops guessing.
pub fn keys_for(email: &str, from: Option<IpAddr>) -> Vec<String> {
    let mut keys = vec![format!("user:{}", email.trim().to_lowercase())];
    if let Some(ip) = from.filter(|ip| !ip.is_loopback()) {
        keys.push(format!("from:{ip}"));
    }
    keys
}

#[cfg(test)]
mod tests {
    use super::*;

    fn keys() -> Vec<String> {
        vec!["user:reader@example.test".to_string()]
    }

    #[test]
    fn the_first_five_failures_cost_nothing() {
        let t = Throttle::new();
        for i in 1..=FREE_ATTEMPTS {
            assert_eq!(t.check(&keys()), Verdict::Allow, "attempt {i} was throttled");
            assert_eq!(t.record_failure(&keys()), None, "attempt {i} imposed a wait");
        }
    }

    #[test]
    fn the_sixth_imposes_a_wait_and_it_doubles() {
        let t = Throttle::new();
        for _ in 0..FREE_ATTEMPTS {
            t.record_failure(&keys());
        }

        let first = t.record_failure(&keys()).expect("no wait after the allowance");
        assert_eq!(first, BASE_DELAY);

        let second = t.record_failure(&keys()).expect("no wait");
        assert_eq!(second, BASE_DELAY * 2);

        let third = t.record_failure(&keys()).expect("no wait");
        assert_eq!(third, BASE_DELAY * 4);
    }

    #[test]
    fn a_throttled_key_is_refused() {
        let t = Throttle::new();
        for _ in 0..=FREE_ATTEMPTS {
            t.record_failure(&keys());
        }
        assert!(matches!(t.check(&keys()), Verdict::Wait(_)));
    }

    /// The ceiling has to hold, or a long run of failures overflows the
    /// arithmetic and the wait wraps round to nothing.
    #[test]
    fn the_wait_stops_growing() {
        assert_eq!(delay_after(FREE_ATTEMPTS, 1000), MAX_DELAY);
        assert_eq!(delay_after(FREE_ATTEMPTS, u32::MAX), MAX_DELAY);
        assert!(delay_after(FREE_ATTEMPTS, FREE_ATTEMPTS + 20) <= MAX_DELAY);
    }

    /// Two people behind one router are one address to this server. If the
    /// source were as tight as the account, one of them mistyping their
    /// password would lock out the other — which is a far more likely event
    /// than the attack it would be defending against.
    #[test]
    fn one_persons_mistakes_do_not_lock_out_the_household() {
        let t = Throttle::new();
        let shared = "from:203.0.113.7".to_string();

        // Someone fumbles their own password well past the account limit.
        for _ in 0..(FREE_ATTEMPTS + 3) {
            t.record_failure(&vec!["user:forgetful@example.test".to_string(), shared.clone()]);
        }

        // They are throttled...
        assert!(matches!(
            t.check(&["user:forgetful@example.test".to_string()]),
            Verdict::Wait(_)
        ));
        // ...but the person sitting next to them is not.
        assert_eq!(
            t.check(&["user:someone-else@example.test".to_string(), shared.clone()]),
            Verdict::Allow
        );
    }

    /// The looser source limit still has to bite eventually, or spraying many
    /// accounts from one place costs nothing.
    #[test]
    fn a_source_working_through_many_accounts_is_still_caught() {
        let t = Throttle::new();
        let source = "from:203.0.113.9".to_string();

        for i in 0..=FREE_ATTEMPTS_PER_SOURCE {
            t.record_failure(&vec![format!("user:target{i}@example.test"), source.clone()]);
        }

        assert!(matches!(t.check(&[source]), Verdict::Wait(_)));
    }

    #[test]
    fn signing_in_clears_it() {
        let t = Throttle::new();
        for _ in 0..=FREE_ATTEMPTS {
            t.record_failure(&keys());
        }
        assert!(matches!(t.check(&keys()), Verdict::Wait(_)));

        t.record_success(&keys());
        assert_eq!(t.check(&keys()), Verdict::Allow);
    }

    /// One account guessed from many places, and many accounts guessed from
    /// one place, are different attacks. Counting only one leaves the other
    /// free, so the two are tallied apart — with their own allowances, since
    /// what is suspicious for an account is ordinary for an address.
    #[test]
    fn the_account_and_the_source_are_counted_apart() {
        let t = Throttle::new();
        let source = "from:203.0.113.7".to_string();

        // Enough failures to throttle one account, spread over several.
        for i in 0..=FREE_ATTEMPTS {
            t.record_failure(&vec![
                format!("user:reader{i}@example.test"),
                source.clone(),
            ]);
        }

        // Each account took one hit, so none is near its own limit...
        assert_eq!(
            t.check(&["user:reader0@example.test".to_string()]),
            Verdict::Allow
        );
        // ...and the source is well inside its looser one.
        assert_eq!(t.check(&[source.clone()]), Verdict::Allow);

        // But hammering a single account still stops quickly.
        for _ in 0..FREE_ATTEMPTS {
            t.record_failure(&vec!["user:reader0@example.test".to_string(), source.clone()]);
        }
        assert!(matches!(
            t.check(&["user:reader0@example.test".to_string()]),
            Verdict::Wait(_)
        ));
    }

    /// The single-machine setup — server and application on one box — must
    /// not throttle itself. Everything it sends arrives from loopback, so
    /// counting that as a source would make one person's retries look like an
    /// attack from a very busy address.
    #[test]
    fn loopback_is_not_counted_as_a_source() {
        let local = keys_for("reader@example.test", Some("127.0.0.1".parse().unwrap()));
        assert_eq!(local.len(), 1, "loopback should contribute no source key");
        assert!(local[0].starts_with("user:"));

        let remote = keys_for("reader@example.test", Some("203.0.113.4".parse().unwrap()));
        assert_eq!(remote.len(), 2, "a real address should be counted");
        assert!(remote[1].starts_with("from:"));
    }

    #[test]
    #[test]
    fn the_email_is_matched_regardless_of_case_or_spacing() {
        let a = keys_for("Reader@Example.test", None);
        let b = keys_for("  reader@example.TEST  ", None);
        assert_eq!(a, b);
    }
}
