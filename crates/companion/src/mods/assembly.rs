//! MelonLoader assembly facts, inspected as metadata only. No DLL is loaded or executed.

use super::invalid;
use crate::game::install::GameIdentity;
use dotscope::CilAssemblyView;
use dotscope::metadata::tables::{
    AssemblyRaw, CustomAttributeRaw, ManifestResourceRaw, MemberRefRaw, MethodDefRaw, NestedClassRaw, RowReadable, TableId, TypeDefRaw, TypeRefRaw, TypeSpecRaw,
};
use std::cmp::Ordering;
use std::collections::{BTreeSet, HashMap};
use std::fmt;
use std::fs;
use std::io;
use std::path::Path;

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
        let view = read_view(path)?;
        read_melon(&view)
    }

    pub fn obstacle(&self, game: &GameIdentity, loader: &LoaderVersion) -> Option<String> {
        if self.kind == MelonKind::Unknown {
            return Some("Its MelonInfo names a class that is neither a MelonMod nor a MelonPlugin, so MelonLoader will not load it.".into());
        }
        if !self.games.is_empty() && !self.games.iter().any(|declared| declared.fits(game)) {
            return Some(format!(
                "It is made for {} by {}, not {}.",
                self.games[0].name.as_deref().unwrap_or_default(),
                self.games[0].developer.as_deref().unwrap_or_default(),
                game.product
            ));
        }
        if self.domain == MelonDomain::Mono {
            return Some(format!("It is made for Mono games; {} is an Il2Cpp game.", game.product));
        }
        if !self.runs_on_windows64 {
            return Some("It is not made for 64-bit Windows.".into());
        }
        if let Some(required) = &self.required_loader
            && ((required.is_minimum && loader < &required.version) || (!required.is_minimum && !loader.exactly_matches(&required.version)))
        {
            return Some(if required.is_minimum {
                format!("It needs MelonLoader {} or newer; {loader} is installed.", required.version)
            } else {
                format!("It needs exactly MelonLoader {}; {loader} is installed.", required.version)
            });
        }
        None
    }
}

pub(super) fn read_view(path: &Path) -> io::Result<CilAssemblyView> {
    let bytes = fs::read(path)?;
    let view = CilAssemblyView::from_mem(bytes).map_err(|_| not_assembly(path))?;
    one_assembly(&view, path)?;
    Ok(view)
}

fn not_assembly(path: &Path) -> io::Error {
    invalid(format!("{} is not a .NET assembly.", path.file_name().unwrap_or_default().to_string_lossy()))
}

fn one_assembly(view: &CilAssemblyView, path: &Path) -> io::Result<()> {
    if rows::<AssemblyRaw>(view)?.len() != 1 {
        return Err(not_assembly(path));
    }
    Ok(())
}

/// What the Mods page reads from a mod's DLL, from one parse of it: its MelonInfo, the settings description it embeds
/// (read only from a MelonLoader mod) and its strings. Errors keep their kind and words.
#[derive(Debug)]
pub struct AssemblyFacts {
    pub melon: Result<Option<MelonAssembly>, (io::ErrorKind, String)>,
    pub settings: Result<Option<Vec<u8>>, String>,
    pub strings: BTreeSet<String>,
}

pub fn read_facts(path: &Path) -> AssemblyFacts {
    let failed = |kind, text| AssemblyFacts {
        melon: Err((kind, text)),
        settings: Ok(None),
        strings: BTreeSet::new(),
    };
    let bytes = match fs::read(path) {
        Ok(bytes) => bytes,
        Err(error) => return failed(error.kind(), error.to_string()),
    };
    let Ok(view) = CilAssemblyView::from_mem(bytes) else {
        return failed(io::ErrorKind::InvalidData, not_assembly(path).to_string());
    };
    let melon = one_assembly(&view, path).and_then(|()| read_melon(&view));
    AssemblyFacts {
        settings: if matches!(melon, Ok(Some(_))) {
            settings_resource(&view).map_err(|error| error.to_string())
        } else {
            Ok(None)
        },
        melon: melon.map_err(|error| (error.kind(), error.to_string())),
        strings: user_strings(&view),
    }
}

pub(super) trait RawRows: RowReadable {
    fn read_rows(view: &CilAssemblyView) -> io::Result<Vec<Self>>;
}
macro_rules! raw_rows {
    ($($row:ty),+ $(,)?) => { $(impl RawRows for $row {
        fn read_rows(view: &CilAssemblyView) -> io::Result<Vec<Self>> {
            let tables = view.tables().ok_or_else(|| invalid("Missing metadata tables."))?;
            match tables.table::<Self>() { Some(table) => table.iter().map(|row| row.map_err(invalid)).collect(), None => Ok(Vec::new()) }
        }
    })+ };
}
raw_rows!(
    AssemblyRaw,
    CustomAttributeRaw,
    MemberRefRaw,
    MethodDefRaw,
    NestedClassRaw,
    TypeDefRaw,
    TypeRefRaw,
    TypeSpecRaw,
    ManifestResourceRaw
);
pub(super) fn rows<T: RawRows>(view: &CilAssemblyView) -> io::Result<Vec<T>> {
    T::read_rows(view)
}

fn string(view: &CilAssemblyView, index: u32) -> io::Result<String> {
    view.strings()
        .ok_or_else(|| invalid("Missing string heap."))?
        .get(index as usize)
        .map(str::to_owned)
        .map_err(invalid)
}

fn blob(view: &CilAssemblyView, index: u32) -> io::Result<&[u8]> {
    view.blobs().ok_or_else(|| invalid("Missing blob heap."))?.get(index as usize).map_err(invalid)
}

/// Ownership evidence, not executable inspection: all #US strings, including those in methods not currently called.
pub fn read_strings(path: &Path) -> BTreeSet<String> {
    let Ok(bytes) = fs::read(path) else {
        return BTreeSet::new();
    };
    let Ok(view) = CilAssemblyView::from_mem(bytes) else {
        return BTreeSet::new();
    };
    user_strings(&view)
}

fn user_strings(view: &CilAssemblyView) -> BTreeSet<String> {
    view.userstrings()
        .map(|heap| heap.iter().map(|(_, text)| text.to_string_lossy()).collect())
        .unwrap_or_default()
}

/// Only embedded resources count. Linked resources are never followed. The description parser owns JSON validation.
pub fn read_settings_resource(path: &Path) -> io::Result<Option<Vec<u8>>> {
    let bytes = fs::read(path)?;
    let view = CilAssemblyView::from_mem(bytes).map_err(invalid)?;
    settings_resource(&view)
}

fn settings_resource(view: &CilAssemblyView) -> io::Result<Option<Vec<u8>>> {
    for resource in rows::<ManifestResourceRaw>(view)? {
        if !resource.implementation.is_null() || string(view, resource.name)? != "ModSettings.json" {
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
        if length > 1 << 20 || offset.checked_add(4 + length).is_none_or(|end| end > size) {
            return Err(invalid("The resource's length does not fit the DLL."));
        }
        return Ok(Some(data[offset + 4..offset + 4 + length].to_vec()));
    }
    Ok(None)
}

#[derive(Clone, Debug)]
enum Argument {
    Text(Option<String>),
    Int(i32),
    Bool(bool),
    Array(Vec<Argument>),
}
impl Argument {
    fn text(&self) -> Option<&str> {
        match self {
            Self::Text(text) => text.as_deref(),
            _ => None,
        }
    }
    fn int(&self) -> Option<i32> {
        match self {
            Self::Int(value) => Some(*value),
            _ => None,
        }
    }
}

struct Cursor<'a>(&'a [u8]);
impl<'a> Cursor<'a> {
    fn take(&mut self, count: usize) -> io::Result<&'a [u8]> {
        let (head, tail) = self.0.split_at_checked(count).ok_or_else(|| invalid("Truncated metadata blob."))?;
        self.0 = tail;
        Ok(head)
    }
    fn byte(&mut self) -> io::Result<u8> {
        Ok(self.take(1)?[0])
    }
    fn compressed(&mut self) -> io::Result<u32> {
        let first = self.byte()?;
        match first {
            0..=0x7f => Ok(u32::from(first)),
            0x80..=0xbf => Ok((u32::from(first & 0x3f) << 8) | u32::from(self.byte()?)),
            0xc0..=0xdf => {
                let rest = self.take(3)?;
                Ok((u32::from(first & 0x1f) << 24) | (u32::from(rest[0]) << 16) | (u32::from(rest[1]) << 8) | u32::from(rest[2]))
            }
            _ => Err(invalid("Invalid compressed metadata integer.")),
        }
    }
    fn text(&mut self) -> io::Result<Option<String>> {
        if self.0.first() == Some(&0xff) {
            self.byte()?;
            return Ok(None);
        }
        let length = self.compressed()? as usize;
        String::from_utf8(self.take(length)?.to_vec()).map(Some).map_err(invalid)
    }
}

#[derive(Clone)]
enum ArgType {
    Text,
    Int,
    Bool,
    Array(Box<ArgType>),
}

fn signature_type(cursor: &mut Cursor<'_>, view: &CilAssemblyView, depth: u8) -> io::Result<ArgType> {
    if depth > 32 {
        return Err(invalid("Attribute argument nesting is too deep."));
    }
    match cursor.byte()? {
        0x02 => Ok(ArgType::Bool),
        0x08 => Ok(ArgType::Int),
        0x0e => Ok(ArgType::Text),
        0x11 | 0x12 => {
            let index = cursor.compressed()?;
            let name = type_name(view, type_token(index)?)?;
            if name == ("System".into(), "Type".into()) {
                Ok(ArgType::Text)
            } else {
                Ok(ArgType::Int)
            }
        }
        0x1d => Ok(ArgType::Array(Box::new(signature_type(cursor, view, depth + 1)?))),
        _ => Err(invalid("Unsupported attribute argument type.")),
    }
}

fn argument(cursor: &mut Cursor<'_>, kind: &ArgType) -> io::Result<Argument> {
    match kind {
        ArgType::Text => cursor.text().map(Argument::Text),
        ArgType::Int => Ok(Argument::Int(i32::from_le_bytes(cursor.take(4)?.try_into().unwrap()))),
        ArgType::Bool => Ok(Argument::Bool(cursor.byte()? != 0)),
        ArgType::Array(element) => {
            let count = i32::from_le_bytes(cursor.take(4)?.try_into().unwrap());
            if count == -1 {
                return Ok(Argument::Array(Vec::new()));
            }
            if count < 0 || count as usize > cursor.0.len() {
                return Err(invalid("Invalid attribute array length."));
            }
            (0..count).map(|_| argument(cursor, element)).collect::<io::Result<Vec<_>>>().map(Argument::Array)
        }
    }
}

fn type_token(coded: u32) -> io::Result<(TableId, u32)> {
    Ok((
        match coded & 3 {
            0 => TableId::TypeDef,
            1 => TableId::TypeRef,
            2 => TableId::TypeSpec,
            _ => return Err(invalid("Invalid type token.")),
        },
        coded >> 2,
    ))
}

fn type_name(view: &CilAssemblyView, (table, rid): (TableId, u32)) -> io::Result<(String, String)> {
    let (space, name) = match table {
        TableId::TypeDef => {
            let row = rows::<TypeDefRaw>(view)?
                .into_iter()
                .find(|row| row.rid == rid)
                .ok_or_else(|| invalid("Missing TypeDef."))?;
            (row.type_namespace, row.type_name)
        }
        TableId::TypeRef => {
            let row = rows::<TypeRefRaw>(view)?
                .into_iter()
                .find(|row| row.rid == rid)
                .ok_or_else(|| invalid("Missing TypeRef."))?;
            (row.type_namespace, row.type_name)
        }
        _ => return Err(invalid("Unsupported attribute type.")),
    };
    Ok((string(view, space)?, string(view, name)?))
}

fn attribute(view: &CilAssemblyView, row: &CustomAttributeRaw) -> io::Result<(String, String, Vec<Argument>)> {
    let (parent, signature) = match row.constructor.tag {
        TableId::MemberRef => {
            let method = rows::<MemberRefRaw>(view)?
                .into_iter()
                .find(|item| item.rid == row.constructor.row)
                .ok_or_else(|| invalid("Missing MemberRef."))?;
            ((method.class.tag, method.class.row), method.signature)
        }
        TableId::MethodDef => {
            let method = rows::<MethodDefRaw>(view)?
                .into_iter()
                .find(|item| item.rid == row.constructor.row)
                .ok_or_else(|| invalid("Missing MethodDef."))?;
            let owner = rows::<TypeDefRaw>(view)?
                .into_iter()
                .rfind(|item| item.method_list <= method.rid)
                .ok_or_else(|| invalid("Missing method owner."))?;
            ((TableId::TypeDef, owner.rid), method.signature)
        }
        _ => return Err(invalid("Unsupported attribute constructor.")),
    };
    let (space, name) = type_name(view, parent)?;
    if space != "MelonLoader" && space != "System.Reflection" {
        return Err(invalid("Unrelated attribute."));
    }
    let mut signature = Cursor(blob(view, signature)?);
    let convention = signature.byte()?;
    if convention & 0x10 != 0 {
        signature.compressed()?;
    }
    let count = signature.compressed()? as usize;
    if signature.byte()? != 1 || count > signature.0.len() {
        return Err(invalid("Invalid attribute signature."));
    }
    let kinds = (0..count).map(|_| signature_type(&mut signature, view, 0)).collect::<io::Result<Vec<_>>>()?;
    let mut value = Cursor(blob(view, row.value)?);
    if value.take(2)? != [1, 0] {
        return Err(invalid("Invalid attribute prolog."));
    }
    let args = kinds.iter().map(|kind| argument(&mut value, kind)).collect::<io::Result<Vec<_>>>()?;
    // Named arguments do not contribute to these facts, but a truncated argument invalidates the whole attribute.
    let named = u16::from_le_bytes(value.take(2)?.try_into().unwrap());
    for _ in 0..named {
        if !matches!(value.byte()?, 0x53 | 0x54) {
            return Err(invalid("Invalid named attribute argument."));
        }
        let kind = named_type(&mut value, 0)?;
        value.text()?.ok_or_else(|| invalid("Missing named attribute argument name."))?;
        skip_named(&mut value, &kind, 0)?;
    }
    Ok((space, name, args))
}

enum NamedType {
    Bytes(usize),
    Text,
    Object,
    Array(Box<NamedType>),
}
fn named_type(cursor: &mut Cursor<'_>, depth: u8) -> io::Result<NamedType> {
    if depth > 32 {
        return Err(invalid("Attribute argument nesting is too deep."));
    }
    Ok(match cursor.byte()? {
        0x02 | 0x04 | 0x05 => NamedType::Bytes(1),
        0x03 | 0x06 | 0x07 => NamedType::Bytes(2),
        0x08 | 0x09 | 0x0c => NamedType::Bytes(4),
        0x0a | 0x0b | 0x0d => NamedType::Bytes(8),
        0x0e | 0x50 => NamedType::Text,
        0x51 => NamedType::Object,
        0x55 => {
            cursor.text()?.ok_or_else(|| invalid("Missing attribute enum type."))?;
            NamedType::Bytes(4)
        }
        0x1d => NamedType::Array(Box::new(named_type(cursor, depth + 1)?)),
        _ => return Err(invalid("Unsupported named attribute argument type.")),
    })
}
fn skip_named(cursor: &mut Cursor<'_>, kind: &NamedType, depth: u8) -> io::Result<()> {
    if depth > 32 {
        return Err(invalid("Attribute argument nesting is too deep."));
    }
    match kind {
        NamedType::Bytes(length) => {
            cursor.take(*length)?;
        }
        NamedType::Text => {
            cursor.text()?;
        }
        NamedType::Object => {
            let kind = named_type(cursor, depth + 1)?;
            skip_named(cursor, &kind, depth + 1)?;
        }
        NamedType::Array(element) => {
            let count = i32::from_le_bytes(cursor.take(4)?.try_into().unwrap());
            if count == -1 {
                return Ok(());
            }
            if count < 0 || count as usize > cursor.0.len() {
                return Err(invalid("Invalid named attribute array length."));
            }
            for _ in 0..count {
                skip_named(cursor, element, depth + 1)?;
            }
        }
    }
    Ok(())
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
        let Some(ty) = types.iter().find(|ty| ty.rid == current) else {
            return Ok(MelonKind::Unresolved);
        };
        if ty.extends.is_null() {
            return Ok(MelonKind::Unknown);
        }
        let mut base = (ty.extends.tag, ty.extends.row);
        if base.0 == TableId::TypeSpec {
            let Some(spec) = rows::<TypeSpecRaw>(view)?.into_iter().find(|row| row.rid == base.1) else {
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
    let ty = types.iter().find(|ty| ty.rid == rid).ok_or_else(|| invalid("Missing enclosing type."))?;
    let name = string(view, ty.type_name)?;
    if let Some(parent) = parents.get(&rid) {
        return Ok(format!("{}+{name}", full_name(view, types, parents, *parent, depth + 1)?));
    }
    let space = string(view, ty.type_namespace)?;
    Ok(if space.is_empty() { name } else { format!("{space}.{name}") })
}

pub(super) fn loader_version(path: &Path) -> io::Result<LoaderVersion> {
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
