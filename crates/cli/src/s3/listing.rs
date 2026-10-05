//! Listing a bucket: one ListObjectsV2 stream, or several at once.
//!
//! **Why several.** A page is at most 1000 keys and one round trip, so a
//! single stream lists one page per round trip whatever the bandwidth: ten
//! million keys are ten thousand round trips in a row. Folders are disjoint
//! ranges of the key space, and listed side by side they take a fraction of
//! that.
//!
//! **Why the tree cannot change.** The workers only fetch. Objects reach the
//! caller — the tree builder — on the calling thread, in exactly the order
//! one stream would deliver them, key order, so the tree is built by the same
//! code from the same sequence however many workers ran and however the work
//! was split. Invariant 2 kept by construction rather than by sorting after.
//!
//! **How the key space is split.**
//! 1. One plain page first. A bucket that fits in it is done in one request,
//!    as before. For a larger one that page is thrown away and everything is
//!    listed again from the start, so that no listing ever begins in the
//!    middle of a folder — the one place where S3 and MinIO read
//!    `start-after` differently. It costs one request.
//! 2. Discovery: the root is listed with a `/` delimiter, which returns its own
//!    objects and its folders. While there are fewer folders than
//!    `FOLDERS_PER_WORKER` per worker, every folder is listed the same way,
//!    level by level, `MAX_DISCOVERY_DEPTH` levels at most. A folder with more
//!    than `MAX_DISCOVERED_OBJECTS` objects of its own stops being expanded:
//!    what was read of it is kept, and the rest of it becomes one range that
//!    starts after the last key read, so memory stays bounded and nothing is
//!    read twice.
//! 3. Folders with one parent and nothing listed between them form a run,
//!    listed as one range: `prefix` the parent, `start-after` the first
//!    folder, stopping where the next step of discovery's picture begins. A
//!    thousand small folders are then a few pages, not a thousand requests.
//! 4. A worker with no range to take takes the second half of the folders the
//!    earliest busy range has not reached yet, and that range now stops where
//!    the half begins. What a range has handed over and where it may stop are
//!    decided under one lock, so a split leaves neither a gap nor an overlap.
//!
//! **Memory, and the front of the queue.** The caller takes keys in order,
//! so what is fetched far ahead of it waits in memory. A range stops once
//! everything queued from the range being taken up to and including itself
//! reaches `MAX_BUFFERED` objects, and its worker goes where it is needed:
//! the earliest range nobody is listing, or half of the earliest one being
//! listed. The stopped range resumes after its last key when there is room.
//! Workers that waited inside their ranges instead left one worker on the
//! range the caller was waiting for while the rest sat on full queues: 587k
//! keys on AWS at 32 workers took 111 s that way, 27 s this way, and 23 s
//! with no cap at all. The range being taken counts only its own queue, which
//! the caller drains, so it never waits on the others; a second cap at twice
//! `MAX_BUFFERED` bounds the total however the timing falls.
//!
//! A static cut — runs split into about four fixed ranges a worker, handed
//! out two ranges a worker ahead of the caller, a two-page queue each — was
//! tried against this on the same AWS buckets and lost: 587k keys took 167 s
//! against 38 s at 8 workers and 130 s against 19 s at 16, 78k keys 11.7 s
//! against 7.4 s at 8. A range that turns out large cannot be split, and the
//! ranges behind it can only run as far ahead as their queues.

use std::collections::{HashSet, VecDeque};
use std::ops::ControlFlow;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::{Condvar, Mutex};
use std::time::Duration;

use anyhow::{bail, Context, Result};
use spacetrace_scan_core::ScanProgress;

use super::client::{check_cancelled, Bucket, Client, Query};
use super::lock;
use super::xml::{ListPage, Object};

/// Concurrent requests on AWS when `--threads` does not say.
///
/// Measured on an Apple M3 Max against the public `sentinel-cogs` bucket in
/// us-west-2, about 200 ms away over a 5.4 MB/s link, medians of interleaved
/// runs (two rounds for 587k keys, three for 78k): 587k keys took 175 s in
/// one stream, 30.5 s with 8 workers, 29.1 s with 16, 37.6 s with 32 and
/// 31.3 s with 64; 78k keys took 22.2, 6.7, 7.0, 8.9 and 6.0 s. Past eight
/// the link is full — 6 MB/s of listing XML — so sixteen is not shown to beat
/// eight there; it costs nothing there either, and leaves room on a link
/// that is not the limit. Sixty-four only adds requests.
pub const DEFAULT_WORKERS: usize = 16;

/// The most `--threads` gets for a bucket. Each worker is a thread and a
/// connection, `--threads` takes up to 65535, and nothing past 64 was
/// measured — 64 itself was no faster than 8.
pub const MAX_WORKERS: usize = 64;

/// Consecutive pages with no keys and a token for more, before a stream is
/// refused. S3 may send an empty page while it skips past what a listing does
/// not show; a thousand in a row is not that, it is a server that will never
/// finish.
pub const MAX_EMPTY_PAGES: u64 = 1000;

/// Discovery goes one level deeper while there are fewer folders than this
/// per worker: enough that a worker finishing early finds something to take.
const FOLDERS_PER_WORKER: usize = 4;

/// Levels below the root that discovery lists with a delimiter. Each level is
/// a round of requests before any range starts.
const MAX_DISCOVERY_DEPTH: usize = 3;

/// A folder with more objects of its own than this is not expanded further:
/// its objects would otherwise wait in memory for the folders before them.
const MAX_DISCOVERED_OBJECTS: usize = 10_000;

/// All expanded folders' own objects together, held until their turn.
const DISCOVERY_BUDGET: usize = 100_000;

/// Objects fetched ahead of the caller, counted from the range it is taking
/// (see the module comment). At about a hundred bytes an object, 10 MB, and
/// at most twice that however the timing falls, against about 170 MB for the
/// tree of a million keys (measured).
pub const MAX_BUFFERED: usize = 100_000;

/// How a listing runs.
#[derive(Debug, Clone, Copy)]
pub struct Plan {
    /// `1` is the single stream of before.
    pub workers: usize,
    pub max_buffered: usize,
}

impl Plan {
    pub fn workers(workers: usize) -> Plan {
        Plan {
            workers,
            max_buffered: MAX_BUFFERED,
        }
    }
}

/// What `list` hands the caller, page by page, in key order.
pub type Sink<'a> = dyn FnMut(Vec<Object>) -> Result<()> + 'a;

/// List everything under `root` (`""` or ending in `/`) and give it to `sink`
/// in key order. Returns the number of requests made.
pub fn list(
    client: &mut Client,
    bucket: &Bucket,
    root: &str,
    plan: Plan,
    progress: &ScanProgress,
    sink: &mut Sink<'_>,
) -> Result<u64> {
    let requests = AtomicU64::new(0);
    if plan.workers <= 1 {
        stream(
            client,
            bucket,
            &Query::plain(root),
            progress,
            &requests,
            |page| {
                sink(page.objects)?;
                Ok(ControlFlow::Continue(()))
            },
        )?;
        return Ok(requests.into_inner());
    }

    // The first page alone, on this thread: a small bucket ends here, and
    // AWS's answer to a wrong region is settled before any worker copies the
    // client.
    let first = client.list_page(bucket, &Query::plain(root), None, progress)?;
    requests.fetch_add(1, Ordering::Relaxed);
    if !first.is_truncated {
        sink(first.objects)?;
        return Ok(requests.into_inner());
    }

    let job = Job {
        bucket,
        root,
        workers: plan.workers,
        max_buffered: plan.max_buffered,
        page: client.page_hint(),
        progress,
        requests: &requests,
    };
    let items = job.discover(client)?;
    job.fetch(client, plan_work(items), sink)?;
    Ok(requests.into_inner())
}

/// One ListObjectsV2 stream to its end, or until `each` breaks, with the
/// checks that keep a misbehaving server from looping it forever.
fn stream(
    client: &mut Client,
    bucket: &Bucket,
    query: &Query<'_>,
    progress: &ScanProgress,
    requests: &AtomicU64,
    mut each: impl FnMut(ListPage) -> Result<ControlFlow<()>>,
) -> Result<()> {
    let mut token: Option<String> = None;
    let mut seen_tokens = HashSet::new();
    let mut empty_in_a_row = 0u64;
    loop {
        let page = client.list_page(bucket, query, token.as_deref(), progress)?;
        requests.fetch_add(1, Ordering::Relaxed);
        let truncated = page.is_truncated;
        let empty = page.objects.is_empty() && page.prefixes.is_empty();
        let next = page.next_continuation_token.clone();
        if each(page)?.is_break() || !truncated {
            return Ok(());
        }
        empty_in_a_row = match empty {
            true => empty_in_a_row + 1,
            false => 0,
        };
        if empty_in_a_row > MAX_EMPTY_PAGES {
            bail!(
                "s3://{} sent more than {MAX_EMPTY_PAGES} empty pages in a row, each promising more; \
                 the listing would never end",
                bucket.name
            );
        }
        let next = next.with_context(|| {
            format!(
                "s3://{} said there is more and gave no continuation token",
                bucket.name
            )
        })?;
        // A server handing back a token it already gave would make this loop
        // forever while the counters kept moving — the one hang the stall
        // warning cannot see.
        if !seen_tokens.insert(next.clone()) {
            bail!(
                "s3://{} repeated a continuation token; the listing would never end",
                bucket.name
            );
        }
        token = Some(next);
    }
}

/// Discovery's picture of the key space, in key order.
enum Item {
    Objects(Vec<Object>),
    /// A folder not listed yet: ends in `/`.
    Folder(String),
    /// The rest of a folder discovery stopped expanding, listed whole.
    Rest(Range),
}

/// One range of the key space, listed by one worker at a time.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Range {
    /// What the requests ask for: the parent of a run, or the folder whose
    /// rest this is.
    prefix: String,
    /// Moves to the last key seen when the range pauses, so it resumes there.
    start_after: Option<String>,
    /// Keys before this belong to an earlier step: the parent's own objects
    /// that sort between `start_after` and the first folder.
    from: Option<String>,
    /// Keys from here on belong to a later step. Shrinks when split.
    until: Option<String>,
    /// The run's folders, sorted: where a split may fall. Empty for a rest.
    folders: Vec<String>,
}

impl Range {
    /// The first key this range can hand over, as a bound for the step
    /// before it.
    fn start(&self) -> &str {
        self.from.as_deref().unwrap_or(&self.prefix)
    }
}

/// What the caller walks, in key order.
#[derive(Debug, PartialEq, Eq)]
enum Work {
    Objects(Vec<Object>),
    Range(Range),
}

/// Discovery's items as work: runs of sibling folders become ranges, each
/// stopping where the next step begins. Nothing of the parent's own sorts
/// inside a run — the delimiter listing would have put it between two of
/// its folders and ended the run — so a range holds only its folders' keys.
fn plan_work(items: Vec<Item>) -> Vec<Work> {
    enum Step {
        Objects(Vec<Object>),
        Run {
            parent: String,
            folders: Vec<String>,
        },
        Rest(Range),
    }
    let mut steps: Vec<Step> = Vec::new();
    for item in items {
        let folder = match item {
            Item::Objects(objects) => {
                steps.push(Step::Objects(objects));
                continue;
            }
            Item::Rest(range) => {
                steps.push(Step::Rest(range));
                continue;
            }
            Item::Folder(folder) => folder,
        };
        let parent = parent_of(&folder);
        if let Some(Step::Run { parent: p, folders }) = steps.last_mut() {
            if p == parent {
                folders.push(folder);
                continue;
            }
        }
        steps.push(Step::Run {
            parent: parent.to_string(),
            folders: vec![folder],
        });
    }
    // Backwards, so each range learns where the step after it begins.
    let mut work = Vec::with_capacity(steps.len());
    let mut next_start: Option<String> = None;
    for step in steps.into_iter().rev() {
        let range = match step {
            Step::Objects(objects) => {
                next_start = Some(objects[0].key.clone());
                work.push(Work::Objects(objects));
                continue;
            }
            Step::Run { parent, folders } => Range {
                prefix: parent,
                start_after: Some(seek(&folders[0])),
                from: Some(folders[0].clone()),
                until: next_start.take(),
                folders,
            },
            Step::Rest(range) => Range {
                until: next_start.take(),
                ..range
            },
        };
        next_start = Some(range.start().to_string());
        work.push(Work::Range(range));
    }
    work.reverse();
    work
}

/// What every part of one parallel listing shares.
struct Job<'a> {
    bucket: &'a Bucket,
    root: &'a str,
    workers: usize,
    max_buffered: usize,
    /// Keys per page, for judging whether a split is worth a request.
    page: usize,
    progress: &'a ScanProgress,
    requests: &'a AtomicU64,
}

impl Job<'_> {
    /// Expand folders level by level until there are enough to share out.
    fn discover(&self, client: &Client) -> Result<Vec<Item>> {
        let mut items = vec![Item::Folder(self.root.to_string())];
        let budget = AtomicUsize::new(DISCOVERY_BUDGET);
        for depth in 0..MAX_DISCOVERY_DEPTH {
            let folders: Vec<String> = items
                .iter()
                .filter_map(|item| match item {
                    Item::Folder(f) => Some(f.clone()),
                    _ => None,
                })
                .collect();
            if folders.is_empty()
                || (depth > 0 && folders.len() >= self.workers * FOLDERS_PER_WORKER)
            {
                break;
            }
            let expansions = on_workers(client, self.workers, &folders, |client, folder| {
                self.expand(client, folder, &budget)
            })?;
            let mut expansions = expansions.into_iter();
            let mut next = Vec::with_capacity(items.len());
            for item in items {
                match item {
                    Item::Folder(_) => next.extend(expansions.next().expect("one per folder")),
                    other => next.push(other),
                }
            }
            items = next;
        }
        Ok(items)
    }

    /// One folder listed with a delimiter: its own objects and its
    /// subfolders, in key order. A folder with too many objects of its own is
    /// read only so far: what was read stays, up to its last object, and the
    /// rest becomes one range after that object.
    fn expand(&self, client: &mut Client, folder: &str, budget: &AtomicUsize) -> Result<Vec<Item>> {
        let query = Query {
            prefix: folder,
            delimiter: true,
            start_after: None,
        };
        let rest = |start_after: Option<String>| {
            Item::Rest(Range {
                prefix: folder.to_string(),
                start_after,
                from: None,
                until: None,
                folders: Vec::new(),
            })
        };
        let mut items: Vec<Item> = Vec::new();
        let mut taken = 0usize;
        let mut stopped = false;
        let mut ignored_delimiter = false;
        stream(
            client,
            self.bucket,
            &query,
            self.progress,
            self.requests,
            |page| {
                // Nothing reaches the tree before discovery ends, so this is
                // the counter that moves meanwhile (invariant 8). Not `dirs`:
                // the tree sets that from what it holds, which starts lower
                // than what discovery found, and it would run backwards.
                self.progress.rows_done.fetch_add(1, Ordering::Relaxed);
                // A server that ignored the delimiter sends keys from deeper
                // down; that folder is listed whole, the way it sends it.
                let nested = |key: &str| {
                    key.get(folder.len()..)
                        .is_none_or(|rest| rest.contains('/'))
                };
                if page.objects.iter().any(|o| nested(&o.key)) {
                    ignored_delimiter = true;
                    return Ok(ControlFlow::Break(()));
                }
                for prefix in &page.prefixes {
                    let below = prefix
                        .strip_prefix(folder)
                        .and_then(|rest| rest.strip_suffix('/'))
                        .is_some_and(|segment| !segment.contains('/'));
                    if !below {
                        bail!(
                            "s3://{} listed the folder {prefix:?}, which is not one level below {folder:?}",
                            self.bucket.name
                        );
                    }
                }
                let own = page.objects.len();
                let within_budget = budget
                    .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |left| {
                        left.checked_sub(own)
                    })
                    .is_ok();
                merge_page(&mut items, page);
                taken += own;
                if !within_budget || taken > MAX_DISCOVERED_OBJECTS {
                    stopped = true;
                    return Ok(ControlFlow::Break(()));
                }
                Ok(ControlFlow::Continue(()))
            },
        )?;
        if ignored_delimiter {
            return Ok(vec![rest(None)]);
        }
        if !stopped {
            return Ok(items);
        }
        // Folders read after the last object go back to the rest: the rest
        // starts after that object, and so lists them.
        let last_object = items.iter().rev().find_map(|item| match item {
            Item::Objects(run) => run.last().map(|o| o.key.clone()),
            _ => None,
        });
        let Some(after) = last_object else {
            return Ok(vec![rest(None)]);
        };
        while !matches!(items.last(), Some(Item::Objects(_))) {
            items.pop();
        }
        items.push(rest(Some(after)));
        Ok(items)
    }

    /// Turn the work into ranges, list them, and hand everything to `sink`
    /// in order.
    fn fetch(&self, client: &mut Client, work: Vec<Work>, sink: &mut Sink<'_>) -> Result<()> {
        let mut ranges = Vec::new();
        let steps: Vec<Option<Vec<Object>>> = work
            .into_iter()
            .map(|w| match w {
                Work::Objects(objects) => Some(objects),
                Work::Range(range) => {
                    ranges.push(range);
                    None
                }
            })
            .collect();
        // One range that cannot be split is one stream: nothing to run
        // beside it, and no worker to start.
        if ranges.len() <= 1 && ranges.iter().all(|r| r.folders.len() <= 1) {
            return self.fetch_inline(client, steps, ranges, sink);
        }
        let state = Mutex::new(State {
            order: (0..ranges.len()).collect(),
            ranges: ranges
                .into_iter()
                .map(|range| Live {
                    range,
                    emitted: None,
                    handed: 0,
                    pages: VecDeque::new(),
                    queued: 0,
                    planned: true,
                    busy: false,
                    done: false,
                })
                .collect(),
            taking: 0,
            buffered: 0,
            failure: None,
            stop: false,
        });
        let wake = Wake::default();
        std::thread::scope(|scope| {
            // All of them, not one per planned range: a worker with no range
            // of its own is the one that splits another's.
            for _ in 0..self.workers {
                let mut client = client.clone();
                let (state, wake) = (&state, &wake);
                scope.spawn(move || self.work(&mut client, state, wake));
            }
            let taken = self.take(steps, &state, &wake, sink);
            lock(&state).stop = true;
            wake.idle.notify_all();
            taken
        })
    }

    fn fetch_inline(
        &self,
        client: &mut Client,
        steps: Vec<Option<Vec<Object>>>,
        ranges: Vec<Range>,
        sink: &mut Sink<'_>,
    ) -> Result<()> {
        let mut ranges = ranges.into_iter();
        for step in steps {
            match step {
                Some(objects) => sink(objects)?,
                None => {
                    let range = ranges.next().expect("one range per marker");
                    let query = Query {
                        prefix: &range.prefix,
                        delimiter: false,
                        start_after: range.start_after.as_deref(),
                    };
                    stream(
                        client,
                        self.bucket,
                        &query,
                        self.progress,
                        self.requests,
                        |page| {
                            let (kept, reached_end) = within(&range, page.objects);
                            if !kept.is_empty() {
                                sink(kept)?;
                            }
                            Ok(match reached_end {
                                true => ControlFlow::Break(()),
                                false => ControlFlow::Continue(()),
                            })
                        },
                    )?;
                }
            }
        }
        Ok(())
    }

    /// A worker: whatever range is most needed and may go on, until every
    /// range is done.
    fn work(&self, client: &mut Client, state: &Mutex<State>, wake: &Wake) {
        loop {
            let (id, range) = {
                let mut st = lock(state);
                loop {
                    if st.stop || st.ranges.iter().all(|live| live.done) {
                        return;
                    }
                    if let Some(id) = self.claim(&mut st) {
                        break (id, st.ranges[id].range.clone());
                    }
                    // Nothing may go on until the caller takes something, or
                    // a range finishes; the timeout is for a cancel, which
                    // nobody announces.
                    st = wake
                        .idle
                        .wait_timeout(st, Duration::from_millis(100))
                        .unwrap_or_else(std::sync::PoisonError::into_inner)
                        .0;
                    if self.progress.is_cancelled() {
                        return;
                    }
                }
            };
            let left = self.list_range(client, &range, id, state, wake);
            let mut st = lock(state);
            st.ranges[id].busy = false;
            match left {
                Ok(Left::Done) => st.ranges[id].done = true,
                Ok(Left::Paused) => {}
                Err(e) => {
                    st.failure.get_or_insert(e);
                    st.stop = true;
                    wake.idle.notify_all();
                }
            }
            wake.taker.notify_one();
        }
    }

    /// The earliest range nobody is listing, if it may go on; else the
    /// second half of the earliest range being listed that has folders it
    /// has not reached. Earliest, because the caller takes keys in order:
    /// work near the front is what it waits for.
    fn claim(&self, st: &mut State) -> Option<usize> {
        // A split costs a request and the tail of the page the other range
        // has in flight, so the half taken has to be worth two pages — by
        // what that range has met so far, by what all of them have, or
        // blindly before anything has arrived at all.
        let worth = 2 * self.page;
        let everywhere = st
            .ranges
            .iter()
            .filter_map(rate_of)
            .reduce(|(h1, f1), (h2, f2)| (h1 + h2, f1 + f2));
        let mut victim = None;
        let mut queued = 0;
        for (offset, &id) in st.order[st.taking..].iter().enumerate() {
            let live = &st.ranges[id];
            queued += live.queued;
            if live.done || !st.room(offset == 0, live.queued, queued, self.max_buffered) {
                continue;
            }
            if !live.busy {
                st.ranges[id].busy = true;
                return Some(id);
            }
            if victim.is_none() {
                victim = split_of(live, everywhere)
                    .filter(|split| split.keys.is_none_or(|keys| keys >= worth))
                    .map(|split| (st.taking + offset, split));
            }
        }
        let (position, Split { at, end, .. }) = victim?;
        let old = &st.ranges[st.order[position]];
        let folder = old.range.folders[at].clone();
        let half = Live {
            range: Range {
                prefix: old.range.prefix.clone(),
                start_after: Some(seek(&folder)),
                from: Some(folder.clone()),
                until: old.range.until.clone(),
                folders: old.range.folders[at..end].to_vec(),
            },
            emitted: None,
            handed: 0,
            pages: VecDeque::new(),
            queued: 0,
            planned: false,
            busy: true,
            done: false,
        };
        let id = st.ranges.len();
        let victim = st.order[position];
        st.ranges[victim].range.until = Some(folder);
        st.ranges.push(half);
        st.order.insert(position + 1, id);
        Some(id)
    }

    /// List `range` until its end, or until it has no room to go on.
    fn list_range(
        &self,
        client: &mut Client,
        range: &Range,
        id: usize,
        state: &Mutex<State>,
        wake: &Wake,
    ) -> Result<Left> {
        let query = Query {
            prefix: &range.prefix,
            delimiter: false,
            start_after: range.start_after.as_deref(),
        };
        let mut left = Left::Done;
        stream(
            client,
            self.bucket,
            &query,
            self.progress,
            self.requests,
            |page| {
                let mut st = lock(state);
                if st.stop {
                    return Ok(ControlFlow::Break(()));
                }
                let seen = page.objects.last().map(|o| o.key.clone());
                // Read under the lock it is changed under, together with the
                // hand-over below: a split cannot fall between the two.
                let (kept, reached_end) = within(&st.ranges[id].range, page.objects);
                if let Some(last) = kept.last() {
                    let n = kept.len();
                    let live = &mut st.ranges[id];
                    live.emitted = Some(last.key.clone());
                    live.handed += n;
                    live.queued += n;
                    live.pages.push_back(kept);
                    st.buffered += n;
                    if st.order[st.taking] == id {
                        wake.taker.notify_one();
                    }
                }
                if reached_end {
                    return Ok(ControlFlow::Break(()));
                }
                // Where a pause resumes: after the last key this range has
                // seen, kept or not.
                if let Some(key) = seen {
                    st.ranges[id].range.start_after = Some(key);
                }
                if !st.has_room(id, self.max_buffered) {
                    left = Left::Paused;
                    return Ok(ControlFlow::Break(()));
                }
                Ok(ControlFlow::Continue(()))
            },
        )?;
        Ok(left)
    }

    /// The caller's side: every step in order, each range followed by the
    /// halves split off it, which sit right after it in `order`.
    fn take(
        &self,
        steps: Vec<Option<Vec<Object>>>,
        state: &Mutex<State>,
        wake: &Wake,
        sink: &mut Sink<'_>,
    ) -> Result<()> {
        let mut position = 0;
        for step in steps {
            if let Some(objects) = step {
                sink(objects)?;
                continue;
            }
            loop {
                let id = {
                    let mut st = lock(state);
                    st.taking = position;
                    st.order[position]
                };
                // Where the caller is decides who has room.
                wake.idle.notify_all();
                while let Some(page) = self.next_page(state, wake, id)? {
                    sink(page)?;
                }
                position += 1;
                let st = lock(state);
                let next = st.order.get(position).map(|&id| st.ranges[id].planned);
                if next.is_none_or(|planned| planned) {
                    break;
                }
            }
        }
        Ok(())
    }

    /// The next page of range `id`, or `None` once it is done and drained.
    fn next_page(
        &self,
        state: &Mutex<State>,
        wake: &Wake,
        id: usize,
    ) -> Result<Option<Vec<Object>>> {
        let mut st = lock(state);
        loop {
            if let Some(e) = st.failure.take() {
                return Err(e);
            }
            check_cancelled(self.progress)?;
            if let Some(page) = st.ranges[id].pages.pop_front() {
                st.ranges[id].queued -= page.len();
                st.buffered -= page.len();
                // Room for one more page somewhere.
                wake.idle.notify_one();
                return Ok(Some(page));
            }
            if st.ranges[id].done {
                return Ok(None);
            }
            st = wake
                .taker
                .wait_timeout(st, Duration::from_millis(100))
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .0;
        }
    }
}

/// The objects of a page that belong to `range`, and whether the page went
/// past its end.
fn within(range: &Range, objects: Vec<Object>) -> (Vec<Object>, bool) {
    let mut kept = Vec::with_capacity(objects.len());
    for object in objects {
        if range
            .until
            .as_deref()
            .is_some_and(|u| object.key.as_str() >= u)
        {
            return (kept, true);
        }
        if range
            .from
            .as_deref()
            .is_none_or(|f| object.key.as_str() >= f)
        {
            kept.push(object);
        }
    }
    (kept, false)
}

// ------------------------------------------------------------ the ranges

struct Live {
    range: Range,
    /// The last key handed over, and how many keys so far.
    emitted: Option<String>,
    handed: usize,
    pages: VecDeque<Vec<Object>>,
    /// Objects in `pages`.
    queued: usize,
    /// One of discovery's ranges rather than a half split off one: where the
    /// caller's next step begins.
    planned: bool,
    /// A worker is listing it now.
    busy: bool,
    done: bool,
}

struct State {
    ranges: Vec<Live>,
    /// Range ids in the order the caller takes them. A half split off a range
    /// goes right after it.
    order: Vec<usize>,
    /// Where in `order` the caller is taking from. Ranges before it are done
    /// and drained, so a split never lands before it and it never moves
    /// under the caller.
    taking: usize,
    /// Objects queued in every range.
    buffered: usize,
    failure: Option<anyhow::Error>,
    stop: bool,
}

impl State {
    /// Whether a range may fetch another page: the one being taken while its
    /// own queue is under `max`, any other while everything queued from the
    /// one being taken up to and including it is, and the total under twice
    /// that.
    fn room(&self, taking: bool, own: usize, up_to: usize, max: usize) -> bool {
        match taking {
            true => own < max,
            false => up_to < max && self.buffered < max.saturating_mul(2),
        }
    }

    fn has_room(&self, id: usize, max: usize) -> bool {
        let mut up_to = 0;
        for (offset, &other) in self.order[self.taking..].iter().enumerate() {
            up_to += self.ranges[other].queued;
            if other == id {
                return self.room(offset == 0, self.ranges[id].queued, up_to, max);
            }
        }
        false
    }
}

/// The two kinds of waiting: workers for something to take, and the caller
/// for its next page.
#[derive(Default)]
struct Wake {
    idle: Condvar,
    taker: Condvar,
}

/// How a worker left a range.
enum Left {
    Done,
    /// Out of room; it resumes after its last key.
    Paused,
}

/// Where `live` could be split: the second half of the folders it has not
/// reached, and how many keys that half is likely to hold.
struct Split {
    at: usize,
    end: usize,
    /// `None` while nothing anywhere has been handed over to judge by.
    keys: Option<usize>,
}

/// The folder `live` is in: the last one at or before the key it handed
/// over last, since every key a range hands over lies in one of its folders.
fn current_folder(live: &Live) -> usize {
    match &live.emitted {
        Some(key) => {
            live.range
                .folders
                .partition_point(|f| f.as_str() <= key.as_str())
                .max(1)
                - 1
        }
        None => 0,
    }
}

/// Keys handed over and folders reached, for a range that has handed over
/// something.
fn rate_of(live: &Live) -> Option<(usize, usize)> {
    live.emitted.as_ref()?;
    (!live.range.folders.is_empty()).then(|| (live.handed, current_folder(live) + 1))
}

/// `fallback` — keys per folder over every range — judges a range that has
/// not handed anything over yet, such as a half just split off.
fn split_of(live: &Live, fallback: Option<(usize, usize)>) -> Option<Split> {
    let folders = &live.range.folders;
    let current = current_folder(live);
    let end = match &live.range.until {
        Some(until) => folders.partition_point(|f| f < until),
        None => folders.len(),
    };
    let unreached = end.checked_sub(current + 1).filter(|n| *n > 0)?;
    let at = current + 1 + unreached / 2;
    let keys = rate_of(live)
        .or(fallback)
        .map(|(handed, reached)| handed * (end - at) / reached.max(1));
    Some(Split { at, end, keys })
}

fn parent_of(folder: &str) -> &str {
    let inner = folder.strip_suffix('/').unwrap_or(folder);
    match inner.rfind('/') {
        Some(i) => &folder[..=i],
        None => "",
    }
}

/// Where a range starting at `folder` begins asking: just before the
/// folder's name.
fn seek(folder: &str) -> String {
    folder.strip_suffix('/').unwrap_or(folder).to_string()
}

/// A delimiter page's objects and folders, merged into key order. A key and a
/// folder compare the way the folder's first key would: a key holds no `/`
/// past the parent, so every byte that decides is within the folder's name.
fn merge_page(items: &mut Vec<Item>, page: ListPage) {
    let mut objects = page.objects.into_iter().peekable();
    let mut prefixes = page.prefixes.into_iter().peekable();
    loop {
        let object_first = match (objects.peek(), prefixes.peek()) {
            (None, None) => return,
            (Some(_), None) => true,
            (None, Some(_)) => false,
            (Some(o), Some(p)) => o.key.as_str() < p.as_str(),
        };
        if !object_first {
            items.push(Item::Folder(prefixes.next().expect("peeked")));
            continue;
        }
        let object = objects.next().expect("peeked");
        match items.last_mut() {
            Some(Item::Objects(run)) => run.push(object),
            _ => items.push(Item::Objects(vec![object])),
        }
    }
}

/// `f` over `inputs` on up to `workers` threads, each with its own copy of the
/// client; results in input order. The first failure ends it: no worker
/// takes another input, and that failure is what comes back.
fn on_workers<T: Sync, R: Send>(
    client: &Client,
    workers: usize,
    inputs: &[T],
    f: impl Fn(&mut Client, &T) -> Result<R> + Sync,
) -> Result<Vec<R>> {
    let next = AtomicUsize::new(0);
    let failure: Mutex<Option<anyhow::Error>> = Mutex::new(None);
    let results: Vec<Mutex<Option<R>>> = inputs.iter().map(|_| Mutex::new(None)).collect();
    std::thread::scope(|scope| {
        for _ in 0..workers.min(inputs.len()) {
            let mut client = client.clone();
            let (next, failure, results, f) = (&next, &failure, &results, &f);
            scope.spawn(move || loop {
                if lock(failure).is_some() {
                    return;
                }
                let i = next.fetch_add(1, Ordering::Relaxed);
                let Some(input) = inputs.get(i) else { return };
                match f(&mut client, input) {
                    Ok(result) => *lock(&results[i]) = Some(result),
                    Err(e) => {
                        lock(failure).get_or_insert(e);
                        return;
                    }
                }
            });
        }
    });
    if let Some(e) = failure
        .into_inner()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
    {
        return Err(e);
    }
    Ok(results
        .into_iter()
        .map(|slot| {
            slot.into_inner()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .expect("every input ran")
        })
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn object(key: &str) -> Object {
        Object {
            key: key.into(),
            size: 1,
            last_modified: 0,
        }
    }

    fn live(folders: &[&str], emitted: Option<&str>, until: Option<&str>, handed: usize) -> Live {
        Live {
            range: Range {
                prefix: "p/".into(),
                start_after: None,
                from: Some(folders[0].into()),
                until: until.map(str::to_string),
                folders: folders.iter().map(|f| f.to_string()).collect(),
            },
            emitted: emitted.map(str::to_string),
            handed,
            pages: VecDeque::new(),
            queued: 0,
            planned: true,
            busy: true,
            done: false,
        }
    }

    /// A split never hands over the folder the range is in, nor anything
    /// before it, gives away the later half of what is left, and guesses
    /// that half's keys from the folders already passed.
    #[test]
    fn a_split_falls_in_the_folders_not_yet_reached() {
        let f = ["p/a/", "p/b/", "p/c/", "p/d/", "p/e/"];
        let at = |l: Live| split_of(&l, None).map(|s| (s.at, s.end, s.keys));
        assert_eq!(
            at(live(&f, None, None, 0)),
            Some((3, 5, None)),
            "a is current; b..e left"
        );
        assert_eq!(
            at(live(&f, Some("p/b/x"), None, 600)),
            Some((3, 5, Some(600))),
            "c..e left, 300 keys a folder so far, d and e taken"
        );
        assert_eq!(at(live(&f, Some("p/d/x"), None, 8)), Some((4, 5, Some(2))));
        assert_eq!(
            at(live(&f, Some("p/e/x"), None, 9)).map(|s| s.0),
            None,
            "nothing left"
        );
        assert_eq!(
            at(live(&f, Some("p/a/x"), Some("p/c/"), 1)),
            Some((1, 2, Some(1))),
            "b is the only folder before the stop"
        );
        assert_eq!(at(live(&f, Some("p/b/z"), Some("p/c/"), 1)), None);

        let fresh = live(&f, None, None, 0);
        assert_eq!(
            split_of(&fresh, Some((1000, 10))).map(|s| s.keys),
            Some(Some(200)),
            "a fresh half is judged by the rate everywhere: d and e at 100 keys a folder"
        );
    }

    #[test]
    fn folders_know_their_parent_even_with_empty_segments() {
        assert_eq!(parent_of("a/"), "");
        assert_eq!(parent_of("a/b/"), "a/");
        assert_eq!(parent_of("a//"), "a/");
        assert_eq!(parent_of("/"), "");
        assert_eq!(parent_of("p//x/"), "p//");
    }

    /// Objects and folders of one delimiter page come out in the order their
    /// keys would: `a.txt` < `a/` < `a0` because `.` < `/` < `0`.
    #[test]
    fn a_delimiter_page_merges_into_key_order() {
        let page = ListPage {
            objects: vec![object("a.txt"), object("a0"), object("b")],
            prefixes: vec!["a/".into(), "b/".into()],
            is_truncated: false,
            next_continuation_token: None,
        };
        let mut items = Vec::new();
        merge_page(&mut items, page);
        let shape: Vec<String> = items
            .iter()
            .map(|i| match i {
                Item::Objects(o) => o
                    .iter()
                    .map(|o| o.key.clone())
                    .collect::<Vec<_>>()
                    .join("+"),
                Item::Folder(f) => format!("[{f}]"),
                Item::Rest(r) => format!("rest of {}", r.prefix),
            })
            .collect();
        assert_eq!(shape, ["a.txt", "[a/]", "a0+b", "[b/]"]);
    }

    /// Each range stops where the next step begins and begins at its first
    /// folder, so the parent's own object just before it is not listed twice.
    #[test]
    fn runs_become_ranges_that_end_where_the_next_step_begins() {
        let folder = |f: &str| Item::Folder(f.into());
        let rest = Range {
            prefix: "z/".into(),
            start_after: Some("z/k".into()),
            from: None,
            until: None,
            folders: Vec::new(),
        };
        let items = vec![
            Item::Objects(vec![object("p/a.txt")]),
            folder("p/a/"),
            folder("p/b/"),
            folder("p/c/"),
            Item::Objects(vec![object("p/m")]),
            folder("p/n/"),
            Item::Rest(rest.clone()),
        ];
        let run = |after: &str, folders: &[&str], until: &str| {
            Work::Range(Range {
                prefix: "p/".into(),
                start_after: Some(after.into()),
                from: Some(folders[0].into()),
                until: Some(until.into()),
                folders: folders.iter().map(|f| f.to_string()).collect(),
            })
        };
        assert_eq!(
            plan_work(items),
            vec![
                Work::Objects(vec![object("p/a.txt")]),
                run("p/a", &["p/a/", "p/b/", "p/c/"], "p/m"),
                Work::Objects(vec![object("p/m")]),
                run("p/n", &["p/n/"], "z/"),
                Work::Range(rest),
            ]
        );
    }
}
