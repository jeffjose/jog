//! Moving, copying and trashing files on a thread of their own: across
//! drives a video takes seconds to copy, and the window must not stand
//! still for it. The list and the toast change at once, as if it were
//! done; what fails is put back and said when it does.

use std::sync::atomic::AtomicUsize;

use super::*;

/// Jobs still going: quitting waits for them, so that no copy is left
/// half written, and Undo waits so as not to overtake one.
static RUNNING: AtomicUsize = AtomicUsize::new(0);

/// What a job does to each file.
#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Work {
    Move,
    Copy,
    /// A copy of the picture on view, beside where jog was started.
    Save,
    Trash,
    /// A move taken back, to where each file came from.
    MoveBack,
    /// A move taken back and now done again.
    MoveAgain,
}

/// A job done: the files it got through and those it did not, with why.
pub struct FilesDone {
    work: Work,
    done: Vec<Moved>,
    failed: Vec<(Moved, String)>,
}

/// Whether a job is still going.
pub(super) fn busy() -> bool {
    RUNNING.load(atomic::Ordering::SeqCst) > 0
}

/// Waits for the jobs going to finish, for quitting.
pub(super) fn finish() {
    let mut said = false;
    while busy() {
        if !std::mem::replace(&mut said, true) {
            eprintln!("jog: finishing moving files…");
        }
        std::thread::sleep(Duration::from_millis(20));
    }
}

fn run(work: Work, item: &Moved) -> Result<(), String> {
    let result = match work {
        Work::Move | Work::MoveAgain => ops::move_file(&item.from, &item.to),
        Work::Copy | Work::Save => ops::copy_file(&item.from, &item.to),
        Work::MoveBack => ops::move_file(&item.to, &item.from),
        Work::Trash => {
            let status = std::process::Command::new("gio").arg("trash").arg("--").arg(&item.from).stderr(std::process::Stdio::null()).status();
            return match status {
                Ok(status) if status.success() => Ok(()),
                Ok(_) => Err("gio could not trash it".into()),
                Err(error) => Err(format!("gio: {error}")),
            };
        }
    };
    // "Permission denied", without the "(os error 13)" after it.
    result.map_err(|error| {
        let words = error.to_string();
        words.find(" (os error").map_or(words.clone(), |end| words[..end].to_string())
    })
}

impl App {
    /// Starts a job on its own thread; `Event::FilesDone` tells of it.
    pub(super) fn file_work(&mut self, work: Work, items: Vec<Moved>) {
        if items.is_empty() {
            return;
        }
        RUNNING.fetch_add(1, atomic::Ordering::SeqCst);
        let proxy = self.proxy.clone();
        std::thread::spawn(move || {
            let (mut done, mut failed) = (Vec::new(), Vec::new());
            for item in items {
                match run(work, &item) {
                    Ok(()) => done.push(item),
                    Err(error) => failed.push((item, error)),
                }
            }
            let _ = proxy.send_event(Event::FilesDone(FilesDone { work, done, failed }));
            RUNNING.fetch_sub(1, atomic::Ordering::SeqCst);
        });
    }

    /// A job is done: what it did is said on the terminal and can be
    /// undone, and what failed is put back as it was.
    pub(super) fn files_done(&mut self, job: FilesDone, event_loop: &ActiveEventLoop) {
        let FilesDone { work, done, failed } = job;
        for item in &done {
            match work {
                Work::Move => println!("[move] {} -> {}", item.from.display(), item.to.display()),
                Work::Copy | Work::Save => println!("[copy to] {} -> {}", item.from.display(), item.to.display()),
                Work::Trash => println!("[trash] {}", item.from.display()),
                Work::MoveBack => println!("[undo] {} -> {}", item.to.display(), item.from.display()),
                Work::MoveAgain => println!("[redo] {} -> {}", item.from.display(), item.to.display()),
            }
        }
        for (item, error) in &failed {
            eprintln!("jog: {}: {error}", item.from.display());
        }
        let done_any = !done.is_empty();
        match work {
            Work::Move if done_any => self.did(Undo::Moved(done)),
            Work::Copy if done_any => self.did(Undo::Created(done.into_iter().map(|copy| copy.to).collect())),
            Work::MoveBack if done_any => self.redo.push(Redo::Moved(done)),
            Work::MoveAgain if done_any => self.undo.push(Undo::Moved(done)),
            // Its words still up: now with Show and Undo to click.
            Work::Save if done_any => {
                let saved = done.into_iter().map(|copy| copy.to).collect::<Vec<_>>();
                let said = format!("Saved {}", saved[0].display());
                match self.toast.as_ref().is_some_and(|(words, _)| *words == said) {
                    true => self.made(saved[0].clone(), said),
                    false => {
                        self.adopt_created(&saved);
                        self.did(Undo::Created(saved));
                    }
                }
            }
            _ => {}
        }
        if failed.is_empty() {
            return;
        }
        // Those that stayed where they were come back into the list, or,
        // not moved back, leave it again.
        let (word, error) = (match work {
            Work::Move | Work::MoveAgain => "Not moved",
            Work::Copy => "Not copied",
            Work::Save => "Not saved",
            Work::Trash => "Not trashed",
            Work::MoveBack => "Not moved back",
        }, failed[0].1.clone());
        let named = match failed.as_slice() {
            [(only, _)] => only.from.file_name().unwrap_or_default().to_string_lossy().into_owned(),
            many => format!("{} files", many.len()),
        };
        match work {
            Work::Copy | Work::Save => {}
            Work::MoveBack => {
                let gone: Vec<(usize, usize)> = failed.iter().filter_map(|(item, _)| Some((item.id, self.list.iter().position(|id| *id == item.id)?))).collect();
                self.drop_from_list(&gone, event_loop);
            }
            Work::Move | Work::MoveAgain | Work::Trash => {
                let viewing = self.list.get(self.pos).copied();
                for (item, _) in &failed {
                    self.put_back(item);
                }
                self.pos = viewing.and_then(|viewing| self.list.iter().position(|id| *id == viewing)).unwrap_or(self.pos);
                self.request();
                self.refresh();
            }
        }
        self.toast(format!("{word} · {named} · {error}"));
    }
}
