//! The player processes running, for the Performance drawer: which they
//! are, and what each takes of the machine, read from /proc, which is the
//! kernel's memory and costs no waiting on a disk.

use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::Mutex;
use std::time::{Duration, Instant};

use super::Role;

struct Entry {
    pid: i32,
    role: Role,
    started: Instant,
    /// Its CPU time when last asked, and when that was: how busy it has
    /// been since is the difference.
    seen: Option<(u64, Instant)>,
}

static RUNNING: Mutex<Vec<Entry>> = Mutex::new(Vec::new());
static CRASHED: AtomicU32 = AtomicU32::new(0);

/// A player process started.
pub(super) fn started(pid: i32, role: Role) {
    RUNNING.lock().unwrap_or_else(std::sync::PoisonError::into_inner).push(Entry { pid, role, started: Instant::now(), seen: None });
}

/// A player process gone; `crashed` if it was not asked to go.
pub(super) fn ended(pid: i32, crashed: bool) {
    RUNNING.lock().unwrap_or_else(std::sync::PoisonError::into_inner).retain(|entry| entry.pid != pid);
    if crashed {
        CRASHED.fetch_add(1, Ordering::Relaxed);
    }
}

/// How many player processes have crashed since jog started.
pub fn crashes_this_run() -> u32 {
    CRASHED.load(Ordering::Relaxed)
}

/// What a player process takes of the machine.
#[derive(Clone, Debug)]
pub struct Usage {
    pub pid: i32,
    pub role: Role,
    /// Of one core, in percent, since it was last asked; none the first time.
    pub cpu: Option<f64>,
    /// Resident memory, in bytes.
    pub memory: Option<u64>,
    pub age: Duration,
}

/// Each player process running, and what it takes.
pub fn usage() -> Vec<Usage> {
    // SAFETY: sysconf takes a plain number.
    let (ticks, page) = unsafe { (libc::sysconf(libc::_SC_CLK_TCK).max(1) as f64, libc::sysconf(libc::_SC_PAGESIZE).max(1) as u64) };
    let now = Instant::now();
    let mut running = RUNNING.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    running
        .iter_mut()
        .map(|entry| {
            let cpu_time = cpu_ticks(entry.pid);
            let cpu = match (cpu_time, entry.seen) {
                (Some(time), Some((before, at))) if now > at => Some((time.saturating_sub(before) as f64 / ticks) / now.duration_since(at).as_secs_f64() * 100.0),
                _ => None,
            };
            entry.seen = cpu_time.map(|time| (time, now));
            let memory = std::fs::read_to_string(format!("/proc/{}/statm", entry.pid)).ok().and_then(|text| text.split_whitespace().nth(1)?.parse::<u64>().ok()).map(|pages| pages * page);
            Usage { pid: entry.pid, role: entry.role, cpu, memory, age: now.duration_since(entry.started) }
        })
        .collect()
}

/// The CPU time a process has had, user and system, in clock ticks.
fn cpu_ticks(pid: i32) -> Option<u64> {
    let text = std::fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
    // After the name, which is in brackets and may hold spaces: the state
    // is the 3rd field, user time the 14th and system time the 15th.
    let rest = &text[text.rfind(')')? + 2..];
    let fields: Vec<&str> = rest.split_whitespace().collect();
    Some(fields.get(11)?.parse::<u64>().ok()? + fields.get(12)?.parse::<u64>().ok()?)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn this_process_has_cpu_time() {
        assert!(cpu_ticks(std::process::id() as i32).is_some());
    }
}
