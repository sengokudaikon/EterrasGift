//! Items joined into one phrase, as the user's language joins them: "a", "a and b", "a, b and c".

use spiderland_phrases::phrases;
use std::fmt;

/// The word before a [`list`]'s last item.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Conjunction {
    /// "a, b and c": all of them.
    And,
    /// "a, b or c": one of them.
    Or,
}

/// `items` as a list in the user's language. Each joint is a phrase of its own, so a translation chooses its separator
/// and where the conjunction goes.
pub fn list<T: fmt::Display>(items: &[T], conjunction: Conjunction) -> String {
    phrases! {
        AndTwo: "a list of two" => "{first} and {second}"
        OrTwo: "a list of two" => "{first} or {second}"
        More: "a longer list, before its last item" => "{list}, {item}"
        AndLast: "a longer list, its last item" => "{list} and {last}"
        OrLast: "a longer list, its last item" => "{list} or {last}"
    }
    let (two, last) = match conjunction {
        Conjunction::And => (AndTwo, AndLast),
        Conjunction::Or => (OrTwo, OrLast),
    };
    match items {
        [] => String::new(),
        [one] => one.to_string(),
        [first, second] => two.with(&[("first", first), ("second", second)]),
        [first, middle @ .., end] => {
            let list = middle.iter().fold(first.to_string(), |list, item| More.with(&[("list", &list), ("item", item)]));
            last.with(&[("list", &list), ("last", end)])
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn english_joins_the_last_item_with_its_conjunction() {
        assert_eq!(list::<&str>(&[], Conjunction::And), "");
        assert_eq!(list(&["a"], Conjunction::And), "a");
        assert_eq!(list(&["a", "b"], Conjunction::Or), "a or b");
        assert_eq!(list(&["a", "b", "c", "d"], Conjunction::And), "a, b, c and d");
    }
}
