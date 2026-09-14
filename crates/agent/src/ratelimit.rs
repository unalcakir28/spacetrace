//! A cap on how fast one client may ask the agent for things.
//!
//! **Why this exists, given a token is already required.** The token was the
//! reason this was deferred, and it answers a different question. It decides
//! *who* may read a snapshot; it does not decide how often. Two things sit
//! outside it entirely: `/health` is deliberately unauthenticated, so anyone
//! who can reach the port can ask for it as fast as they like, and a wrong
//! token still costs a header parse, a comparison and a reply. So the limiter
//! runs **before** authentication, which is the only position from which it
//! can bound either of them.
//!
//! Above the free ones, the expensive request is `GET /scans/:id/download`: it
//! reads a snapshot off the disk the agent is supposed to be measuring. The
//! realistic failure is not an attacker but a client stuck in a retry loop —
//! the agent runs on a NAS, and a NAS has one disk.
//!
//! **The key is the peer address, and behind a reverse proxy that is the
//! proxy.** The project recommends running the agent behind one, and there the
//! per-client limit becomes a single limit shared by everybody. That is worth
//! saying plainly rather than hiding: it still stops one runaway client from
//! saturating the machine, and it no longer isolates clients from each other.
//! `X-Forwarded-For` is **not** read, because a header anyone can set is a way
//! to get a fresh allowance by typing one, and trusting it has to be a
//! deliberate configuration rather than a default.
//!
//! **Hand-written, like the cron parser**, for the same reason: the whole of
//! it is a division and a comparison, and the agent has to stay a single
//! static binary somebody will install on a NAS.

use std::collections::HashMap;
use std::net::IpAddr;
use std::sync::Mutex;
use std::time::{Duration, Instant};

/// How many client addresses to remember at once.
///
/// A hard bound, not a target. The map grows with distinct callers, and the
/// endpoint that needs no token is reachable by anyone who can reach the port
/// — so without a ceiling, a stream of requests from random addresses is a way
/// to spend the agent's memory. At roughly fifty bytes an entry this ceiling
/// is a couple of hundred kilobytes.
///
/// **When it is full and every client still owes, new addresses are refused.**
/// The alternative is to forget somebody who is mid-flood, which hands them a
/// full allowance again and makes the table a way around the limit. Refusing
/// is the stricter answer and the safe one: an address flood large enough to
/// fill this table *is* the thing being defended against, and on a machine
/// behind the recommended reverse proxy there is only ever one entry.
const MAX_TRACKED: usize = 4096;

/// One client's allowance, as a token bucket.
///
/// A bucket rather than requests-per-window, because a window boundary lets
/// twice the limit through across it — the last instant of one window and the
/// first of the next — and because a bucket is what makes "a burst is fine,
/// a sustained flood is not" expressible at all.
#[derive(Debug)]
struct Bucket {
    /// Allowance left, in requests. Fractional because it refills continuously.
    tokens: f64,
    last: Instant,
}

/// A limit on requests per client, refilling over time.
#[derive(Debug)]
pub struct RateLimit {
    /// Sustained rate, in requests per second.
    refill_per_second: f64,
    /// The most that can be saved up, so a client that has been quiet may
    /// still open several connections at once without being refused.
    burst: f64,
    clients: Mutex<HashMap<IpAddr, Bucket>>,
}

/// What to do with a request.
#[derive(Debug, PartialEq, Eq)]
pub enum Decision {
    Allow,
    /// Refuse, and tell the caller how long until one request is affordable.
    /// Rounded up and never zero: a `Retry-After: 0` invites the retry that is
    /// already the problem.
    Refuse {
        retry_after: Duration,
    },
}

impl RateLimit {
    /// `per_minute` requests sustained, with `burst` saved up at most.
    ///
    /// Returns `None` for a rate of zero, which is how the configuration says
    /// "no limit" — an agent on a trusted network behind something that
    /// already does this should not pay for it twice.
    pub fn new(per_minute: u32, burst: u32) -> Option<RateLimit> {
        if per_minute == 0 {
            return None;
        }
        Some(RateLimit {
            refill_per_second: f64::from(per_minute) / 60.0,
            // Never below one, or nothing is ever affordable.
            burst: f64::from(burst.max(1)),
            clients: Mutex::new(HashMap::new()),
        })
    }

    /// Charge one request to `client`.
    pub fn check(&self, client: IpAddr) -> Decision {
        self.check_at(client, Instant::now())
    }

    /// `check`, with the clock supplied. The tests own the clock; without that
    /// every assertion about refilling would be a sleep and a guess.
    fn check_at(&self, client: IpAddr, now: Instant) -> Decision {
        let mut clients = self
            .clients
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());

        if clients.len() >= MAX_TRACKED && !clients.contains_key(&client) {
            // Only ever drops clients that owe nothing: a bucket back at full
            // allowance is indistinguishable from one that was never created,
            // so forgetting it changes no decision. A client mid-flood is
            // below full and is therefore never forgotten — which is the
            // property that stops eviction from being a way around the limit.
            let cutoff = self.burst;
            let refill = self.refill_per_second;
            clients.retain(|_, bucket| {
                let filled = bucket.tokens
                    + now.saturating_duration_since(bucket.last).as_secs_f64() * refill;
                filled < cutoff
            });

            if clients.len() >= MAX_TRACKED {
                // Nobody has recovered yet, so there is no room to admit this
                // address without either growing without limit or forgetting
                // somebody who is still over their rate. Refusing is the only
                // answer that does neither. The wait advertised is the one it
                // takes a spent bucket to come back, which is when a slot can
                // next appear.
                let seconds = self.burst / self.refill_per_second;
                return Decision::Refuse {
                    retry_after: Duration::from_secs(seconds.ceil().max(1.0) as u64),
                };
            }
        }

        let burst = self.burst;
        let refill = self.refill_per_second;
        let bucket = clients.entry(client).or_insert(Bucket {
            tokens: burst,
            last: now,
        });

        // `saturating_duration_since`, because `Instant` is monotonic per
        // platform and not per process: a clock read on another core can come
        // back very slightly earlier, and subtracting would panic.
        let elapsed = now.saturating_duration_since(bucket.last).as_secs_f64();
        bucket.tokens = (bucket.tokens + elapsed * refill).min(burst);
        bucket.last = now;

        if bucket.tokens >= 1.0 {
            bucket.tokens -= 1.0;
            return Decision::Allow;
        }

        let short_by = 1.0 - bucket.tokens;
        let seconds = short_by / refill;
        Decision::Refuse {
            // Rounded up to the next whole second: `Retry-After` is expressed
            // in seconds, and rounding down would advertise a moment at which
            // the answer is still no.
            retry_after: Duration::from_secs(seconds.ceil().max(1.0) as u64),
        }
    }

    /// How many clients are being remembered. For the tests and `/status`.
    pub fn tracked(&self) -> usize {
        self.clients
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ip(last: u8) -> IpAddr {
        IpAddr::from([10, 0, 0, last])
    }

    #[test]
    fn a_burst_is_allowed_and_then_the_rate_bites() {
        let limit = RateLimit::new(60, 5).unwrap();
        let now = Instant::now();

        for i in 0..5 {
            assert_eq!(limit.check_at(ip(1), now), Decision::Allow, "burst {i}");
        }
        assert!(
            matches!(limit.check_at(ip(1), now), Decision::Refuse { .. }),
            "the sixth request in the same instant is over the burst"
        );
    }

    #[test]
    fn the_allowance_comes_back_as_time_passes() {
        let limit = RateLimit::new(60, 2).unwrap();
        let start = Instant::now();
        assert_eq!(limit.check_at(ip(1), start), Decision::Allow);
        assert_eq!(limit.check_at(ip(1), start), Decision::Allow);
        assert!(matches!(
            limit.check_at(ip(1), start),
            Decision::Refuse { .. }
        ));

        // 60 per minute is one per second.
        let later = start + Duration::from_secs(1);
        assert_eq!(
            limit.check_at(ip(1), later),
            Decision::Allow,
            "a second later one request is affordable again"
        );
    }

    /// Saving up forever would turn a quiet week into a flood.
    #[test]
    fn the_allowance_stops_at_the_burst() {
        let limit = RateLimit::new(60, 3).unwrap();
        let start = Instant::now();
        let much_later = start + Duration::from_secs(3600);

        for i in 0..3 {
            assert_eq!(limit.check_at(ip(1), much_later), Decision::Allow, "{i}");
        }
        assert!(
            matches!(limit.check_at(ip(1), much_later), Decision::Refuse { .. }),
            "an hour of quiet buys the burst and not an hour's worth"
        );
    }

    /// The point of keying on the client: one caller's flood must not refuse
    /// everybody else.
    #[test]
    fn clients_are_limited_separately() {
        let limit = RateLimit::new(60, 2).unwrap();
        let now = Instant::now();
        for _ in 0..2 {
            assert_eq!(limit.check_at(ip(1), now), Decision::Allow);
        }
        assert!(matches!(
            limit.check_at(ip(1), now),
            Decision::Refuse { .. }
        ));
        assert_eq!(
            limit.check_at(ip(2), now),
            Decision::Allow,
            "a different address has its own allowance"
        );
    }

    #[test]
    fn retry_after_is_when_the_answer_changes() {
        let limit = RateLimit::new(60, 1).unwrap();
        let start = Instant::now();
        assert_eq!(limit.check_at(ip(1), start), Decision::Allow);

        let Decision::Refuse { retry_after } = limit.check_at(ip(1), start) else {
            panic!("the second request in the same instant must be refused");
        };
        assert_eq!(retry_after, Duration::from_secs(1));
        assert_eq!(
            limit.check_at(ip(1), start + retry_after),
            Decision::Allow,
            "waiting exactly as long as advertised has to be enough"
        );
    }

    /// The map is bounded, and the bound must not become a way through the
    /// limit: a client that is being refused is below full and is kept.
    #[test]
    fn idle_clients_are_forgotten_and_flooding_ones_are_not() {
        let limit = RateLimit::new(60, 1).unwrap();
        let start = Instant::now();

        // One client spends its allowance and stays over the limit.
        assert_eq!(limit.check_at(ip(1), start), Decision::Allow);
        assert!(matches!(
            limit.check_at(ip(1), start),
            Decision::Refuse { .. }
        ));

        // Fill the table with addresses that each ask once and go quiet.
        for n in 0..MAX_TRACKED + 64 {
            let octets = (n as u32).to_be_bytes();
            limit.check_at(IpAddr::from([172, octets[1], octets[2], octets[3]]), start);
        }
        assert!(
            limit.tracked() <= MAX_TRACKED,
            "the table stays bounded, saw {}",
            limit.tracked()
        );
        assert!(
            matches!(limit.check_at(ip(1), start), Decision::Refuse { .. }),
            "the flooding client must not have been forgotten"
        );

        // Once time has passed the spent buckets are full again, and a new
        // address is admitted rather than turned away for good.
        let recovered = start + Duration::from_secs(60);
        assert_eq!(
            limit.check_at(ip(9), recovered),
            Decision::Allow,
            "the table has to let go of clients that went quiet"
        );
        assert!(
            limit.tracked() <= MAX_TRACKED,
            "and stay bounded afterwards, saw {}",
            limit.tracked()
        );
    }

    #[test]
    fn a_rate_of_zero_means_no_limiter_at_all() {
        assert!(RateLimit::new(0, 10).is_none());
    }

    /// A burst of zero would refuse the first request, which is not a limit
    /// but an outage.
    #[test]
    fn a_burst_of_zero_still_allows_one_request() {
        let limit = RateLimit::new(60, 0).unwrap();
        assert_eq!(limit.check_at(ip(1), Instant::now()), Decision::Allow);
    }
}
