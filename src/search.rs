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
}

impl Query {
    /// Terms are separated by spaces. Matching ignores case.
    pub fn parse(text: &str) -> Query {
        let terms = text
            .split_whitespace()
            .filter_map(|word| {
                let lower = word.to_lowercase();
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

    /// How well a name matches, higher being better, or `None` if it does
    /// not match at all.
    pub fn score(&self, name: &str) -> Option<i32> {
        let name = name.to_lowercase();
        let mut total = 0;
        for term in &self.terms {
            total += match term {
                Term::Fuzzy(letters) => fuzzy(letters, &name)?,
                Term::Prefix(start) => name.starts_with(start.as_str()).then_some(200)?,
                Term::Without(part) => (!name.contains(part.as_str())).then_some(0)?,
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
        let mut scored: Vec<(i32, &str)> =
            names.iter().filter_map(|name| Some((query.score(name)?, *name))).collect();
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
    fn an_unfinished_query_matches_everything() {
        assert!(Query::parse("  ").is_empty());
        assert!(Query::parse("^").is_empty());
        assert_eq!(matches("", NAMES).len(), NAMES.len());
    }
}
