//! Give up on work that may never finish.
//!
//! There is exactly one caller: the first `lstat` into a mounted filesystem.
//! That syscall is uninterruptible — no signal, no flag, no `close` on the
//! other side will bring it back once the server has gone — so the only way
//! to survive it is to stop waiting and leave it where it is.
//!
//! **The abandoned thread never comes back, and that is the design.** It is
//! parked in the kernel holding a stack and nothing else; there is no way to
//! reclaim it and pretending otherwise (a "cancel" flag it will never read)
//! would be theatre. The count is bounded by the number of mount points that
//! have stopped answering, which on a machine anyone is still using is small.

use std::sync::mpsc;
use std::time::Duration;

/// Run `work` on another thread and wait `limit` for it.
///
/// `None` means it did not answer in time. The work keeps running; the caller
/// has only stopped caring.
pub(crate) fn with_deadline<T, F>(limit: Duration, work: F) -> Option<T>
where
    T: Send + 'static,
    F: FnOnce() -> T + Send + 'static,
{
    // Bounded at one and never read after a timeout: the sender must not
    // block on a receiver that has gone away, or a thread that *does*
    // eventually come back would hang for a second time, on the send.
    let (tx, rx) = mpsc::sync_channel::<T>(1);
    if std::thread::Builder::new()
        .name("spacetrace-probe".to_string())
        .spawn(move || {
            let _ = tx.send(work());
        })
        .is_err()
    {
        // Out of threads. Reporting "did not answer" would blame the
        // filesystem for a local resource problem, so say nothing happened
        // and let the caller fall back to the ordinary path.
        return None;
    }
    rx.recv_timeout(limit).ok()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Instant;

    #[test]
    fn work_that_finishes_in_time_is_returned() {
        assert_eq!(with_deadline(Duration::from_secs(5), || 42), Some(42));
    }

    /// The whole point: the caller comes back even though the work does not.
    #[test]
    fn work_that_overruns_is_abandoned() {
        let started = Instant::now();
        let answer = with_deadline(Duration::from_millis(20), || {
            std::thread::sleep(Duration::from_secs(30));
            42
        });
        assert_eq!(answer, None);
        assert!(
            started.elapsed() < Duration::from_secs(5),
            "waited {:?}, which means it did not give up",
            started.elapsed()
        );
    }

    /// An abandoned thread finishing later must not panic or block on a
    /// channel nobody is listening to. Nothing asserts here beyond the
    /// process staying alive, which is the claim.
    #[test]
    fn a_late_answer_is_dropped_quietly() {
        assert_eq!(
            with_deadline(Duration::from_millis(10), || {
                std::thread::sleep(Duration::from_millis(50));
                7
            }),
            None
        );
        std::thread::sleep(Duration::from_millis(200));
    }
}
