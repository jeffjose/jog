//! Matching file names against what is typed in the search box.

/// What was typed, split into terms that must all hold.
pub struct Query {
    terms: Vec<Term>,
}

enum Term {
    /// The letters appear in this order, not necessarily together.
    Fuzzy(Vec<char>),
    /// `^text`: the name starts with this.
    Prefix(String),
    /// `!text`: the name does not contain this.
    Without(String),
    /// `is:video` and the like.
    Is(Kind),
    /// `>10mb` or `<1gb`: larger than, or smaller than, this many bytes.
    Size { larger: bool, bytes: u64 },
}

#[derive(Clone, Copy)]
enum Kind {
    Video,
    Image,
    Gif,
    Marked,
}

/// What a query is matched against.
pub struct Facts<'a> {
    pub name: &'a str,
    pub video: bool,
    pub marked: bool,
    /// Size in bytes; only looked up when `Query::wants_size` says so.
    pub size: Option<u64>,
}

/// A size as typed: a number, then `b`, `kb`, `mb` or `gb` (or nothing,
/// for bytes).
fn parse_size(text: &str) -> Option<u64> {
    let digits = text.trim_end_matches(|c: char| c.is_ascii_alphabetic());
    let unit: f64 = match &text[digits.len()..] {
        "" | "b" => 1.0,
        "k" | "kb" => 1024.0,
        "m" | "mb" => 1024.0 * 1024.0,
        "g" | "gb" => 1024.0 * 1024.0 * 1024.0,
        _ => return None,
    };
    let number: f64 = digits.parse().ok().filter(|number| *number >= 0.0)?;
    Some((number * unit) as u64)
}

impl Query {
    /// Terms are separated by spaces. Matching ignores case.
    pub fn parse(text: &str) -> Query {
        let terms = text
            .split_whitespace()
            .filter_map(|word| {
                let lower = word.to_lowercase();
                // One of these still being typed narrows nothing yet.
                if let Some(kind) = lower.strip_prefix("is:") {
                    return Some(Term::Is(match kind {
                        "video" => Kind::Video,
                        "image" => Kind::Image,
                        "gif" => Kind::Gif,
                        "marked" => Kind::Marked,
                        _ => return None,
                    }));
                }
                if let Some(size) = lower.strip_prefix(['>', '<']) {
                    let bytes = parse_size(size)?;
                    return Some(Term::Size { larger: lower.starts_with('>'), bytes });
                }
                let (kind, rest) = match lower.chars().next() {
                    Some(mark @ ('^' | '!')) => (mark, lower[1..].to_string()),
                    _ => (' ', lower),
                };
                // A lone `^` or `!` is a term still being typed.
                (!rest.is_empty()).then(|| match kind {
                    '^' => Term::Prefix(rest),
                    '!' => Term::Without(rest),
                    _ => Term::Fuzzy(rest.chars().collect()),
                })
            })
            .collect();
        Query { terms }
    }

    pub fn is_empty(&self) -> bool {
        self.terms.is_empty()
    }

    /// Whether any term compares sizes, which means looking each file up.
    pub fn wants_size(&self) -> bool {
        self.terms.iter().any(|term| matches!(term, Term::Size { .. }))
    }

    /// How well a file matches, higher being better, or `None` if it does
    /// not match at all.
    pub fn score(&self, file: &Facts) -> Option<i32> {
        let name = file.name.to_lowercase();
        let mut total = 0;
        for term in &self.terms {
            total += match term {
                Term::Fuzzy(letters) => fuzzy(letters, &name)?,
                Term::Prefix(start) => name.starts_with(start.as_str()).then_some(200)?,
                Term::Without(part) => (!name.contains(part.as_str())).then_some(0)?,
                Term::Is(kind) => {
                    let is = match kind {
                        Kind::Video => file.video,
                        Kind::Image => !file.video,
                        Kind::Gif => name.ends_with(".gif"),
                        Kind::Marked => file.marked,
                    };
                    is.then_some(0)?
                }
                Term::Size { larger, bytes } => {
                    let size = file.size?;
                    (if *larger { size > *bytes } else { size < *bytes }).then_some(0)?
                }
            };
        }
        Some(total)
    }
}

/// Scores `letters` appearing in order within `name`. Runs of adjacent
/// letters and matches at the start of the name or of a word count for more,
/// so that the closest names come first.
fn fuzzy(letters: &[char], name: &str) -> Option<i32> {
    let name: Vec<char> = name.chars().collect();
    let word_start = |at: usize| at == 0 || !name[at - 1].is_alphanumeric();

    // The letters together, as typed, beat any scattered match.
    let together = name.windows(letters.len()).position(|window| window == letters);
    if let Some(at) = together {
        let place = if at == 0 { 60 } else if word_start(at) { 30 } else { 0 };
        return Some(100 + place + 10 * letters.len() as i32);
    }

    let (mut score, mut wanted, mut previous) = (0, 0, None);
    for (at, letter) in name.iter().enumerate() {
        if wanted < letters.len() && *letter == letters[wanted] {
            score += 10;
            if previous == Some(at.wrapping_sub(1)) {
                score += 15;
            }
            if word_start(at) {
                score += 10;
            }
            previous = Some(at);
            wanted += 1;
        }
    }
    (wanted == letters.len()).then_some(score)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn matches<'a>(query: &str, names: &[&'a str]) -> Vec<&'a str> {
        let query = Query::parse(query);
        let facts = |name: &'a str| Facts { name, video: name.ends_with(".mp4"), marked: name == "bark.jpg", size: Some(name.len() as u64 * 1024) };
        let mut scored: Vec<(i32, &str)> =
            names.iter().filter_map(|name| Some((query.score(&facts(name))?, *name))).collect();
        scored.sort_by_key(|(score, _)| -score);
        scored.into_iter().map(|(_, name)| name).collect()
    }

    const NAMES: &[&str] = &["berserker-bloodmoon.mp4", "bark.jpg", "My-Foo.png", "foo-bar.jpg", "food.webp", "alien.jpg"];

    #[test]
    fn letters_match_in_order_and_ignore_case() {
        assert_eq!(matches("brk", NAMES), ["bark.jpg", "berserker-bloodmoon.mp4"]);
        assert_eq!(matches("FOO", NAMES), ["foo-bar.jpg", "food.webp", "My-Foo.png"]);
        assert!(matches("zzz", NAMES).is_empty());
    }

    #[test]
    fn prefix_exclusion_and_several_terms() {
        assert_eq!(matches("^foo", NAMES), ["foo-bar.jpg", "food.webp"]);
        assert_eq!(matches("foo !bar", NAMES), ["food.webp", "My-Foo.png"]);
        assert_eq!(matches("foo .jpg", NAMES), ["foo-bar.jpg"]);
        assert_eq!(matches(".jpg", NAMES).len(), 3);
    }

    #[test]
    fn kinds_and_sizes() {
        assert_eq!(matches("is:video", NAMES), ["berserker-bloodmoon.mp4"]);
        assert_eq!(matches("is:image foo", NAMES).len(), 3);
        assert_eq!(matches("is:marked", NAMES), ["bark.jpg"]);
        // Each name stands for a file of as many kilobytes as it has letters.
        assert_eq!(matches(">20kb", NAMES), ["berserker-bloodmoon.mp4"]);
        assert_eq!(matches("<9KB", NAMES), ["bark.jpg"]);
        assert_eq!(matches(">0.02mb is:video", NAMES), ["berserker-bloodmoon.mp4"]);
        assert!(matches(">1gb", NAMES).is_empty());
        assert_eq!(parse_size("1.5gb"), Some(1_610_612_736));
        assert_eq!(parse_size("10"), Some(10));
        assert_eq!(parse_size("10xb"), None);
    }

    #[test]
    fn an_unfinished_query_matches_everything() {
        assert!(Query::parse("is:vid").is_empty());
        assert!(Query::parse(">").is_empty());
        assert!(Query::parse("  ").is_empty());
        assert!(Query::parse("^").is_empty());
        assert_eq!(matches("", NAMES).len(), NAMES.len());
    }
}
