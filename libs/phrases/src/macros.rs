/// Declares named phrases, one after the other with no separator between them:
///
/// ```
/// phrases::phrases! {
///     /// Shown over the item list.
///     pub AddItem: "Add item"
///     pub(crate) Level: "Level {level}"
///     OpenVerb: "verb" => "Open"
///     Files: "{n} file" | "{n} files"
/// }
/// assert_eq!(AddItem.get(), "Add item");
/// assert_eq!(Files.count(2), "2 files");
/// ```
///
/// `Name: "English"` and `Name: "context" => "English"` are [`Phrase`](crate::Phrase)s, as a view's `@tr("context" => …)`
/// tells English apart, and `Name: "one" | "other"` is a [`Plural`](crate::Plural), as a view's `@tr("one" | "other" % n)`
/// is. Each is a plain `const`, so it costs what the `const` costs and works in a function as at the top of a module.
/// Names are in CamelCase, which reads as the word it stands for at its use: `tr::UnsavedChanges.get()`.
///
/// The English has to be a literal for `build::Template` to read, and this takes nothing else.
#[macro_export]
macro_rules! phrases {
    (@item [$($attr:tt)*] [$vis:vis] $name:ident $english:literal) => {
        $($attr)* #[allow(non_upper_case_globals)] $vis const $name: $crate::Phrase = $crate::Phrase::new($english);
    };
    (@item [$($attr:tt)*] [$vis:vis] $name:ident $context:literal => $english:literal) => {
        $($attr)* #[allow(non_upper_case_globals)] $vis const $name: $crate::Phrase = $crate::Phrase::ctx($context, $english);
    };
    (@item [$($attr:tt)*] [$vis:vis] $name:ident $one:literal | $other:literal) => {
        $($attr)* #[allow(non_upper_case_globals)] $vis const $name: $crate::Plural = $crate::Plural::new($one, $other);
    };
    ($($(#[$attr:meta])* $vis:vis $name:ident : $first:literal $(=> $english:literal)? $(| $other:literal)?)*) => {
        $($crate::phrases! { @item [$(#[$attr])*] [$vis] $name $first $(=> $english)? $(| $other)? })*
    };
}

#[cfg(test)]
mod tests {
    use crate::{Phrase, Plural};

    phrases! {
        /// Doc comments and attributes reach the constant.
        #[cfg(windows)]
        pub OnWindows: "On Windows"
        pub(crate) Braces: "Added {name}."
        // A comment between entries.
        Context: "verb" => "Open"
        Forms: "{n} file" | "{n} files"
        Continued: "one \
                    line"
        Raw: r#"say "hi""#
    }

    #[test]
    fn each_shape_is_the_constant_it_stands_for() {
        phrases! { InAFunction: "Inside a function" }
        #[cfg(windows)]
        assert_eq!(OnWindows, Phrase::new("On Windows"));
        assert_eq!(Braces.with(&[("name", &"notes.txt")]), "Added notes.txt.");
        assert_eq!(Context, Phrase::ctx("verb", "Open"));
        assert_eq!(Forms, Plural::new("{n} file", "{n} files"));
        assert_eq!(Continued.get(), "one line");
        assert_eq!(Raw.get(), "say \"hi\"");
        assert_eq!(InAFunction.get(), "Inside a function");
    }
}
