//! The CIL metadata a DLL carries, read as data through `dotscope`: its tables and heaps, and custom attributes decoded
//! from their blobs.

use crate::invalid;
use dotscope::CilAssemblyView;
use dotscope::metadata::tables::{
    AssemblyRaw, CustomAttributeRaw, ManifestResourceRaw, MemberRefRaw, MethodDefRaw, NestedClassRaw, RowReadable, TableId, TypeDefRaw, TypeRefRaw, TypeSpecRaw,
};
use std::fs;
use std::io::{self, Read};
use std::path::Path;

/// The most bytes a DLL read here may have. A mod is kilobytes to a few megabytes, and MelonLoader's generated assemblies
/// tens of megabytes at most; a larger file is refused unread rather than parsed.
pub(crate) const MAX_DLL_BYTES: u64 = 64 << 20;

/// A DLL's bytes, refused when it is larger than [`MAX_DLL_BYTES`].
pub(crate) fn read_dll(path: &Path) -> io::Result<Vec<u8>> {
    let mut bytes = Vec::new();
    fs::File::open(path)?.take(MAX_DLL_BYTES + 1).read_to_end(&mut bytes)?;
    if bytes.len() as u64 > MAX_DLL_BYTES {
        return Err(invalid(format!("{} is too large for a mod's DLL.", path.file_name().unwrap_or_default().to_string_lossy())));
    }
    Ok(bytes)
}

/// Runs a parse of a DLL someone else built. A panic in it is that DLL's problem: the caller reads it as a DLL that cannot
/// be read, and the process goes on. The panic hook has still reported it. An abort, or running out of stack, can't be caught.
pub(crate) fn contained<T>(parse: impl FnOnce() -> T) -> Option<T> {
    std::panic::catch_unwind(std::panic::AssertUnwindSafe(parse)).ok()
}

pub(crate) fn read_view(path: &Path) -> io::Result<CilAssemblyView> {
    let bytes = read_dll(path)?;
    let view = CilAssemblyView::from_mem(bytes).map_err(|_| not_assembly(path))?;
    one_assembly(&view, path)?;
    Ok(view)
}

pub(crate) fn not_assembly(path: &Path) -> io::Error {
    invalid(format!("{} is not a .NET assembly.", path.file_name().unwrap_or_default().to_string_lossy()))
}

pub(crate) fn one_assembly(view: &CilAssemblyView, path: &Path) -> io::Result<()> {
    if rows::<AssemblyRaw>(view)?.len() != 1 {
        return Err(not_assembly(path));
    }
    Ok(())
}

pub(crate) trait RawRows: RowReadable {
    fn read_rows(view: &CilAssemblyView) -> io::Result<Vec<Self>>;
    fn read_row(view: &CilAssemblyView, rid: u32) -> io::Result<Option<Self>>;
    fn count(view: &CilAssemblyView) -> io::Result<u32>;
}
macro_rules! raw_rows {
    ($($row:ty),+ $(,)?) => { $(impl RawRows for $row {
        fn read_rows(view: &CilAssemblyView) -> io::Result<Vec<Self>> {
            let tables = view.tables().ok_or_else(|| invalid("Missing metadata tables."))?;
            match tables.table::<Self>() { Some(table) => table.iter().map(|row| row.map_err(invalid)).collect(), None => Ok(Vec::new()) }
        }
        fn read_row(view: &CilAssemblyView, rid: u32) -> io::Result<Option<Self>> {
            let tables = view.tables().ok_or_else(|| invalid("Missing metadata tables."))?;
            match tables.table::<Self>() { Some(table) => table.get(rid).map_err(invalid), None => Ok(None) }
        }
        fn count(view: &CilAssemblyView) -> io::Result<u32> {
            let tables = view.tables().ok_or_else(|| invalid("Missing metadata tables."))?;
            Ok(tables.table::<Self>().map_or(0, |table| table.row_count))
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
pub(crate) fn rows<T: RawRows>(view: &CilAssemblyView) -> io::Result<Vec<T>> {
    T::read_rows(view)
}

/// Row `rid` of a table, read where it lies rather than by reading the table: a lookup per attribute or type stays
/// constant however large the DLL's tables are. None when the table has no such row.
pub(crate) fn row<T: RawRows>(view: &CilAssemblyView, rid: u32) -> io::Result<Option<T>> {
    T::read_row(view, rid)
}

/// The row of `rows` (a whole table, as [`rows`] reads it) whose ID is `rid`.
pub(crate) fn by_rid<T: HasRid>(rows: &[T], rid: u32) -> Option<&T> {
    rows.get(rid.checked_sub(1)? as usize).filter(|row| row.rid() == rid)
}

/// A table row's own ID, which [`by_rid`] checks.
pub(crate) trait HasRid {
    fn rid(&self) -> u32;
}
macro_rules! has_rid {
    ($($row:ty),+ $(,)?) => { $(impl HasRid for $row { fn rid(&self) -> u32 { self.rid } })+ };
}
has_rid!(TypeDefRaw);

/// The TypeDef that owns method `rid`: the last whose method list starts at or before it. A binary search over rows read
/// in place, since the lists are in order; a DLL whose lists are not finds some owner, whose attribute is then skipped.
pub(crate) fn method_owner(view: &CilAssemblyView, rid: u32) -> io::Result<Option<TypeDefRaw>> {
    let (mut low, mut high) = (1, TypeDefRaw::count(view)? + 1);
    while low < high {
        let middle = low + (high - low) / 2;
        let ty = row::<TypeDefRaw>(view, middle)?.ok_or_else(|| invalid("Missing TypeDef."))?;
        if ty.method_list <= rid {
            low = middle + 1;
        } else {
            high = middle;
        }
    }
    if low == 1 { Ok(None) } else { row::<TypeDefRaw>(view, low - 1) }
}

pub(crate) fn string(view: &CilAssemblyView, index: u32) -> io::Result<String> {
    view.strings()
        .ok_or_else(|| invalid("Missing string heap."))?
        .get(index as usize)
        .map(str::to_owned)
        .map_err(invalid)
}

pub(crate) fn blob(view: &CilAssemblyView, index: u32) -> io::Result<&[u8]> {
    view.blobs().ok_or_else(|| invalid("Missing blob heap."))?.get(index as usize).map_err(invalid)
}

#[derive(Clone, Debug)]
pub(crate) enum Argument {
    Text(Option<String>),
    Int(i32),
    Bool(bool),
    Array(Vec<Argument>),
}
impl Argument {
    pub(crate) fn text(&self) -> Option<&str> {
        match self {
            Self::Text(text) => text.as_deref(),
            _ => None,
        }
    }
    pub(crate) fn int(&self) -> Option<i32> {
        match self {
            Self::Int(value) => Some(*value),
            _ => None,
        }
    }
}

pub(crate) struct Cursor<'a>(pub(crate) &'a [u8]);
impl<'a> Cursor<'a> {
    fn take(&mut self, count: usize) -> io::Result<&'a [u8]> {
        let (head, tail) = self.0.split_at_checked(count).ok_or_else(|| invalid("Truncated metadata blob."))?;
        self.0 = tail;
        Ok(head)
    }
    pub(crate) fn byte(&mut self) -> io::Result<u8> {
        Ok(self.take(1)?[0])
    }
    pub(crate) fn compressed(&mut self) -> io::Result<u32> {
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

pub(crate) fn type_token(coded: u32) -> io::Result<(TableId, u32)> {
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

pub(crate) fn type_name(view: &CilAssemblyView, (table, rid): (TableId, u32)) -> io::Result<(String, String)> {
    let (space, name) = match table {
        TableId::TypeDef => {
            let row = row::<TypeDefRaw>(view, rid)?.ok_or_else(|| invalid("Missing TypeDef."))?;
            (row.type_namespace, row.type_name)
        }
        TableId::TypeRef => {
            let row = row::<TypeRefRaw>(view, rid)?.ok_or_else(|| invalid("Missing TypeRef."))?;
            (row.type_namespace, row.type_name)
        }
        _ => return Err(invalid("Unsupported attribute type.")),
    };
    Ok((string(view, space)?, string(view, name)?))
}

pub(crate) fn attribute(view: &CilAssemblyView, row: &CustomAttributeRaw) -> io::Result<(String, String, Vec<Argument>)> {
    let (parent, signature) = match row.constructor.tag {
        TableId::MemberRef => {
            let method = self::row::<MemberRefRaw>(view, row.constructor.row)?.ok_or_else(|| invalid("Missing MemberRef."))?;
            ((method.class.tag, method.class.row), method.signature)
        }
        TableId::MethodDef => {
            let method = self::row::<MethodDefRaw>(view, row.constructor.row)?.ok_or_else(|| invalid("Missing MethodDef."))?;
            let owner = method_owner(view, method.rid)?.ok_or_else(|| invalid("Missing method owner."))?;
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
