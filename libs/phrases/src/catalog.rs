//! The bundled translations: what an app's build script hands over, checked, and the one language phrases show in.

use crate::plural_rule::Rule;
use std::collections::HashMap;
use std::fmt;
use std::sync::RwLock;

/// One language's translations as an app's build script bundles them, with what made it in: entries the
/// translator finished (not fuzzy, every form filled), as Slint takes them for the views.
#[derive(Debug)]
pub struct Catalog {
    /// As its folder under `lang/` names it, e.g. "de" or "pt-BR".
    pub language: &'static str,
    /// The PO header's plural expression in `n`, e.g. `n != 1`.
    pub plural: &'static str,
    /// (context, msgid, forms): one form, or one per plural form for a [`Plural`](crate::Plural) (its msgid is the singular).
    pub entries: &'static [(Option<&'static str>, &'static str, &'static [&'static str])],
}

/// The bundled catalogs, each plural rule parsed.
pub struct Translations(Vec<&'static Language>);

impl fmt::Debug for Translations {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_tuple("Translations")
            .field(&self.0.iter().map(|language| language.name).collect::<Vec<_>>())
            .finish()
    }
}

/// A bundled catalog that can't be used, because its plural rule can't be read.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
#[error("the {language} catalog's plural rule {rule:?} can't be read: {reason}")]
pub struct Error {
    language: &'static str,
    rule: &'static str,
    reason: String,
}

impl Error {
    /// The catalog's language, as its folder under `lang/` names it.
    pub fn language(&self) -> &'static str {
        self.language
    }
}

pub(crate) struct Language {
    name: &'static str,
    rule: Rule,
    entries: HashMap<(Option<&'static str>, &'static str), &'static [&'static str]>,
}

static CURRENT: RwLock<Option<&'static Language>> = RwLock::new(None);

pub(crate) fn current() -> Option<&'static Language> {
    *CURRENT.read().unwrap_or_else(|p| p.into_inner())
}

impl Translations {
    /// Fails when a catalog's plural rule can't be read. A test that builds this from the app's catalogs finds that before it
    /// ships.
    pub fn new(catalogs: &'static [Catalog]) -> Result<Self, Error> {
        catalogs
            .iter()
            .map(|catalog| {
                let rule = Rule::parse(catalog.plural).map_err(|reason| Error {
                    language: catalog.language,
                    rule: catalog.plural,
                    reason,
                })?;
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
    pub(crate) fn text(&self, context: Option<&str>, msgid: &str) -> Option<&'static str> {
        self.entries.get(&(context, msgid)).and_then(|forms| forms.first()).copied()
    }

    pub(crate) fn plural(&self, one: &str, n: u64) -> Option<&'static str> {
        let forms = self.entries.get(&(None, one))?;
        forms.get(self.rule.form(n)).or(forms.last()).copied()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_catalog_answers_for_its_language() {
        static CATALOGS: [Catalog; 2] = [
            Catalog {
                language: "de",
                plural: "n != 1",
                entries: &[
                    (None, "Delete", &["Löschen"]),
                    (Some("key"), "Delete", &["Entf"]),
                    (None, "{n} file", &["{n} Datei", "{n} Dateien"]),
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
        assert_eq!([german.plural("{n} file", 1), german.plural("{n} file", 3)], [Some("{n} Datei"), Some("{n} Dateien")]);
        static BROKEN: [Catalog; 1] = [Catalog {
            language: "xx",
            plural: "n !=",
            entries: &[],
        }];
        let error = Translations::new(&BROKEN).expect_err("a plural rule that can't be read");
        assert_eq!(error.language(), "xx");
        assert!(error.to_string().starts_with(r#"the xx catalog's plural rule "n !=" can't be read: "#), "{error}");
    }
}
