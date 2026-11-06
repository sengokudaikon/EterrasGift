//! What a mod's or MelonLoader's own DLL says about itself, inspected as metadata only. No DLL is loaded or executed, and
//! a parse that panics on someone else's DLL stops there ([`contained`]).

use crate::cil::{Argument, Cursor, attribute, blob, by_rid, contained, not_assembly, one_assembly, read_dll, read_view, row, rows, string, type_name, type_token};
use crate::invalid;
use dotscope::CilAssemblyView;
use dotscope::metadata::tables::{AssemblyRaw, CustomAttributeRaw, ManifestResourceRaw, NestedClassRaw, TableId, TypeDefRaw, TypeSpecRaw};
use std::cmp::Ordering;
use std::collections::{BTreeSet, HashMap};
use std::fmt;
use std::io;
use std::path::Path;

/// Who made a game and what it is called, as Unity records them; MelonLoader matches mods against these.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GameIdentity {
    pub developer: String,
    pub product: String,
}

#[derive(Clone, Debug, Default)]
pub struct LoaderVersion {
    pub major: i32,
    pub minor: i32,
    pub patch: i32,
    pub prerelease: String,
}

impl LoaderVersion {
    /// An exact requirement is met only by the same prerelease spelling: beta.01 is not beta.1 here, though the two
    /// compare equal for a minimum.
    pub fn exactly_matches(&self, other: &Self) -> bool {
        (self.major, self.minor, self.patch, &self.prerelease) == (other.major, other.minor, other.patch, &other.prerelease)
    }
    pub fn new(major: i32, minor: i32, patch: i32) -> Self {
        Self {
            major,
            minor,
            patch,
            prerelease: String::new(),
        }
    }

    /// MelonLoader accepts one to four numeric components; its comparison ignores the fourth and build metadata.
    pub fn parse(text: &str) -> Option<Self> {
        let text = text.trim().trim_start_matches(['v', 'V']);
        let text = text.split('+').next()?;
        let (number, prerelease) = text.split_once('-').unwrap_or((text, ""));
        let parts: Vec<_> = number.split('.').collect();
        if parts.len() > 4 {
            return None;
        }
        let mut numbers = [0; 3];
        for (index, part) in parts.iter().enumerate() {
            if part.is_empty() || !part.bytes().all(|byte| byte.is_ascii_digit()) {
                return None;
            }
            let value: i32 = part.parse().ok()?;
            if index < 3 {
                numbers[index] = value;
            }
        }
        Some(Self {
            major: numbers[0],
            minor: numbers[1],
            patch: numbers[2],
            prerelease: prerelease.into(),
        })
    }
}

// Ordered collections must agree with comparison, even for numerically equal tags such as beta.01 and beta.1.
impl PartialEq for LoaderVersion {
    fn eq(&self, other: &Self) -> bool {
        self.cmp(other) == Ordering::Equal
    }
}
impl Eq for LoaderVersion {}

impl fmt::Display for LoaderVersion {
    fn fmt(&self, out: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(out, "{}.{}.{}", self.major, self.minor, self.patch)?;
        if !self.prerelease.is_empty() {
            write!(out, "-{}", self.prerelease)?;
        }
        Ok(())
    }
}

impl Ord for LoaderVersion {
    fn cmp(&self, other: &Self) -> Ordering {
        let order = (self.major, self.minor, self.patch).cmp(&(other.major, other.minor, other.patch));
        if order != Ordering::Equal || self.prerelease == other.prerelease {
            return order;
        }
        if self.prerelease.is_empty() {
            return Ordering::Greater;
        }
        if other.prerelease.is_empty() {
            return Ordering::Less;
        }
        let left: Vec<_> = self.prerelease.split('.').collect();
        let right: Vec<_> = other.prerelease.split('.').collect();
        let numeric = |part: &str| {
            (!part.is_empty() && part.bytes().all(|byte| byte.is_ascii_digit()))
                .then(|| part.parse::<u64>().ok())
                .flatten()
        };
        for (a, b) in left.iter().zip(&right) {
            let order = match (numeric(a), numeric(b)) {
                (Some(a), Some(b)) => a.cmp(&b),
                (Some(_), None) => Ordering::Less,
                (None, Some(_)) => Ordering::Greater,
                _ => a.encode_utf16().cmp(b.encode_utf16()),
            };
            if order != Ordering::Equal {
                return order;
            }
        }
        left.len().cmp(&right.len())
    }
}

impl PartialOrd for LoaderVersion {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum MelonKind {
    Mod,
    Plugin,
    #[default]
    Unknown,
    Unresolved,
}
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum MelonDomain {
    #[default]
    Universal,
    Mono,
    Il2Cpp,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MelonGame {
    pub developer: Option<String>,
    pub name: Option<String>,
}
impl MelonGame {
    pub fn fits(&self, game: &GameIdentity) -> bool {
        self.developer.as_deref().is_none_or(str::is_empty)
            || self.name.as_deref().is_none_or(str::is_empty)
            || (self.developer.as_deref() == Some(game.developer.as_str()) && self.name.as_deref() == Some(game.product.as_str()))
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LoaderRequirement {
    pub version: LoaderVersion,
    pub is_minimum: bool,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct MelonAssembly {
    pub name: String,
    pub version: String,
    pub author: String,
    pub download_link: Option<String>,
    pub kind: MelonKind,
    pub games: Vec<MelonGame>,
    pub domain: MelonDomain,
    pub runs_on_windows64: bool,
    pub required_loader: Option<LoaderRequirement>,
    pub description: Option<String>,
    pub project_version: Option<String>,
}

impl MelonAssembly {
    pub fn read(path: &Path) -> io::Result<Option<Self>> {
        contained(|| {
            let view = read_view(path)?;
            read_melon(&view)
        })
        .unwrap_or_else(|| Err(not_assembly(path)))
    }
}

/// An embedded resource to ask a DLL for, and the most bytes it may hold.
#[derive(Clone, Copy, Debug)]
pub struct EmbeddedResource {
    pub name: &'static str,
    pub max_length: usize,
}

/// What a mod's DLL says, from one parse of it: its MelonInfo, the resource asked for (read only from a MelonLoader mod)
/// and its strings. Errors keep their kind and words.
#[derive(Debug)]
pub struct AssemblyFacts {
    pub melon: Result<Option<MelonAssembly>, (io::ErrorKind, String)>,
    pub resource: Result<Option<Vec<u8>>, String>,
    pub strings: BTreeSet<String>,
}

pub fn read_facts(path: &Path, resource: EmbeddedResource) -> AssemblyFacts {
    contained(|| parse_facts(path, resource)).unwrap_or_else(|| AssemblyFacts {
        melon: Err((io::ErrorKind::InvalidData, not_assembly(path).to_string())),
        resource: Ok(None),
        strings: BTreeSet::new(),
    })
}

fn parse_facts(path: &Path, resource: EmbeddedResource) -> AssemblyFacts {
    let failed = |kind, text| AssemblyFacts {
        melon: Err((kind, text)),
        resource: Ok(None),
        strings: BTreeSet::new(),
    };
    let bytes = match read_dll(path) {
        Ok(bytes) => bytes,
        Err(error) => return failed(error.kind(), error.to_string()),
    };
    let Ok(view) = CilAssemblyView::from_mem(bytes) else {
        return failed(io::ErrorKind::InvalidData, not_assembly(path).to_string());
    };
    let melon = one_assembly(&view, path).and_then(|()| read_melon(&view));
    AssemblyFacts {
        resource: if matches!(melon, Ok(Some(_))) {
            embedded(&view, resource).map_err(|error| error.to_string())
        } else {
            Ok(None)
        },
        melon: melon.map_err(|error| (error.kind(), error.to_string())),
        strings: user_strings(&view),
    }
}

/// Ownership evidence, not executable inspection: all #US strings, including those in methods not currently called.
pub fn read_strings(path: &Path) -> BTreeSet<String> {
    let Ok(bytes) = read_dll(path) else {
        return BTreeSet::new();
    };
    contained(|| {
        let Ok(view) = CilAssemblyView::from_mem(bytes) else {
            return BTreeSet::new();
        };
        user_strings(&view)
    })
    .unwrap_or_default()
}

fn user_strings(view: &CilAssemblyView) -> BTreeSet<String> {
    view.userstrings()
        .map(|heap| heap.iter().map(|(_, text)| text.to_string_lossy()).collect())
        .unwrap_or_default()
}

/// Only embedded resources count. Linked resources are never followed. What the bytes mean is the caller's to check.
pub fn read_resource(path: &Path, resource: EmbeddedResource) -> io::Result<Option<Vec<u8>>> {
    let bytes = read_dll(path)?;
    contained(|| {
        let view = CilAssemblyView::from_mem(bytes).map_err(invalid)?;
        embedded(&view, resource)
    })
    .unwrap_or_else(|| Err(not_assembly(path)))
}

fn embedded(view: &CilAssemblyView, wanted: EmbeddedResource) -> io::Result<Option<Vec<u8>>> {
    for resource in rows::<ManifestResourceRaw>(view)? {
        if !resource.implementation.is_null() || string(view, resource.name)? != wanted.name {
            continue;
        }
        let header = view.cor20header();
        let directory = view.file().rva_to_offset(header.resource_rva as usize).map_err(invalid)?;
        let offset = resource.offset_field as usize;
        let size = header.resource_size as usize;
        if offset.checked_add(4).is_none_or(|end| end > size) {
            return Err(invalid("The resource lies outside the DLL's resources."));
        }
        let data = view.file().data_slice(directory, size).map_err(invalid)?;
        let length = u32::from_le_bytes(data[offset..offset + 4].try_into().unwrap()) as usize;
        if length > wanted.max_length || offset.checked_add(4 + length).is_none_or(|end| end > size) {
            return Err(invalid("The resource's length does not fit the DLL."));
        }
        return Ok(Some(data[offset + 4..offset + 4 + length].to_vec()));
    }
    Ok(None)
}

fn read_melon(view: &CilAssemblyView) -> io::Result<Option<MelonAssembly>> {
    let mut result = MelonAssembly {
        runs_on_windows64: true,
        ..Default::default()
    };
    let mut has_info = false;
    for row in rows::<CustomAttributeRaw>(view)? {
        if row.parent.tag != TableId::Assembly || row.parent.row != 1 {
            continue;
        }
        // Like System.Reflection.Metadata, an unreadable attribute is skipped, not guessed.
        let Ok((space, name, args)) = attribute(view, &row) else {
            continue;
        };
        if space == "System.Reflection" {
            if let [Argument::Text(Some(text))] = args.as_slice()
                && !text.is_empty()
            {
                match name.as_str() {
                    "AssemblyDescriptionAttribute" => result.description = Some(text.clone()),
                    "AssemblyInformationalVersionAttribute" => result.project_version = Some(text.split('+').next().unwrap_or(text).into()),
                    _ => {}
                }
            }
            continue;
        }
        match name.as_str() {
            "MelonInfoAttribute" | "MelonModInfoAttribute" | "MelonPluginInfoAttribute" if !has_info => {
                if args.len() < 4 {
                    continue;
                }
                let (Some(ty), Some(title)) = (args[0].text(), args[1].text()) else {
                    continue;
                };
                let (mut version, mut rest) = if let Some(text) = args[2].text() {
                    (text.to_owned(), 3)
                } else if let (Some(a), Some(b), Some(c)) = (args[2].int(), args.get(3).and_then(Argument::int), args.get(4).and_then(Argument::int)) {
                    (format!("{a}.{b}.{c}"), 5)
                } else {
                    continue;
                };
                if rest == 5
                    && args.len() >= 8
                    && let Some(identifier) = args[5].text()
                {
                    version.push_str(identifier);
                    rest = 6;
                }
                result.name = title.into();
                result.version = version;
                result.author = args.get(rest).and_then(Argument::text).unwrap_or_default().into();
                result.download_link = args.get(rest + 1).and_then(Argument::text).filter(|text| !text.is_empty()).map(str::to_owned);
                result.kind = match name.as_str() {
                    "MelonModInfoAttribute" => MelonKind::Mod,
                    "MelonPluginInfoAttribute" => MelonKind::Plugin,
                    _ => kind_of(view, ty)?,
                };
                has_info = true;
            }
            "MelonGameAttribute" | "MelonModGameAttribute" | "MelonPluginGameAttribute" if args.len() == 2 => result.games.push(MelonGame {
                developer: args[0].text().map(str::to_owned),
                name: args[1].text().map(str::to_owned),
            }),
            "MelonPlatformDomainAttribute" if args.len() == 1 => {
                result.domain = match args[0].int() {
                    Some(1) => MelonDomain::Mono,
                    Some(2) => MelonDomain::Il2Cpp,
                    _ => MelonDomain::Universal,
                }
            }
            "MelonPlatformAttribute" => {
                if let [Argument::Array(platforms)] = args.as_slice() {
                    result.runs_on_windows64 = platforms.is_empty() || platforms.iter().any(|platform| matches!(platform.int(), Some(0 | 2)));
                }
            }
            "VerifyLoaderVersionAttribute" => result.required_loader = requirement(&args),
            _ => {}
        }
    }
    Ok(has_info.then_some(result))
}

fn requirement(args: &[Argument]) -> Option<LoaderRequirement> {
    let version = if let Some(text) = args.first().and_then(Argument::text) {
        LoaderVersion::parse(text)?
    } else {
        LoaderVersion {
            major: args.first()?.int()?,
            minor: args.get(1)?.int()?,
            patch: args.get(2)?.int()?,
            prerelease: args.get(3).and_then(Argument::text).unwrap_or_default().into(),
        }
    };
    Some(LoaderRequirement {
        version,
        is_minimum: matches!(args.last(), Some(Argument::Bool(true))),
    })
}

fn kind_of(view: &CilAssemblyView, serialized: &str) -> io::Result<MelonKind> {
    let types = rows::<TypeDefRaw>(view)?;
    let parents: HashMap<_, _> = rows::<NestedClassRaw>(view)?.into_iter().map(|row| (row.nested_class, row.enclosing_class)).collect();
    let name = serialized.split(',').next().unwrap_or_default().trim();
    let mut current = None;
    for ty in &types {
        if full_name(view, &types, &parents, ty.rid, 0)? == name {
            current = Some(ty.rid);
            break;
        }
    }
    let Some(mut current) = current else {
        return Ok(MelonKind::Unresolved);
    };
    for _ in 0..32 {
        let Some(ty) = by_rid(&types, current) else {
            return Ok(MelonKind::Unresolved);
        };
        if ty.extends.is_null() {
            return Ok(MelonKind::Unknown);
        }
        let mut base = (ty.extends.tag, ty.extends.row);
        if base.0 == TableId::TypeSpec {
            let Some(spec) = row::<TypeSpecRaw>(view, base.1)? else {
                return Ok(MelonKind::Unresolved);
            };
            let mut bytes = Cursor(blob(view, spec.signature)?);
            if bytes.byte()? == 0x15 && matches!(bytes.byte()?, 0x11 | 0x12) {
                base = type_token(bytes.compressed()?)?;
            }
        }
        match base.0 {
            TableId::TypeDef => current = base.1,
            TableId::TypeRef => {
                let (space, name) = type_name(view, base)?;
                return Ok(match (space.as_str(), name.as_str()) {
                    ("MelonLoader", "MelonMod") => MelonKind::Mod,
                    ("MelonLoader", "MelonPlugin") => MelonKind::Plugin,
                    ("MelonLoader" | "System", _) => MelonKind::Unknown,
                    _ => MelonKind::Unresolved,
                });
            }
            _ => return Ok(MelonKind::Unresolved),
        }
    }
    Ok(MelonKind::Unresolved)
}

fn full_name(view: &CilAssemblyView, types: &[TypeDefRaw], parents: &HashMap<u32, u32>, rid: u32, depth: u32) -> io::Result<String> {
    if depth >= 64 {
        return Err(invalid("Cyclic or overly deep nested types."));
    }
    let ty = by_rid(types, rid).ok_or_else(|| invalid("Missing enclosing type."))?;
    let name = string(view, ty.type_name)?;
    if let Some(parent) = parents.get(&rid) {
        return Ok(format!("{}+{name}", full_name(view, types, parents, *parent, depth + 1)?));
    }
    let space = string(view, ty.type_namespace)?;
    Ok(if space.is_empty() { name } else { format!("{space}.{name}") })
}

pub(crate) fn loader_version(path: &Path) -> io::Result<LoaderVersion> {
    contained(|| parse_loader_version(path)).unwrap_or_else(|| Err(not_assembly(path)))
}

fn parse_loader_version(path: &Path) -> io::Result<LoaderVersion> {
    let view = read_view(path)?;
    for row in rows::<CustomAttributeRaw>(&view)? {
        if row.parent.tag == TableId::Assembly
            && let Ok((_, name, args)) = attribute(&view, &row)
            && name == "AssemblyInformationalVersionAttribute"
            && let Some(version) = args.first().and_then(Argument::text).and_then(LoaderVersion::parse)
        {
            return Ok(version);
        }
    }
    let definition = rows::<AssemblyRaw>(&view)?.remove(0);
    Ok(LoaderVersion::new(
        definition.major_version as i32,
        definition.minor_version as i32,
        definition.build_number as i32,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cil::MAX_DLL_BYTES;

    #[test]
    fn a_panic_in_a_parse_stops_at_the_parse() {
        assert_eq!(contained(|| 7), Some(7));
        assert_eq!(contained(|| -> i32 { panic!("fixture panic") }), None);
    }

    #[test]
    fn a_dll_larger_than_any_mod_is_refused_unread() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("Huge.dll");
        std::fs::File::create(&path).unwrap().set_len(MAX_DLL_BYTES + 1).unwrap();
        assert_eq!(read_dll(&path).unwrap_err().kind(), io::ErrorKind::InvalidData);
        assert_eq!(MelonAssembly::read(&path).unwrap_err().kind(), io::ErrorKind::InvalidData);
        assert!(read_facts(&path, EmbeddedResource { name: "x", max_length: 1 }).melon.is_err());
        assert!(read_strings(&path).is_empty());
    }
}
