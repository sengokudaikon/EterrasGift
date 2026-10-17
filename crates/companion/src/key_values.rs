//! Steam's text KeyValues format (libraryfolders.vdf, appmanifest_*.acf): quoted keys followed by a quoted value or a
//! braced block. Keys compare case-insensitively, as Steam's do. Comments and conditionals are not used by these files.

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct KeyValues {
    /// The text of a leaf; None for a block.
    pub value: Option<String>,
    pub children: Vec<(String, KeyValues)>,
}

impl KeyValues {
    /// The first child with this key, whatever its case.
    pub fn get(&self, key: &str) -> Option<&KeyValues> {
        self.children.iter().find(|(name, _)| name.to_lowercase() == key.to_lowercase()).map(|(_, child)| child)
    }

    /// Reads a whole file's text; None when it is not KeyValues.
    pub fn parse(text: &str) -> Option<KeyValues> {
        let chars: Vec<char> = text.chars().collect();
        let mut position = 0;
        let mut root = KeyValues::default();
        root.read_block(&chars, &mut position, false).then_some(root)
    }

    fn read_block(&mut self, text: &[char], position: &mut usize, nested: bool) -> bool {
        loop {
            skip_space(text, position);
            let Some(&next) = text.get(*position) else {
                return !nested;
            };
            if next == '}' {
                *position += 1;
                return nested;
            }
            let Some(key) = read_string(text, position) else {
                return false;
            };
            skip_space(text, position);
            let child = match text.get(*position) {
                None => return false,
                Some('{') => {
                    *position += 1;
                    let mut child = KeyValues::default();
                    if !child.read_block(text, position, true) {
                        return false;
                    }
                    child
                }
                Some(_) => match read_string(text, position) {
                    Some(value) => KeyValues {
                        value: Some(value),
                        children: Vec::new(),
                    },
                    None => return false,
                },
            };
            self.children.push((key, child));
        }
    }
}

fn skip_space(text: &[char], position: &mut usize) {
    while text.get(*position).is_some_and(|c| c.is_whitespace()) {
        *position += 1;
    }
}

fn read_string(text: &[char], position: &mut usize) -> Option<String> {
    if text.get(*position) != Some(&'"') {
        return None;
    }
    *position += 1;
    let mut value = String::new();
    while let Some(&c) = text.get(*position) {
        *position += 1;
        match c {
            '"' => return Some(value),
            '\\' if *position < text.len() => {
                let escaped = text[*position];
                *position += 1;
                value.push(match escaped {
                    'n' => '\n',
                    't' => '\t',
                    other => other,
                });
            }
            other => value.push(other),
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn steams_library_list_reads_with_escapes_and_any_key_case() {
        let text = "\"libraryfolders\"\n{\n\t\"0\"\n\t{\n\t\t\"path\"\t\t\"D:\\\\Steam\"\n\t\t\"apps\"\n\t\t{\n\t\t\t\"899770\"\t\t\"123\"\n\t\t}\n\t}\n}\n";
        let root = KeyValues::parse(text).unwrap();
        let folder = root.get("LibraryFolders").unwrap().get("0").unwrap();
        assert_eq!(folder.get("PATH").unwrap().value.as_deref(), Some(r"D:\Steam"));
        assert!(folder.get("apps").unwrap().get("899770").is_some());
        assert!(KeyValues::parse("\"unclosed\" {").is_none());
        assert!(KeyValues::parse("not keyvalues").is_none());
    }
}
