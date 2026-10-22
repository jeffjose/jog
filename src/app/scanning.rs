//! The listing coming in after the window has opened: a large folder, or
//! one on a NAS, is shown as soon as its first files are found, and the
//! rest join the list where its order puts them as they are found.

use std::time::Instant;

use crate::files::{self, Found, Placed};

use super::*;

/// How often what was found is taken into the list: more often is more
/// work for the window for little to see.
const BATCH: Duration = Duration::from_millis(250);

/// For this long after the window opens, the list's first file is followed
/// if it changes, while it is the one on view: what is shown first is
/// what the order puts first, once that is known.
const FOLLOW: Duration = Duration::from_secs(3);

pub struct Scanning {
    /// What each file found so far is put in order by, by id.
    placed: Vec<Option<Placed>>,
    /// Found while the list could not take it: a search box was open.
    waiting: Vec<Found>,
    done: bool,
    started: Instant,
    opened: Instant,
}

impl App {
    /// The rest of the listing brought in as it is found, `scan` holding
    /// what was found before the window opened, in the list's order.
    pub fn keep_scanning(&mut self, scan: files::Scan, started: Instant) {
        let files::Scan { receiver, known } = scan;
        let placed = known.into_iter().map(|found| Some(found.placed)).collect();
        self.scan = Some(Scanning { placed, waiting: Vec::new(), done: false, started, opened: Instant::now() });
        let proxy = self.proxy.clone();
        std::thread::spawn(move || {
            use std::sync::mpsc::RecvTimeoutError;
            let (mut batch, mut sent) = (Vec::new(), Instant::now());
            loop {
                match receiver.recv_timeout(BATCH) {
                    Ok(found) => batch.extend(found),
                    Err(RecvTimeoutError::Timeout) => {}
                    Err(RecvTimeoutError::Disconnected) => break,
                }
                if !batch.is_empty() && sent.elapsed() >= BATCH {
                    if proxy.send_event(Event::Scanned(std::mem::take(&mut batch))).is_err() {
                        return;
                    }
                    sent = Instant::now();
                }
            }
            if !batch.is_empty() {
                let _ = proxy.send_event(Event::Scanned(batch));
            }
            let _ = proxy.send_event(Event::ScanDone);
        });
    }

    pub fn scanning(&self) -> bool {
        self.scan.is_some()
    }

    /// Files the scan found: each where the order puts it, the file on
    /// view staying on view, or the first, if that changes, while it is.
    pub(super) fn take_scanned(&mut self, found: Vec<Found>) {
        let Some(scan) = &mut self.scan else { return };
        scan.waiting.extend(found);
        // Not under an open search box, whose own lists would go stale.
        if self.palette.is_some() || scan.waiting.is_empty() {
            return;
        }
        let found = std::mem::take(&mut scan.waiting);
        let early = scan.opened.elapsed() < FOLLOW;
        self.verdicts.borrow_mut().clear();
        let (viewed, focus) = (self.id(), self.list[self.focus()]);
        let following = self.pos == 0 && self.grid.is_none() && early;
        let query = self.filter.as_ref().map(|(typed, _)| Query::parse(typed));
        let began = Instant::now();
        let first_new = self.files.len();
        self.loader.add_files(found.iter().map(|found| found.path.clone()));
        for found in found {
            let id = self.files.len();
            self.files.push(found.path);
            if let Some(scan) = &mut self.scan {
                scan.placed.resize(id, None);
                scan.placed.push(Some(found.placed));
            }
        }
        let new: Vec<usize> = (first_new..self.files.len()).collect();
        let matching: Vec<usize> = match &query {
            Some(query) => new.iter().copied().filter(|id| self.score(query, *id, false).is_some()).collect(),
            None => new.clone(),
        };
        let mut list = std::mem::take(&mut self.list);
        self.merge_in(&mut list, matching);
        self.list = list;
        if let Some((typed, mut all)) = self.filter.take() {
            self.merge_in(&mut all, new);
            self.filter = Some((typed, all));
        }
        self.sizes.clear();

        let at = |list: &[usize], id: usize| list.iter().position(|&other| other == id).unwrap_or(0);
        let pos = if following { 0 } else { at(&self.list, viewed) };
        let moved = self.list[pos] != viewed;
        self.pos = pos;
        let cursor = at(&self.list, focus);
        if let Some(grid) = &mut self.grid {
            grid.cursor = cursor;
        }
        if moved {
            self.error = None;
            self.reset_view();
        }
        if self.options.debug {
            eprintln!("jog: {:>7.1?} listing took in {} files", began.elapsed(), self.files.len() - first_new);
        }
        let count = self.total();
        self.toast(format!("Listing  ·  {}", grouped(count)));
        self.request();
        self.refresh();
        self.redraw();
    }

    /// `new`, ids of files just found, put into `list` where the order has
    /// them; a shuffled list takes them anywhere after the file on view.
    fn merge_in(&self, list: &mut Vec<usize>, mut new: Vec<usize>) {
        if new.is_empty() {
            return;
        }
        if self.shuffled {
            let mut rng = crate::rng::Rng::from_clock();
            for id in new {
                let from = (self.pos + 1).min(list.len());
                let at = from + rng.below(list.len() - from + 1);
                list.insert(at, id);
            }
            return;
        }
        let Some(scan) = &self.scan else { return };
        let (sort, reversed) = (self.sort, self.reversed);
        // Those not found by the scan, made since by a save or a copy, are
        // placed by reading them now.
        let missing: HashMap<usize, Placed> = (list.iter().chain(&new))
            .filter(|id| scan.placed.get(**id).is_none_or(Option::is_none))
            .map(|id| (*id, files::placed_now(&self.files[*id], sort)))
            .collect();
        let placed = |id: &usize| scan.placed.get(*id).and_then(Option::as_ref).unwrap_or_else(|| &missing[id]);
        let order = |a: &usize, b: &usize| {
            let order = files::found_order(sort, (&self.files[*a], placed(a)), (&self.files[*b], placed(b)));
            if reversed { order.reverse() } else { order }
        };
        new.sort_by(|a, b| placed(a).rank.cmp(&placed(b).rank));
        new.sort_by(order);
        insert_sorted(list, new, order);
    }

    /// All of it listed: what was waiting taken in, and the end said.
    pub(super) fn scan_done(&mut self) {
        let Some(scan) = &mut self.scan else { return };
        scan.done = true;
        if !scan.waiting.is_empty() {
            // Taken in when the search box closes.
            if self.palette.is_some() {
                return;
            }
            self.take_scanned(Vec::new());
        }
        self.finish_scan();
    }

    /// Once the scan is done and nothing it found is waiting, it is let go.
    fn finish_scan(&mut self) {
        let Some(scan) = self.scan.take() else { return };
        if self.options.verbose {
            eprintln!("jog: {} files, all listed in {:.1?}", self.files.len(), scan.started.elapsed());
        }
        self.toast(format!("Listed  ·  {}", grouped(self.total())));
    }

    /// What the scan found while the search box was open, taken in now it
    /// has closed.
    pub(super) fn scan_catch_up(&mut self) {
        let Some(scan) = &self.scan else { return };
        if self.palette.is_some() || scan.waiting.is_empty() {
            return;
        }
        let done = scan.done;
        self.take_scanned(Vec::new());
        if done {
            self.finish_scan();
        }
    }
}

/// `new`, in `order` already, put into `list`, which is too: where each
/// goes looked for by halves, a few comparisons each however long the list
/// is, and all put in at once. Each goes after those it ties with.
fn insert_sorted(list: &mut Vec<usize>, new: Vec<usize>, order: impl Fn(&usize, &usize) -> std::cmp::Ordering) {
    let places: Vec<usize> = new.iter().map(|id| list.partition_point(|old| order(old, id) != std::cmp::Ordering::Greater)).collect();
    let old = std::mem::take(list);
    list.reserve(old.len() + new.len());
    let mut from = 0;
    for (id, at) in new.into_iter().zip(places) {
        list.extend_from_slice(&old[from..at]);
        list.push(id);
        from = at;
    }
    list.extend_from_slice(&old[from..]);
}

/// A count with its thousands set apart: 12,345.
fn grouped(count: usize) -> String {
    let digits = count.to_string();
    let mut out = String::new();
    for (at, digit) in digits.chars().enumerate() {
        if at > 0 && (digits.len() - at) % 3 == 0 {
            out.push(',');
        }
        out.push(digit);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn what_comes_in_is_merged_as_a_whole_sort_would_have_it() {
        // Values with ties, in batches as a scan brings them; ordered by
        // value, ties by id, as the rank breaks them.
        let values: Vec<u32> = (0..500u32).map(|n| (n * 7919) % 97).collect();
        let order = |a: &usize, b: &usize| values[*a].cmp(&values[*b]).then(a.cmp(b));
        let mut list = Vec::new();
        for batch in (0..values.len()).collect::<Vec<_>>().chunks(37) {
            let mut new = batch.to_vec();
            new.sort_by(order);
            insert_sorted(&mut list, new, order);
        }
        let mut whole: Vec<usize> = (0..values.len()).collect();
        whole.sort_by(order);
        assert_eq!(list, whole);
    }

    #[test]
    fn counts_are_grouped_in_thousands() {
        assert_eq!(grouped(7), "7");
        assert_eq!(grouped(1234), "1,234");
        assert_eq!(grouped(300000), "300,000");
        assert_eq!(grouped(1234567), "1,234,567");
    }
}
