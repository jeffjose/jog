//! The shell commands that have been run on files, kept between runs so
//! that one typed before can be picked again.

use std::path::PathBuf;

use crate::positions::state_dir;

/// How many commands to remember; the ones run longest ago go first.
const MAX_ENTRIES: usize = 200;

fn file() -> Option<PathBuf> {
    state_dir().map(|dir| dir.join("commands"))
}

/// The commands run so far, the latest first.
pub fn load() -> Vec<String> {
    let text = file().and_then(|file| std::fs::read_to_string(file).ok()).unwrap_or_default();
    text.lines().filter(|line| !line.trim().is_empty()).map(str::to_string).collect()
}

/// Puts `command` at the head of the list, taking it out from further
/// down if it was there.
fn promote(commands: &mut Vec<String>, command: &str) {
    commands.retain(|other| other != command);
    commands.insert(0, command.to_string());
    commands.truncate(MAX_ENTRIES);
}

/// Notes that `command` has been run, and writes the list out.
pub fn remember(commands: &mut Vec<String>, command: &str) {
    promote(commands, command);
    let Some(file) = file() else { return };
    let text: String = commands.iter().map(|command| format!("{command}\n")).collect();
    let written = file.parent().map_or(Ok(()), std::fs::create_dir_all).and_then(|()| std::fs::write(&file, text));
    if let Err(error) = written {
        eprintln!("jog: cannot remember commands in {}: {error}", file.display());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_command_run_again_moves_to_the_head_of_the_list() {
        let mut commands = vec!["one".to_string(), "two".to_string(), "three".to_string()];
        promote(&mut commands, "two");
        assert_eq!(commands, ["two", "one", "three"]);
        promote(&mut commands, "four");
        assert_eq!(commands, ["four", "two", "one", "three"]);
    }
}
