//! Paths written for a shell, on one line, to be pasted after `rm` or
//! `ls`: the files of a folder folded together with braces, as
//! `/photos/IMG_{0412,0415}.jpg`, and quoted where the shell would
//! otherwise take a character for something else. Braces with commas only,
//! never ranges like `{1..9}`: tcsh, csh and fish do not know those.

use std::cmp::Ordering;
use std::path::{Path, PathBuf};

/// The paths on one line, each folder's files folded into one word.
pub fn compress(paths: &[PathBuf]) -> String {
    // The folders in the order their first file came, each with its files.
    let mut folders: Vec<(PathBuf, Vec<String>)> = Vec::new();
    for path in paths {
        let path = std::path::absolute(path).unwrap_or_else(|_| path.clone());
        let folder = path.parent().map(Path::to_path_buf).unwrap_or_default();
        let name = path.file_name().map(|name| name.to_string_lossy().into_owned()).unwrap_or_default();
        match folders.iter_mut().find(|(other, _)| *other == folder) {
            Some((_, names)) => names.push(name),
            None => folders.push((folder, vec![name])),
        }
    }
    let words: Vec<String> = folders.into_iter().map(|(folder, names)| fold(&folder, names)).collect();
    words.join(" ")
}

/// One folder's files as one word.
fn fold(folder: &Path, mut names: Vec<String>) -> String {
    let base = folder.to_string_lossy();
    let base = if base.ends_with('/') { base.into_owned() } else { format!("{base}/") };
    names.sort_by(|a, b| natural(a, b));
    names.dedup();
    if let [only] = names.as_slice() {
        return quote(&format!("{base}{only}"));
    }
    let chars: Vec<Vec<char>> = names.iter().map(|name| name.chars().collect()).collect();
    let shortest = chars.iter().map(Vec::len).min().unwrap_or(0);
    let mut front = (0..shortest).take_while(|at| chars.iter().all(|name| name[*at] == chars[0][*at])).count();
    let mut back = (0..shortest - front).take_while(|at| chars.iter().all(|name| name[name.len() - 1 - at] == chars[0][chars[0].len() - 1 - at])).count();
    // What is shared ends, and begins again, only between a word or a
    // number and something else: IMG_10 and IMG_11 are IMG_{10,11}, not
    // IMG_1{0,1}, and a.png and b.jpg are {a.png,b.jpg}, not {a.pn,b.jp}g.
    // A letter and a digit side by side are two words: x1 and x2 are x{1,2}.
    let class = |c: &char| if c.is_ascii_digit() { 1 } else if c.is_alphanumeric() { 2 } else { 0 };
    let joined = |a: Option<&char>, b: Option<&char>| matches!((a.map(class), b.map(class)), (Some(x), Some(y)) if x == y && x != 0);
    let splits = |at: usize| chars.iter().any(|name| joined(name.get(at.wrapping_sub(1)), name.get(at)));
    while front > 0 && splits(front) {
        front -= 1;
    }
    let splits_back = |back: usize| chars.iter().any(|name| {
        let at = name.len() - back;
        joined(name.get(at.wrapping_sub(1)), name.get(at))
    });
    while back > 0 && splits_back(back) {
        back -= 1;
    }
    let prefix: String = chars[0][..front].iter().collect();
    let suffix: String = chars[0][chars[0].len() - back..].iter().collect();
    let middles: Vec<String> = chars.iter().map(|name| name[front..name.len() - back].iter().collect::<String>()).map(|middle| quote_bare(&middle)).collect();
    format!("{}{{{}}}{}", quote(&format!("{base}{prefix}")), middles.join(","), quote_bare(&suffix))
}

/// Whether a character means nothing to a shell, and needs no quotes.
fn plain(c: char) -> bool {
    c.is_ascii_alphanumeric() || "/._-+:@%^=".contains(c)
}

/// A part of a word quoted if it needs to be; a part that is empty stays
/// empty, as one between braces may be.
fn quote_bare(text: &str) -> String {
    if text.chars().all(plain) {
        return text.to_string();
    }
    quote(text)
}

/// Text quoted for a shell: in single quotes, a quote in it closed,
/// escaped and opened again; and an exclamation mark escaped outside the
/// quotes, as csh and tcsh see it even inside them.
fn quote(text: &str) -> String {
    if !text.is_empty() && text.chars().all(plain) {
        return text.to_string();
    }
    let parts: Vec<String> = text.split('!').map(|part| if part.is_empty() { String::new() } else { format!("'{}'", part.replace('\'', r"'\''")) }).collect();
    parts.join(r"\!")
}

/// Names in the order a person counts them: the numbers in them by their
/// value, so that 9 comes before 10.
fn natural(a: &str, b: &str) -> Ordering {
    let (mut a, mut b) = (a.chars().peekable(), b.chars().peekable());
    loop {
        match (a.peek().copied(), b.peek().copied()) {
            (None, None) => return Ordering::Equal,
            (None, _) => return Ordering::Less,
            (_, None) => return Ordering::Greater,
            (Some(x), Some(y)) if x.is_ascii_digit() && y.is_ascii_digit() => {
                let number = |chars: &mut std::iter::Peekable<std::str::Chars>| {
                    let mut digits = String::new();
                    while let Some(c) = chars.peek().copied().filter(char::is_ascii_digit) {
                        digits.push(c);
                        chars.next();
                    }
                    digits
                };
                let (x, y) = (number(&mut a), number(&mut b));
                let (x_trimmed, y_trimmed) = (x.trim_start_matches('0'), y.trim_start_matches('0'));
                let order = x_trimmed.len().cmp(&y_trimmed.len()).then(x_trimmed.cmp(y_trimmed)).then(x.len().cmp(&y.len()));
                if order != Ordering::Equal {
                    return order;
                }
            }
            (Some(x), Some(y)) => {
                if x != y {
                    return x.cmp(&y);
                }
                a.next();
                b.next();
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn paths(list: &[&str]) -> Vec<PathBuf> {
        list.iter().map(PathBuf::from).collect()
    }

    #[test]
    fn a_folders_files_fold_into_braces_numbers_whole() {
        assert_eq!(compress(&paths(&["/p/image-10.png", "/p/image-1.png", "/p/image-5.png"])), "/p/image-{1,5,10}.png");
        assert_eq!(compress(&paths(&["/p/IMG_10.jpg", "/p/IMG_11.jpg"])), "/p/IMG_{10,11}.jpg");
        assert_eq!(compress(&paths(&["/p/a.png", "/p/b.jpg"])), "/p/{a.png,b.jpg}");
        assert_eq!(compress(&paths(&["/p/one.mp4"])), "/p/one.mp4");
    }

    #[test]
    fn folders_are_words_of_their_own_in_the_order_they_came() {
        assert_eq!(compress(&paths(&["/b/x1.png", "/a/y.png", "/b/x2.png"])), "/b/x{1,2}.png /a/y.png");
    }

    #[test]
    fn what_a_shell_would_take_for_something_else_is_quoted() {
        assert_eq!(compress(&paths(&["/My Photos/img 1.jpg", "/My Photos/img 2.jpg"])), "'/My Photos/img '{1,2}.jpg");
        assert_eq!(compress(&paths(&["/p/it's.png"])), r"'/p/it'\''s.png'");
        assert_eq!(compress(&paths(&["/p/wow!.png"])), r"'/p/wow'\!'.png'");
        assert_eq!(compress(&paths(&["/p/a,b.png", "/p/c.png"])), "/p/{'a,b',c}.png");
    }

    #[test]
    fn numbers_count_by_their_value() {
        let mut names = vec!["10", "9", "1", "01"];
        names.sort_by(|a, b| natural(a, b));
        assert_eq!(names, ["1", "01", "9", "10"]);
    }
}
