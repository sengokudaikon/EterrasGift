//! What MelonLoader's Latest.log actually observed. A previous build's failure is not evidence against a new DLL;
//! incomplete loading and failed Unity support leave individual mods' loading status unknown.

use super::assembly::LoaderVersion;
use std::collections::BTreeMap;
use std::fs;
use std::io;
use std::path::Path;
use std::time::Duration;

use phrases::phrases;

phrases! {
    ClosedEarly: "The game closed before MelonLoader finished loading mods."
    NoReason: "MelonLoader read it but did not load it, and its log gives no reason."
    NotFound: "MelonLoader did not find it in the Mods folder."
}

pub const LOG_LINES_PER_MOD: usize = 50;
const ASSEMBLY_LOADED: &str = "Melon Assembly loaded: '";
const HASH_LABEL: &str = "SHA256 Hash:";
const HASH: &str = "SHA256 Hash: '";
const ASSEMBLY: &str = "Assembly: ";

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MelonBanner {
    pub name: String,
    pub version: String,
    pub author: Option<String>,
    pub file: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MelonLogLine {
    pub time: Duration,
    pub source: Option<String>,
    pub text: String,
    pub is_error: bool,
}

impl MelonLogLine {
    /// Whether the line is tagged with the mod's name, which its tag may spell with underscores for spaces.
    pub fn is_from(&self, name: &str) -> bool {
        self.source.as_deref().is_some_and(|source| tag(source).eq(tag(name)))
    }
}

/// A mod's name as its tag reads: underscores as spaces, without the spaces at either end.
fn tag(name: &str) -> impl Iterator<Item = char> + '_ {
    name.trim_matches(|character: char| character.is_whitespace() || character == '_')
        .chars()
        .map(|character| if character == '_' { ' ' } else { character })
}

/// What finds one mod in MelonLoader's own lines, worked out once for all of them.
struct Mentions {
    /// The name as MelonLoader quotes it.
    quoted: String,
    /// The DLL's file name, lower-cased.
    file: String,
}

impl Mentions {
    fn new(name: &str, file: &str) -> Self {
        Self {
            quoted: format!("'{name}'"),
            file: file.to_lowercase(),
        }
    }

    /// One of MelonLoader's own lines (not a mod's) that names the mod or its file.
    fn in_line(&self, line: &MelonLogLine) -> bool {
        line.source.is_none() && (line.text.contains(&self.quoted) || names_file(&line.text, &self.file))
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ModLogRun {
    pub line: MelonLogLine,
    pub count: usize,
    pub last: Duration,
}

#[derive(Clone, Debug, Default)]
pub struct MelonLog {
    pub loader: Option<LoaderVersion>,
    pub found: Vec<String>,
    pub registered: Vec<MelonBanner>,
    pub lines: Vec<MelonLogLine>,
    pub reached_mods: bool,
    hashes: BTreeMap<String, String>,
}

impl MelonLog {
    /// Rust's Windows file reader shares read/write/delete, allowing MelonLoader to keep writing this log.
    pub fn read(path: &Path) -> io::Result<Self> {
        let bytes = fs::read(path)?;
        Ok(Self::parse(file_ops::text::strip_bom(&String::from_utf8_lossy(&bytes))))
    }

    pub fn parse(text: &str) -> Self {
        let mut log = Self::default();
        let text = text.replace("\r\n", "\n").replace(['\r', '\u{85}', '\u{2028}', '\u{2029}'], "\n");
        for raw in text.split('\n') {
            let Some(line) = timed(raw) else {
                if !raw.is_empty()
                    && let Some(last) = log.lines.last_mut()
                {
                    last.text.push('\n');
                    last.text.push_str(raw.trim_end());
                }
                continue;
            };
            log.lines.push(line);
            let line = log.lines.last().expect("the line was just appended");
            if line.source.is_some() {
                continue;
            }
            if log.loader.is_none()
                && let Some(version) = line.text.strip_prefix("MelonLoader v")
            {
                log.loader = LoaderVersion::parse(version.split(' ').next().unwrap_or_default());
            } else if let Some(file) = line.text.strip_prefix(ASSEMBLY_LOADED) {
                log.found.push(file.trim_end_matches('\'').rsplit(['/', '\\']).next().unwrap_or_default().to_owned());
            } else if let Some(hash) = line.text.strip_prefix(HASH) {
                if let Some(file) = log.found.last() {
                    log.hashes.insert(file.to_lowercase(), hash.trim_end_matches('\'').to_uppercase());
                }
            } else if line.text.starts_with(ASSEMBLY) {
                if let Some(banner) = banner(&log.lines) {
                    log.registered.push(banner);
                }
            } else if line.text.ends_with(" Mods loaded.") {
                log.reached_mods = true;
            }
        }
        log
    }

    pub fn support_failed(&self) -> bool {
        self.lines.iter().any(|line| line.is_error && line.text.starts_with("No Support Module Loaded"))
    }

    pub fn hash_of(&self, file: &str) -> Option<&str> {
        self.hashes.get(&file.to_lowercase()).map(String::as_str)
    }

    pub fn banner_for(&self, file: &str) -> Option<&MelonBanner> {
        self.registered.iter().find(|banner| same_file(&banner.file, file))
    }

    pub fn lines_for(&self, name: &str, file: &str) -> Vec<MelonLogLine> {
        let mentions = Mentions::new(name, file);
        self.lines
            .iter()
            .filter(|line| line.is_from(name) || (mentions.in_line(line) && !line.text.starts_with(ASSEMBLY)))
            .cloned()
            .collect()
    }

    pub fn runs_for(&self, name: &str, file: &str) -> Vec<ModLogRun> {
        let runs = runs(self.lines_for(name, file));
        let skip = runs.len().saturating_sub(LOG_LINES_PER_MOD);
        runs.into_iter().skip(skip).collect()
    }

    pub fn loaded_last_time(&self, file: &str, hash: &str) -> Option<bool> {
        (self.hash_of(file).is_some_and(|logged| logged.eq_ignore_ascii_case(hash)) && self.reached_mods && !self.support_failed()).then(|| self.banner_for(file).is_some())
    }

    pub fn problem_for(&self, name: &str, file: &str) -> Option<String> {
        if self.banner_for(file).is_some() {
            return None;
        }
        if !self.reached_mods {
            return Some(ClosedEarly.into());
        }
        let mentions = Mentions::new(name, file);
        let reason = self
            .lines
            .iter()
            .find(|line| mentions.in_line(line) && !line.text.starts_with(ASSEMBLY_LOADED) && !line.text.starts_with(HASH_LABEL));
        Some(
            reason
                .map(|line| line.text.clone())
                .unwrap_or_else(|| if self.found.iter().any(|found| same_file(found, file)) { NoReason } else { NotFound }.into()),
        )
    }
}

/// Fold consecutive identical text/error states before taking the final fifty runs, retaining both timestamps.
pub fn runs(lines: impl IntoIterator<Item = MelonLogLine>) -> Vec<ModLogRun> {
    let mut runs: Vec<ModLogRun> = Vec::new();
    for line in lines {
        if let Some(last) = runs.last_mut()
            && last.line.text == line.text
            && last.line.is_error == line.is_error
        {
            last.count += 1;
            last.last = line.time;
        } else {
            runs.push(ModLogRun { last: line.time, line, count: 1 });
        }
    }
    runs
}

/// File names compare without case, as Windows compares them.
fn same_file(first: &str, second: &str) -> bool {
    first.chars().flat_map(char::to_lowercase).eq(second.chars().flat_map(char::to_lowercase))
}

/// Whether `text` names `file` (lower-cased) as a whole name, whatever the case it is written in there.
fn names_file(text: &str, file: &str) -> bool {
    let name_part = |character: char| character.is_alphanumeric() || matches!(character, '_' | '-' | '.');
    text.char_indices().any(|(at, _)| {
        lowercase_prefix(&text[at..], file).is_some_and(|length| {
            text[..at].chars().next_back().is_none_or(|character| !name_part(character)) && text[at + length..].chars().next().is_none_or(|character| !name_part(character))
        })
    })
}

/// How long the start of `text` is that reads as `lower` once lower-cased; None when it does not start so.
fn lowercase_prefix(text: &str, lower: &str) -> Option<usize> {
    let mut wanted = lower.chars();
    for (at, character) in text.char_indices() {
        if wanted.as_str().is_empty() {
            return Some(at);
        }
        for lowered in character.to_lowercase() {
            if wanted.next() != Some(lowered) {
                return None;
            }
        }
    }
    wanted.as_str().is_empty().then_some(text.len())
}

fn timed(raw: &str) -> Option<MelonLogLine> {
    let prefix = raw.as_bytes().get(..14)?;
    if prefix[0] != b'[' || prefix[3] != b':' || prefix[6] != b':' || prefix[9] != b'.' || prefix[13] != b']' {
        return None;
    }
    let number = |start, end| -> Option<u64> {
        let mut value = 0;
        for &digit in &prefix[start..end] {
            if !digit.is_ascii_digit() {
                return None;
            }
            value = value * 10 + u64::from(digit - b'0');
        }
        Some(value)
    };
    let (hours, minutes, seconds, millis) = (number(1, 3)?, number(4, 6)?, number(7, 9)?, number(10, 13)?);
    if hours > 23 || minutes > 59 || seconds > 59 {
        return None;
    }
    let mut rest = raw.get(14..)?.trim_start();
    let mut source = None;
    let mut is_error = false;
    while rest.starts_with('[') {
        let Some(close) = rest.find(']').filter(|&close| close > 1) else { break };
        match &rest[1..close] {
            "ERROR" => is_error = true,
            "WARNING" | "WARN" | "DEBUG" | "MSG" => {}
            tag => {
                source.get_or_insert_with(|| tag.to_owned());
            }
        }
        rest = rest[close + 1..].trim_start();
    }
    Some(MelonLogLine {
        time: Duration::from_millis(((hours * 60 + minutes) * 60 + seconds) * 1000 + millis),
        source,
        text: rest.trim_end().into(),
        is_error,
    })
}

fn banner(lines: &[MelonLogLine]) -> Option<MelonBanner> {
    let file = lines.last()?.text.strip_prefix(ASSEMBLY)?.trim().to_owned();
    let mut author = None;
    for line in lines[..lines.len() - 1].iter().rev().take(5) {
        if line.text.starts_with("---") {
            break;
        }
        if let Some(name) = line.text.strip_prefix("by ") {
            author = Some(name.to_owned());
        } else if let Some(split) = line.text.rfind(" v").filter(|&split| split > 0) {
            return Some(MelonBanner {
                name: line.text[..split].into(),
                version: line.text[split + 2..].into(),
                author,
                file,
            });
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    const LOG: &str = "[19:40:01.158] MelonLoader v0.7.3 Open-Beta\n\
        [19:40:03.616] Melon Assembly loaded: '.\\Mods\\FlickerDreams.dll'\n\
        [19:40:03.616] SHA256 Hash: 'abC123'\n\
        [19:40:03.620] [ERROR] Failed to load Melon Assembly from '.\\Mods\\Broken.dll':\n\
        System.BadImageFormatException: Bad IL format.\n\
           at MelonLoader.MelonAssembly.LoadMelonAssembly(String path)\n\
        [19:40:04.703] ------------------------------\n\
        [19:40:04.703] Flicker Dreams v0.12.1\n\
        [19:40:04.703] by someone\n\
        [19:40:04.703] Assembly: FlickerDreams.dll\n\
        [19:40:04.704] 1 Mods loaded.\n\
        [19:40:15.360] [Flicker_Dreams] Reach x1.3333334\n\
        [19:41:10.364] [Flicker Dreams] Dreamslash refused\n\
        [19:41:12.000] [Flicker Dreams] [ERROR] Something threw\n\
        [19:41:13.000] [WARNING] [Il2CppInterop] Some warning";

    #[test]
    fn log_reads_banners_hashes_tags_and_multiline_failures() {
        let log = MelonLog::parse(&LOG.replace('\n', "\r\n"));
        assert_eq!(log.loader, LoaderVersion::parse("0.7.3"));
        assert!(log.reached_mods);
        assert_eq!(log.found, ["FlickerDreams.dll"]);
        assert_eq!(log.hash_of("FLICKERDREAMS.dll"), Some("ABC123"));
        let banner = log.banner_for("flickerdreams.dll").unwrap();
        assert_eq!(
            (banner.name.as_str(), banner.version.as_str(), banner.author.as_deref()),
            ("Flicker Dreams", "0.12.1", Some("someone"))
        );
        let lines = log.lines_for("Flicker Dreams", "FlickerDreams.dll");
        assert!(lines.iter().any(|line| line.text == "Reach x1.3333334"));
        assert!(lines.iter().any(|line| line.is_error && line.text == "Something threw"));
        assert_eq!(log.lines.last().unwrap().source.as_deref(), Some("Il2CppInterop"));
        assert!(log.problem_for("Broken", "broken.dll").unwrap().contains("Bad IL format.\nat MelonLoader"));
        assert_eq!(log.problem_for("Ken", "Ken.dll").as_deref(), Some("MelonLoader did not find it in the Mods folder."));
        assert!(log.lines_for("Dreams", "Dreams.dll").is_empty());
    }

    #[test]
    fn loading_evidence_is_for_this_exact_build_and_healthy_completed_support() {
        let log = MelonLog::parse(LOG);
        assert_eq!(log.loaded_last_time("FlickerDreams.dll", "abc123"), Some(true));
        assert_eq!(log.loaded_last_time("FlickerDreams.dll", "different"), None);
        let incomplete = MelonLog::parse(LOG.split("[19:40:04.703]").next().unwrap());
        assert_eq!(incomplete.loaded_last_time("FlickerDreams.dll", "abc123"), None);
        assert_eq!(
            incomplete.problem_for("Flicker Dreams", "FlickerDreams.dll").as_deref(),
            Some("The game closed before MelonLoader finished loading mods.")
        );
        let missing_banner = MelonLog::parse(&LOG.replace("Assembly: FlickerDreams.dll", "Assembly: Other.dll"));
        assert_eq!(missing_banner.loaded_last_time("FlickerDreams.dll", "abc123"), Some(false));
        assert_eq!(
            missing_banner.problem_for("Flicker Dreams", "FlickerDreams.dll").as_deref(),
            Some("MelonLoader read it but did not load it, and its log gives no reason.")
        );
        let unsupported = MelonLog::parse(&format!("{LOG}\n[19:41:14.000] [ERROR] No Support Module Loaded!"));
        assert!(unsupported.support_failed());
        assert_eq!(unsupported.loaded_last_time("FlickerDreams.dll", "abc123"), None);
    }

    #[test]
    fn repeated_lines_fold_before_the_last_fifty_are_taken() {
        let mut text = String::new();
        for index in 0..60 {
            text.push_str(&format!(
                "[19:41:{index:02}.000] [Flicker_Dreams] line {index}\n[19:41:{index:02}.001] [Flicker_Dreams] line {index}\n"
            ));
        }
        let runs = MelonLog::parse(&text).runs_for("Flicker Dreams", "FlickerDreams.dll");
        assert_eq!(runs.len(), 50);
        assert_eq!(runs[0].line.text, "line 10");
        assert!(runs.iter().all(|run| run.count == 2 && run.last - run.line.time == Duration::from_millis(1)));
        assert!(timed("[🎯:00:00.000] no").is_none());
    }

    #[test]
    fn log_can_be_read_while_a_writer_keeps_it_open() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("Latest.log");
        fs::write(&path, LOG).unwrap();
        let _writer = fs::OpenOptions::new().read(true).write(true).open(&path).unwrap();
        assert_eq!(MelonLog::read(&path).unwrap().registered.len(), 1);
    }
}
