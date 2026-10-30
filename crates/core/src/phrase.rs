//! The words the app shows, kept apart from the code that shows them so they can be translated.
//!
//! Each is a `const` [`Phrase`] (or [`Plural`]) declared beside the code that uses it, and used by name; the views'
//! own words are `@tr` strings in `crates/ui/ui/strings/`. A phrase's English is its gettext msgid, as the views' are:
//! `cargo xtask phrases` collects both into `lang/eterras-gift.pot` for translators. English that means two things
//! takes a context, `Phrase::ctx("key", "Delete")`, as a view's `@tr("key" => "Delete")` does.
//!
//! A phrase that takes values names them in braces, `"Added {name}."`, and [`Phrase::with`] fills them in, so a
//! translation can move them. A count goes through a [`Plural`], whose `{n}` picks the form; a list through [`list`].
//! The extractor reads `Phrase::new("…")`, `Phrase::ctx("…", "…")` and `Plural::new("…", "…")` as written: keep the
//! English literal.
//!
//! The translations themselves are `eg-ui`'s: its build script bundles `lang/<language>/LC_MESSAGES/eg-ui.po` into
//! the views and into a table of [`Catalog`]s, which [`Translations`] checks and [`Translations::select`] shows.

use std::collections::HashMap;
use std::fmt;
use std::sync::RwLock;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Phrase {
    context: Option<&'static str>,
    english: &'static str,
}

impl Phrase {
    pub const fn new(english: &'static str) -> Self {
        Self { context: None, english }
    }

    /// English that means something else elsewhere, told apart for translators by `context`.
    pub const fn ctx(context: &'static str, english: &'static str) -> Self {
        Self { context: Some(context), english }
    }

    /// In the user's language.
    pub fn get(self) -> &'static str {
        current().and_then(|language| language.text(self.context, self.english)).unwrap_or(self.english)
    }

    /// With each `{name}` replaced by its value; a name without a value stays as it is.
    pub fn with(self, values: &[(&str, &dyn fmt::Display)]) -> String {
        fill(self.get(), values)
    }
}

impl fmt::Display for Phrase {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.get())
    }
}

impl From<Phrase> for String {
    fn from(phrase: Phrase) -> Self {
        phrase.get().to_owned()
    }
}

/// A sentence about a count, in as many forms as the user's language has: `Plural::new("{n} draft", "{n} drafts")`.
/// English has two, the first for one.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Plural {
    one: &'static str,
    other: &'static str,
}

impl Plural {
    pub const fn new(one: &'static str, other: &'static str) -> Self {
        Self { one, other }
    }

    /// The form for `n`, in the user's language.
    pub fn get(self, n: u64) -> &'static str {
        current()
            .and_then(|language| language.plural(self.one, n))
            .unwrap_or(if n == 1 { self.one } else { self.other })
    }

    /// The form for `n`, with `{n}` filled in.
    pub fn count(self, n: usize) -> String {
        self.with(n, &[])
    }

    /// The form for `n`, with `{n}` and each named value filled in.
    pub fn with(self, n: usize, values: &[(&str, &dyn fmt::Display)]) -> String {
        let mut all: Vec<(&str, &dyn fmt::Display)> = vec![("n", &n)];
        all.extend_from_slice(values);
        fill(self.get(n as u64), &all)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Conjunction {
    And,
    Or,
}

/// Items as a list in the user's language: "a", "a and b", "a, b and c". Each joint is a phrase of its own, so a
/// translation chooses its separator and where the conjunction goes.
pub fn list<T: fmt::Display>(items: &[T], conjunction: Conjunction) -> String {
    const AND_TWO: Phrase = Phrase::ctx("a list of two", "{first} and {second}");
    const OR_TWO: Phrase = Phrase::ctx("a list of two", "{first} or {second}");
    const MORE: Phrase = Phrase::ctx("a longer list, before its last item", "{list}, {item}");
    const AND_LAST: Phrase = Phrase::ctx("a longer list, its last item", "{list} and {last}");
    const OR_LAST: Phrase = Phrase::ctx("a longer list, its last item", "{list} or {last}");
    let (two, last) = match conjunction {
        Conjunction::And => (AND_TWO, AND_LAST),
        Conjunction::Or => (OR_TWO, OR_LAST),
    };
    match items {
        [] => String::new(),
        [one] => one.to_string(),
        [first, second] => two.with(&[("first", first), ("second", second)]),
        [first, middle @ .., end] => {
            let list = middle.iter().fold(first.to_string(), |list, item| MORE.with(&[("list", &list), ("item", item)]));
            last.with(&[("list", &list), ("last", end)])
        }
    }
}

fn fill(text: &str, values: &[(&str, &dyn fmt::Display)]) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(open) = rest.find('{') {
        out.push_str(&rest[..open]);
        let tail = &rest[open..];
        match tail.find('}').map(|close| (&tail[1..close], close)) {
            Some((name, close)) if let Some((_, value)) = values.iter().find(|(key, _)| *key == name) => {
                out.push_str(&value.to_string());
                rest = &tail[close + 1..];
            }
            _ => {
                out.push('{');
                rest = &tail[1..];
            }
        }
    }
    out.push_str(rest);
    out
}

/// One language's translations as `eg-ui`'s build script bundles them, with what made it in: entries the
/// translator finished (not fuzzy, every form filled), as Slint takes them for the views.
pub struct Catalog {
    /// As its folder under `lang/` names it, e.g. "de" or "pt-BR".
    pub language: &'static str,
    /// The PO header's plural expression in `n`, e.g. `n != 1`.
    pub plural: &'static str,
    /// (context, msgid, forms): one form, or one per plural form for a [`Plural`] (its msgid is the singular).
    pub entries: &'static [(Option<&'static str>, &'static str, &'static [&'static str])],
}

/// The bundled catalogs, each plural rule parsed.
pub struct Translations(Vec<&'static Language>);

struct Language {
    name: &'static str,
    rule: Rule,
    entries: HashMap<(Option<&'static str>, &'static str), &'static [&'static str]>,
}

static CURRENT: RwLock<Option<&'static Language>> = RwLock::new(None);

fn current() -> Option<&'static Language> {
    *CURRENT.read().unwrap_or_else(|p| p.into_inner())
}

impl Translations {
    /// Fails on a plural expression it cannot read.
    pub fn new(catalogs: &'static [Catalog]) -> Result<Self, String> {
        catalogs
            .iter()
            .map(|catalog| {
                let rule = Rule::parse(catalog.plural).map_err(|error| format!("{}: plural rule {:?}: {error}", catalog.language, catalog.plural))?;
                let entries = catalog.entries.iter().map(|(context, msgid, forms)| ((*context, *msgid), *forms)).collect();
                // Once per catalog for the process's life, so its phrases can be `&'static`.
                Ok(&*Box::leak(Box::new(Language {
                    name: catalog.language,
                    rule,
                    entries,
                })))
            })
            .collect::<Result<_, _>>()
            .map(Self)
    }

    /// The catalog for a locale such as "de-AT": the same name, else the same language ("de"), as Slint picks its
    /// own at start; English (`None`) when there is neither.
    pub fn choose(&self, locale: &str) -> Option<&'static str> {
        let base = |name: &str| name.split(['-', '_', '@']).next().unwrap_or_default().to_ascii_lowercase();
        self.0
            .iter()
            .find(|language| language.name == locale)
            .or_else(|| self.0.iter().find(|language| base(language.name) == base(locale)))
            .map(|language| language.name)
    }

    /// Phrases show in `language` from now on; `None` is English.
    pub fn select(&self, language: Option<&str>) {
        let chosen = language.and_then(|name| self.0.iter().find(|language| language.name == name)).copied();
        *CURRENT.write().unwrap_or_else(|p| p.into_inner()) = chosen;
    }
}

impl Language {
    fn text(&self, context: Option<&str>, msgid: &str) -> Option<&'static str> {
        self.entries.get(&(context, msgid)).and_then(|forms| forms.first()).copied()
    }

    fn plural(&self, one: &str, n: u64) -> Option<&'static str> {
        let forms = self.entries.get(&(None, one))?;
        forms.get(self.rule.form(n)).or(forms.last()).copied()
    }
}

/// A gettext plural expression: C's integer operators over `n`, as `Plural-Forms: plural=…` writes them.
#[derive(Debug)]
struct Rule(Expr);

#[derive(Debug)]
enum Expr {
    N,
    Number(u64),
    Not(Box<Expr>),
    Binary(Op, Box<Expr>, Box<Expr>),
    Choose(Box<Expr>, Box<Expr>, Box<Expr>),
}

#[derive(Clone, Copy, Debug)]
enum Op {
    Or,
    And,
    Eq,
    Ne,
    Lt,
    Le,
    Gt,
    Ge,
    Add,
    Sub,
    Mul,
    Div,
    Rem,
}

impl Rule {
    fn parse(text: &str) -> Result<Self, String> {
        let mut parser = Parser {
            rest: text.trim().trim_end_matches(';'),
        };
        let expr = parser.choose()?;
        parser.skip();
        if parser.rest.is_empty() {
            Ok(Self(expr))
        } else {
            Err(format!("unexpected {:?}", parser.rest))
        }
    }

    fn form(&self, n: u64) -> usize {
        usize::try_from(eval(&self.0, n)).unwrap_or(usize::MAX)
    }
}

fn eval(expr: &Expr, n: u64) -> u64 {
    match expr {
        Expr::N => n,
        Expr::Number(value) => *value,
        Expr::Not(inner) => u64::from(eval(inner, n) == 0),
        Expr::Choose(test, yes, no) => eval(if eval(test, n) != 0 { yes } else { no }, n),
        Expr::Binary(op, left, right) => {
            let (a, b) = (eval(left, n), eval(right, n));
            match op {
                Op::Or => u64::from(a != 0 || b != 0),
                Op::And => u64::from(a != 0 && b != 0),
                Op::Eq => u64::from(a == b),
                Op::Ne => u64::from(a != b),
                Op::Lt => u64::from(a < b),
                Op::Le => u64::from(a <= b),
                Op::Gt => u64::from(a > b),
                Op::Ge => u64::from(a >= b),
                Op::Add => a.wrapping_add(b),
                Op::Sub => a.wrapping_sub(b),
                Op::Mul => a.wrapping_mul(b),
                Op::Div => a.checked_div(b).unwrap_or(0),
                Op::Rem => a.checked_rem(b).unwrap_or(0),
            }
        }
    }
}

/// Recursive descent in C's precedence: `?:`, `||`, `&&`, equality, comparison, `+ -`, `* / %`, `!`.
struct Parser<'a> {
    rest: &'a str,
}

impl Parser<'_> {
    fn skip(&mut self) {
        self.rest = self.rest.trim_start();
    }

    fn eat(&mut self, token: &str) -> bool {
        self.skip();
        // `<` must not take the start of `<=`, nor `!` of `!=`.
        let longer = matches!(token, "<" | ">" | "!") && self.rest.get(token.len()..).is_some_and(|after| after.starts_with('='));
        if self.rest.starts_with(token) && !longer {
            self.rest = &self.rest[token.len()..];
            true
        } else {
            false
        }
    }

    fn choose(&mut self) -> Result<Expr, String> {
        let test = self.binary(0)?;
        if !self.eat("?") {
            return Ok(test);
        }
        let yes = self.choose()?;
        if !self.eat(":") {
            return Err(format!("expected ':' at {:?}", self.rest));
        }
        Ok(Expr::Choose(Box::new(test), Box::new(yes), Box::new(self.choose()?)))
    }

    fn binary(&mut self, level: usize) -> Result<Expr, String> {
        const LEVELS: [&[(&str, Op)]; 6] = [
            &[("||", Op::Or)],
            &[("&&", Op::And)],
            &[("==", Op::Eq), ("!=", Op::Ne)],
            &[("<=", Op::Le), (">=", Op::Ge), ("<", Op::Lt), (">", Op::Gt)],
            &[("+", Op::Add), ("-", Op::Sub)],
            &[("*", Op::Mul), ("/", Op::Div), ("%", Op::Rem)],
        ];
        let Some(ops) = LEVELS.get(level) else {
            return self.unary();
        };
        let mut left = self.binary(level + 1)?;
        while let Some((_, op)) = ops.iter().find(|(token, _)| self.eat(token)) {
            left = Expr::Binary(*op, Box::new(left), Box::new(self.binary(level + 1)?));
        }
        Ok(left)
    }

    fn unary(&mut self) -> Result<Expr, String> {
        if self.eat("!") {
            return Ok(Expr::Not(Box::new(self.unary()?)));
        }
        if self.eat("(") {
            let inner = self.choose()?;
            return if self.eat(")") { Ok(inner) } else { Err(format!("expected ')' at {:?}", self.rest)) };
        }
        if self.eat("n") {
            return Ok(Expr::N);
        }
        let digits = self.rest.len() - self.rest.trim_start_matches(|c: char| c.is_ascii_digit()).len();
        let value = self.rest[..digits].parse().map_err(|_| format!("expected a value at {:?}", self.rest))?;
        self.rest = &self.rest[digits..];
        Ok(Expr::Number(value))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn values_fill_their_names_and_nothing_else() {
        const ADDED: Phrase = Phrase::new("Added {name}, {count} times. {missing} stays.");
        assert_eq!(ADDED.with(&[("name", &"Loom"), ("count", &3)]), "Added Loom, 3 times. {missing} stays.");
        assert_eq!(Phrase::new("No braces").with(&[]), "No braces");
        assert_eq!(Phrase::new("Open { only").with(&[("only", &1)]), "Open { only");
    }

    #[test]
    fn english_counts_and_lists() {
        const DRAFTS: Plural = Plural::new("{n} draft", "{n} drafts");
        assert_eq!([DRAFTS.count(0), DRAFTS.count(1), DRAFTS.count(2)], ["0 drafts", "1 draft", "2 drafts"]);
        assert_eq!(Plural::new("{n} mod in {place}", "{n} mods in {place}").with(3, &[("place", &"Mods")]), "3 mods in Mods");
        assert_eq!(list::<&str>(&[], Conjunction::And), "");
        assert_eq!(list(&["a"], Conjunction::And), "a");
        assert_eq!(list(&["a", "b"], Conjunction::Or), "a or b");
        assert_eq!(list(&["a", "b", "c", "d"], Conjunction::And), "a, b, c and d");
    }

    #[test]
    fn a_context_is_part_of_the_phrase() {
        assert_ne!(Phrase::ctx("key", "Delete"), Phrase::new("Delete"));
        assert_eq!(Phrase::ctx("key", "Delete").get(), "Delete");
    }

    #[test]
    fn plural_rules_pick_each_languages_forms() {
        let forms = |rule: &str, counts: &[u64]| {
            let rule = Rule::parse(rule).unwrap();
            counts.iter().map(|n| rule.form(*n)).collect::<Vec<_>>()
        };
        assert_eq!(forms("(n != 1);", &[0, 1, 2]), [1, 0, 1]);
        assert_eq!(forms("0", &[0, 1, 5]), [0, 0, 0]);
        let polish = "(n==1 ? 0 : n%10>=2 && n%10<=4 && (n%100<10 || n%100>=20) ? 1 : 2)";
        assert_eq!(forms(polish, &[1, 2, 4, 5, 12, 22, 25, 112]), [0, 1, 1, 2, 2, 1, 2, 2]);
        let arabic = "n==0 ? 0 : n==1 ? 1 : n==2 ? 2 : n%100>=3 && n%100<=10 ? 3 : n%100>=11 ? 4 : 5";
        assert_eq!(forms(arabic, &[0, 1, 2, 3, 10, 11, 99, 100, 102]), [0, 1, 2, 3, 3, 4, 4, 5, 5]);
        assert_eq!(forms("!(n < 2)", &[1, 2]), [0, 1]);
        for broken in ["n ==", "(n != 1", "n ? 1", "x"] {
            assert!(Rule::parse(broken).is_err(), "{broken}");
        }
    }

    #[test]
    fn a_catalog_answers_for_its_language() {
        static CATALOGS: [Catalog; 2] = [
            Catalog {
                language: "de",
                plural: "n != 1",
                entries: &[
                    (None, "Delete", &["Löschen"]),
                    (Some("key"), "Delete", &["Entf"]),
                    (None, "{n} draft", &["{n} Entwurf", "{n} Entwürfe"]),
                ],
            },
            Catalog {
                language: "pt-BR",
                plural: "n > 1",
                entries: &[],
            },
        ];
        let translations = Translations::new(&CATALOGS).unwrap();
        assert_eq!(translations.choose("de-AT"), Some("de"));
        assert_eq!(translations.choose("pt-BR"), Some("pt-BR"));
        assert_eq!(translations.choose("pt_PT"), Some("pt-BR"));
        assert_eq!(translations.choose("fr-FR"), None);
        let german = translations.0[0];
        assert_eq!(german.text(None, "Delete"), Some("Löschen"));
        assert_eq!(german.text(Some("key"), "Delete"), Some("Entf"));
        assert_eq!(german.text(None, "Undo"), None);
        assert_eq!([german.plural("{n} draft", 1), german.plural("{n} draft", 3)], [Some("{n} Entwurf"), Some("{n} Entwürfe")]);
        static BROKEN: [Catalog; 1] = [Catalog {
            language: "xx",
            plural: "n !=",
            entries: &[],
        }];
        assert!(Translations::new(&BROKEN).is_err());
    }
}
