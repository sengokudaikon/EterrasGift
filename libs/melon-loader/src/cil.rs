//! The CIL metadata a DLL carries, read as data through `dotscope`: its tables and heaps, and custom attributes decoded
//! from their blobs.

use crate::invalid;
use dotscope::CilAssemblyView;
use dotscope::metadata::tables::{
    AssemblyRaw, CustomAttributeRaw, ManifestResourceRaw, MemberRefRaw, MethodDefRaw, NestedClassRaw, RowReadable, TableId, TypeDefRaw, TypeRefRaw, TypeSpecRaw,
};
use std::fs;
use std::io;
use std::path::Path;

pub(crate) fn read_view(path: &Path) -> io::Result<CilAssemblyView> {
    let bytes = fs::read(path)?;
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
pub(crate) fn rows<T: RawRows>(view: &CilAssemblyView) -> io::Result<Vec<T>> {
    T::read_rows(view)
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

pub(crate) fn attribute(view: &CilAssemblyView, row: &CustomAttributeRaw) -> io::Result<(String, String, Vec<Argument>)> {
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
