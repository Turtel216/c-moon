//! Guessing what a misspelled identifier was meant to be.
//!
//! An unknown name is usually a typo of one that *is* in scope, so a "did you
//! mean" line turns a lookup error into an answer.

/// The nearest name to `name` among `candidates`, if one is close enough.
///
/// Closeness is Levenshtein distance -- the number of single-character
/// insertions, deletions and substitutions between the two spellings -- with a
/// budget of a third of the name's length, so `valeu` suggests `value` while
/// two unrelated names never suggest each other. Names shorter than three
/// characters get no suggestion at all: every other short name is one edit
/// away, so `b` would "probably mean" `a`.
///
/// # Arguments
///
/// * `name` - the identifier that could not be resolved
/// * `candidates` - the names that were in scope where it was used
///
/// # Examples
///
/// ```ignore
/// let names = ["value", "total"];
/// assert_eq!(nearest("valeu", names.into_iter()), Some("value"));
/// assert_eq!(nearest("zzz", names.into_iter()), None);
/// ```
pub fn nearest<'a>(name: &str, candidates: impl Iterator<Item = &'a str>) -> Option<&'a str> {
    let budget = name.chars().count() / 3;
    if budget == 0 {
        return None;
    }

    candidates
        .filter(|candidate| *candidate != name)
        .map(|candidate| (edit_distance(name, candidate, budget), candidate))
        .filter(|&(distance, _)| distance <= budget)
        // `min_by_key` keeps the first of several equally close names, which is
        // the innermost one when the candidates come from a scope stack.
        .min_by_key(|&(distance, _)| distance)
        .map(|(_, candidate)| candidate)
}

/// Levenshtein distance between `left` and `right`, giving up past `budget`.
///
/// Uses the usual dynamic program, but keeps only the previous row rather than
/// the whole matrix: the distance needs `O(min(len))` memory, not `O(len^2)`.
fn edit_distance(left: &str, right: &str, budget: usize) -> usize {
    let left: Vec<char> = left.chars().collect();
    let right: Vec<char> = right.chars().collect();

    // Two strings whose lengths differ by more than the budget cannot be within
    // it, and the check is free compared with filling the table.
    if left.len().abs_diff(right.len()) > budget {
        return budget + 1;
    }

    // Row zero: turning an empty prefix of `left` into a prefix of `right`
    // costs one insertion per character.
    let mut previous: Vec<usize> = (0..=right.len()).collect();
    let mut current = vec![0; right.len() + 1];

    for (i, &lc) in left.iter().enumerate() {
        current[0] = i + 1;
        for (j, &rc) in right.iter().enumerate() {
            let substitution = previous[j] + usize::from(lc != rc);
            let deletion = previous[j + 1] + 1;
            let insertion = current[j] + 1;
            current[j + 1] = substitution.min(deletion).min(insertion);
        }
        std::mem::swap(&mut previous, &mut current);
    }

    previous[right.len()]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn measures_single_edits() {
        assert_eq!(edit_distance("value", "value", 5), 0);
        assert_eq!(edit_distance("value", "valu", 5), 1);
        assert_eq!(edit_distance("value", "values", 5), 1);
        assert_eq!(edit_distance("value", "valve", 5), 1);
        assert_eq!(edit_distance("value", "valeu", 5), 2);
    }

    #[test]
    fn suggests_the_closest_name() {
        let names = ["counter", "value", "total"];
        assert_eq!(nearest("valu", names.into_iter()), Some("value"));
        assert_eq!(nearest("countr", names.into_iter()), Some("counter"));
    }

    #[test]
    fn suggests_nothing_for_an_unrelated_name() {
        let names = ["counter", "value"];
        assert_eq!(nearest("xyz", names.into_iter()), None);
        assert_eq!(nearest("i", names.into_iter()), None);
    }

    #[test]
    fn never_suggests_the_name_itself() {
        assert_eq!(nearest("value", ["value"].into_iter()), None);
    }

    #[test]
    fn handles_multibyte_names() {
        // Distance is counted in characters, so a two-byte character is one
        // edit rather than two.
        assert_eq!(edit_distance("naïve", "naive", 5), 1);
    }
}
