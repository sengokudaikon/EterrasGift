//! Steam's text KeyValues format (libraryfolders.vdf, appmanifest_*.acf): quoted keys followed by a quoted value or a
//! braced block. Keys compare case-insensitively, as Steam's do. Comments and conditionals are not used by these files.

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct KeyValues {
    /// The text of a leaf; None for a block.
    pub value: Option<String>,
    pub children: Vec<(String, KeyValues)>,
}

impl KeyValues {
    /// The first child with this key, whatever its case. Steam's keys are ASCII.
    pub fn get(&self, key: &str) -> Option<&KeyValues> {
        self.children.iter().find(|(name, _)| name.eq_ignore_ascii_case(key)).map(|(_, child)| child)
    }

    /// Reads a whole file's text; None when it is not KeyValues.
    pub fn parse(text: &str) -> Option<KeyValues> {
        let mut cursor = Cursor { text, position: 0 };
        let mut root = KeyValues::default();
        root.read_block(&mut cursor, false).then_some(root)
    }

    fn read_block(&mut self, cursor: &mut Cursor, nested: bool) -> bool {
        loop {
            cursor.skip_space();
            let Some(next) = cursor.peek() else {
                return !nested;
            };
            if next == '}' {
                cursor.bump();
                return nested;
            }
            let Some(key) = cursor.read_string() else {
                return false;
            };
            cursor.skip_space();
            let child = match cursor.peek() {
                None => return false,
                Some('{') => {
                    cursor.bump();
                    let mut child = KeyValues::default();
                    if !child.read_block(cursor, true) {
                        return false;
                    }
                    child
                }
                Some(_) => match cursor.read_string() {
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

/// A place in the text, read a character at a time.
struct Cursor<'a> {
    text: &'a str,
    position: usize,
}

impl Cursor<'_> {
    fn peek(&self) -> Option<char> {
        self.text[self.position..].chars().next()
    }

    fn bump(&mut self) -> Option<char> {
        let next = self.peek()?;
        self.position += next.len_utf8();
        Some(next)
    }

    fn skip_space(&mut self) {
        while self.peek().is_some_and(char::is_whitespace) {
            self.bump();
        }
    }

    fn read_string(&mut self) -> Option<String> {
        if self.peek() != Some('"') {
            return None;
        }
        self.bump();
        let mut value = String::new();
        loop {
            match self.bump()? {
                '"' => return Some(value),
                '\\' => value.push(match self.bump()? {
                    'n' => '\n',
                    't' => '\t',
                    other => other,
                }),
                other => value.push(other),
            }
        }
    }
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
