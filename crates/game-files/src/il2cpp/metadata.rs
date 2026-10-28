//! `global-metadata.dat`, format 39 (Unity 6000.3–6000.4): names, type and field definitions, generic parameters,
//! images and custom attribute data. Indices into the type definitions, generic containers, the binary's type table and
//! the parameters are as wide as their tables need (1, 2 or 4 bytes); every other index is 4 bytes in this format.
//! Each record size is checked against its section, so a layout this reader has wrong is refused, not misread.
//! Layouts follow Cpp2IL's LibCpp2IL (MIT), the only reader known to handle this format.

use crate::error::{Error, Result};
use crate::reader::{Endian, Reader};

const MAGIC: u32 = 0xFAB1_1BAF;
pub(crate) const VERSION: i32 = 39;

/// The sections, in header order (format 39 has 31, each offset, size and count).
const SECTIONS: [&str; 31] = [
    "stringLiteral",
    "stringLiteralData",
    "string",
    "events",
    "properties",
    "methods",
    "parameterDefaultValues",
    "fieldDefaultValues",
    "fieldAndParameterDefaultValueData",
    "fieldMarshaledSizes",
    "parameters",
    "fields",
    "genericParameters",
    "genericParameterConstraints",
    "genericContainers",
    "nestedTypes",
    "interfaces",
    "vtableMethods",
    "interfaceOffsets",
    "typeDefinitions",
    "images",
    "assemblies",
    "fieldRefs",
    "referencedAssemblies",
    "attributeData",
    "attributeDataRange",
    "unresolvedVirtualCallParameterTypes",
    "unresolvedVirtualCallParameterRanges",
    "windowsRuntimeTypeNames",
    "windowsRuntimeStrings",
    "exportedTypeDefinitions",
];

#[derive(Clone, Copy, Debug)]
struct Section {
    offset: usize,
    size: usize,
    count: usize,
}

/// `TypeAttributes` bits used here.
pub(crate) mod type_attributes {
    pub(crate) const INTERFACE: u32 = 0x20;
    pub(crate) const ABSTRACT: u32 = 0x80;
    pub(crate) const SERIALIZABLE: u32 = 0x2000;
}

#[derive(Clone, Debug)]
pub(crate) struct TypeDefinition {
    pub(crate) name: String,
    pub(crate) namespace: String,
    /// Indices into the binary's type table; `None` where the type has none.
    pub(crate) declaring_type: Option<u32>,
    pub(crate) parent: Option<u32>,
    pub(crate) has_generic_parameters: bool,
    pub(crate) flags: u32,
    pub(crate) first_field: u32,
    pub(crate) field_count: u16,
    pub(crate) is_value_type: bool,
    pub(crate) is_enum: bool,
    pub(crate) token: u32,
}

#[derive(Clone, Debug)]
pub(crate) struct FieldDefinition {
    pub(crate) name: String,
    pub(crate) type_index: u32,
    pub(crate) token: u32,
}

#[derive(Clone, Debug)]
pub(crate) struct Image {
    pub(crate) name: String,
    pub(crate) first_type: u32,
    pub(crate) type_count: u32,
    attribute_start: usize,
    attribute_count: usize,
}

/// What the file says, parsed. Of its bytes only the two sections read later are kept, attribute data and default
/// values, so the file is never held whole.
pub(crate) struct Metadata {
    pub(crate) types: Vec<TypeDefinition>,
    pub(crate) fields: Vec<FieldDefinition>,
    /// Each method's declaring type, all a custom attribute needs of its constructor.
    method_owners: Vec<u32>,
    /// Each generic parameter's position in its owner.
    generic_positions: Vec<u16>,
    pub(crate) images: Vec<Image>,
    attribute_data: Vec<u8>,
    attribute_ranges: Vec<(u32, u32)>,
    /// Literal fields' default values: field index → (type index, offset into the default value data).
    defaults: std::collections::HashMap<u32, (u32, usize)>,
    default_data: Vec<u8>,
}

fn width(count: usize) -> usize {
    match count {
        0..=0xFF => 1,
        0x100..=0xFFFF => 2,
        _ => 4,
    }
}

/// A variable-width index; the width's all-ones value is "none".
fn index(reader: &mut Reader<'_>, width: usize) -> Result<Option<u32>> {
    Ok(match width {
        1 => Some(reader.u8()?).filter(|&v| v != u8::MAX).map(u32::from),
        2 => Some(reader.u16()?).filter(|&v| v != u16::MAX).map(u32::from),
        _ => u32::try_from(reader.i32()?).ok(),
    })
}

impl Metadata {
    /// Reads the file a section at a time, only the sections it uses, never more than one large section and the names
    /// at once.
    pub(crate) fn open(path: &std::path::Path) -> Result<Metadata> {
        let file = std::fs::File::open(path).map_err(|e| Error::io(path, e))?;
        let length = file.metadata().map_err(|e| Error::io(path, e))?.len();
        let length = usize::try_from(length).map_err(|_| Error::unsupported("global-metadata.dat too large"))?;
        Self::read(length, |offset, size| {
            let mut out = vec![0; size];
            crate::reader::read_at(&file, offset as u64, &mut out).map_err(|e| Error::io(path, e))?;
            Ok(out)
        })
    }

    #[cfg(test)]
    pub(crate) fn parse(bytes: Vec<u8>) -> Result<Metadata> {
        Self::read(bytes.len(), |offset, size| Ok(bytes[offset..offset + size].to_vec()))
    }

    /// The metadata of a file `length` long, whose bytes at an offset `read` gives (always within the file).
    fn read(length: usize, read: impl Fn(usize, usize) -> Result<Vec<u8>>) -> Result<Metadata> {
        let header = 8 + 12 * SECTIONS.len();
        if length < header {
            return Err(Error::malformed("not IL2CPP metadata"));
        }
        let header = read(0, header)?;
        let mut reader = Reader::new(&header, Endian::Little);
        if reader.u32()? != MAGIC {
            return Err(Error::malformed("not IL2CPP metadata"));
        }
        let version = reader.i32()?;
        if version != VERSION {
            return Err(Error::unsupported(format!("IL2CPP metadata version {version} (this reader knows {VERSION})")));
        }
        let mut sections = std::collections::HashMap::new();
        for name in SECTIONS {
            let (offset, size, count) = (reader.i32()?, reader.i32()?, reader.i32()?);
            let (Ok(offset), Ok(size), Ok(count)) = (usize::try_from(offset), usize::try_from(size), usize::try_from(count)) else {
                return Err(Error::malformed(format!("section {name} has a negative field")));
            };
            if offset.checked_add(size).is_none_or(|end| end > length) {
                return Err(Error::malformed(format!("section {name} runs past the end")));
            }
            sections.insert(name, Section { offset, size, count });
        }
        let section = |name: &str| sections[name];
        let interface_offsets = section("interfaceOffsets");
        // The type table is in the binary; its index width shows in interface offsets (a type index and an int each).
        let type_width = interface_offsets.size.checked_div(interface_offsets.count).and_then(|w| w.checked_sub(4)).unwrap_or(4);
        if ![1, 2, 4].contains(&type_width) {
            return Err(Error::malformed(format!("type index width {type_width}")));
        }
        let definition_width = width(section("typeDefinitions").count);
        let container_width = width(section("genericContainers").count);
        let parameter_width = width(section("parameters").count);

        let records = |name: &str, record: usize| -> Result<(Section, Vec<u8>)> {
            let s = section(name);
            if s.size != s.count * record {
                return Err(Error::unsupported(format!(
                    "{name} holds {} bytes for {} records of {record}: a layout this reader does not know",
                    s.size, s.count
                )));
            }
            Ok((s, read(s.offset, s.size)?))
        };
        let kept = |s: Section| read(s.offset, s.size);
        let strings = kept(section("string"))?;
        let text = |index: i32| -> Result<String> {
            let at = usize::try_from(index).map_err(|_| Error::malformed("negative string index"))?;
            let mut r = Reader::new(&strings, Endian::Little);
            r.seek(at)?;
            r.cstring()
        };

        let (s, data) = records("typeDefinitions", 8 + 3 * type_width + container_width + 4 + 32 + 16 + 8)?;
        let mut r = Reader::new(&data, Endian::Little);
        let mut types = Vec::with_capacity(s.count);
        for _ in 0..s.count {
            let (name, namespace) = (text(r.i32()?)?, text(r.i32()?)?);
            let _byval_type = index(&mut r, type_width)?;
            let (declaring_type, parent) = (index(&mut r, type_width)?, index(&mut r, type_width)?);
            let generic_container = index(&mut r, container_width)?;
            let flags = r.u32()?;
            let first_field = r.i32()?;
            r.take(7 * 4)?;
            let _method_count = r.u16()?;
            let _property_count = r.u16()?;
            let field_count = r.u16()?;
            r.take(5 * 2)?;
            let bits = r.u32()?;
            let token = r.u32()?;
            types.push(TypeDefinition {
                name,
                namespace,
                declaring_type,
                parent,
                has_generic_parameters: generic_container.is_some(),
                flags,
                first_field: u32::try_from(first_field).unwrap_or(u32::MAX),
                field_count,
                is_value_type: bits & 1 != 0,
                is_enum: bits & 2 != 0,
                token,
            });
        }

        drop(data);
        let (s, data) = records("fields", 8 + type_width)?;
        let mut r = Reader::new(&data, Endian::Little);
        let mut fields = Vec::with_capacity(s.count);
        for _ in 0..s.count {
            let name = text(r.i32()?)?;
            let type_index = index(&mut r, type_width)?.ok_or_else(|| Error::malformed(format!("field {name} has no type")))?;
            fields.push(FieldDefinition {
                name,
                type_index,
                token: r.u32()?,
            });
        }
        for t in &types {
            if t.field_count > 0 && (t.first_field as usize).checked_add(t.field_count.into()).is_none_or(|end| end > fields.len()) {
                return Err(Error::malformed(format!("type {} has fields past the table", t.name)));
            }
        }

        drop(data);
        let (s, data) = records("methods", 4 + definition_width + type_width + 4 + parameter_width + container_width + 4 + 8)?;
        let mut r = Reader::new(&data, Endian::Little);
        let mut method_owners = Vec::with_capacity(s.count);
        for _ in 0..s.count {
            r.i32()?;
            method_owners.push(index(&mut r, definition_width)?.unwrap_or(u32::MAX));
            r.take(type_width + 4 + parameter_width + container_width + 4 + 8)?;
        }

        drop(data);
        let (s, data) = records("genericParameters", container_width + 12)?;
        let mut r = Reader::new(&data, Endian::Little);
        let mut generic_positions = Vec::with_capacity(s.count);
        for _ in 0..s.count {
            r.take(container_width + 8)?;
            generic_positions.push(r.u16()?);
            r.u16()?;
        }

        drop(data);
        let (s, data) = records("images", 8 + 2 * definition_width + 4 + 4 + 4 + 4 + 8)?;
        let mut r = Reader::new(&data, Endian::Little);
        let mut images = Vec::with_capacity(s.count);
        for _ in 0..s.count {
            let name = text(r.i32()?)?;
            r.i32()?;
            let first_type = index(&mut r, definition_width)?.unwrap_or(0);
            let type_count = r.u32()?;
            r.take(definition_width + 4 + 4 + 4)?;
            let (attribute_start, attribute_count) = (r.i32()?, r.u32()?);
            images.push(Image {
                name,
                first_type,
                type_count,
                attribute_start: usize::try_from(attribute_start).unwrap_or(0),
                attribute_count: attribute_count as usize,
            });
        }

        drop(data);
        let (s, data) = records("attributeDataRange", 8)?;
        let mut r = Reader::new(&data, Endian::Little);
        let mut attribute_ranges = Vec::with_capacity(s.count);
        for _ in 0..s.count {
            attribute_ranges.push((r.u32()?, r.u32()?));
        }
        let attribute_data = kept(section("attributeData"))?;

        // Field and data indices are 4 bytes in this format.
        drop(data);
        let (s, data) = records("fieldDefaultValues", 8 + type_width)?;
        let mut r = Reader::new(&data, Endian::Little);
        let mut defaults = std::collections::HashMap::with_capacity(s.count);
        for _ in 0..s.count {
            let field = r.u32()?;
            let type_index = index(&mut r, type_width)?.ok_or_else(|| Error::malformed("default value without a type"))?;
            let data = r.i32()?;
            if let Ok(data) = usize::try_from(data) {
                defaults.insert(field, (type_index, data));
            }
        }
        let default_data = kept(section("fieldAndParameterDefaultValueData"))?;
        Ok(Metadata {
            defaults,
            default_data,
            types,
            fields,
            method_owners,
            generic_positions,
            images,
            attribute_data,
            attribute_ranges,
        })
    }

    pub(crate) fn type_definition(&self, index: u32) -> Result<&TypeDefinition> {
        self.types
            .get(index as usize)
            .ok_or_else(|| Error::malformed(format!("type definition {index} out of range")))
    }

    pub(crate) fn fields_of(&self, definition: &TypeDefinition) -> &[FieldDefinition] {
        let start = definition.first_field as usize;
        if definition.field_count == 0 {
            &[]
        } else {
            &self.fields[start..start + usize::from(definition.field_count)]
        }
    }

    /// The global index of a type definition's field.
    pub(crate) fn field_index(&self, definition: &TypeDefinition, name: &str) -> Option<u32> {
        let found = self.fields_of(definition).iter().position(|f| f.name == name)?;
        Some(definition.first_field + found as u32)
    }

    /// A literal field's default: the type index it was written as, and the value as a compressed signed integer
    /// (how format 29+ stores `Int32` constants).
    pub(crate) fn int_default(&self, field: u32) -> Result<(u32, i32)> {
        let (type_index, offset) = *self.defaults.get(&field).ok_or_else(|| Error::data(format!("field {field} has no default value")))?;
        if offset >= self.default_data.len() {
            return Err(Error::malformed(format!("default value of field {field} out of range")));
        }
        let mut r = Reader::new(&self.default_data[offset..], Endian::Little);
        Ok((type_index, signed(compressed_u32(&mut r)?)))
    }

    pub(crate) fn generic_position(&self, parameter: u32) -> Result<u16> {
        self.generic_positions
            .get(parameter as usize)
            .copied()
            .ok_or_else(|| Error::malformed(format!("generic parameter {parameter} out of range")))
    }

    /// The image (assembly file) a type definition belongs to.
    pub(crate) fn image_of(&self, definition: u32) -> Option<&Image> {
        self.images.iter().find(|i| (i.first_type..i.first_type + i.type_count).contains(&definition))
    }

    /// The declaring types of the custom attributes on a member (by token) of a type in `image`.
    pub(crate) fn attribute_types(&self, image: &Image, token: u32) -> Result<Vec<u32>> {
        let end = (image.attribute_start + image.attribute_count).min(self.attribute_ranges.len());
        let ranges = &self.attribute_ranges[image.attribute_start.min(end)..end];
        let Ok(found) = ranges.binary_search_by_key(&token, |(t, _)| *t) else {
            return Ok(Vec::new());
        };
        let position = image.attribute_start + found;
        let start = self.attribute_ranges[position].1 as usize;
        let stop = self.attribute_ranges.get(position + 1).map_or(self.attribute_data.len(), |(_, s)| *s as usize);
        if start > stop || stop > self.attribute_data.len() {
            return Err(Error::malformed(format!("attribute data for token {token:#x} out of range")));
        }
        let blob = &self.attribute_data[start..stop];
        let mut r = Reader::new(blob, Endian::Little);
        let count = compressed_u32(&mut r)? as usize;
        (0..count)
            .map(|_| {
                let method = r.u32()?;
                self.method_owners
                    .get(method as usize)
                    .copied()
                    .filter(|&o| o != u32::MAX)
                    .ok_or_else(|| Error::malformed(format!("attribute constructor {method} out of range")))
            })
            .collect()
    }
}

/// A compressed unsigned integer read as signed (the runtime's `ReadCompressedInt32`): the low bit is the sign, the rest
/// the magnitude, and all ones is `i32::MIN`.
fn signed(raw: u32) -> i32 {
    if raw == u32::MAX {
        i32::MIN
    } else if raw & 1 == 1 {
        -((raw >> 1) as i32) - 1
    } else {
        (raw >> 1) as i32
    }
}

/// Unity's compressed unsigned integer (as `MetadataUtils.WriteCompressedUInt32` writes it).
fn compressed_u32(r: &mut Reader<'_>) -> Result<u32> {
    let first = r.u8()?;
    Ok(match first {
        0..=0x7F => first.into(),
        0xF0 => r.u32()?,
        0xFE => u32::MAX - 1,
        0xFF => u32::MAX,
        _ if first & 0xC0 == 0xC0 => (u32::from(first & 0x3F) << 24) | (u32::from(r.u8()?) << 16) | (u32::from(r.u8()?) << 8) | u32::from(r.u8()?),
        _ => (u32::from(first & 0x7F) << 8) | u32::from(r.u8()?),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn compressed_integers_decode_every_width() {
        for (bytes, value) in [
            (&[0x05][..], 5u32),
            (&[0x81, 0x02], 0x102),
            (&[0xC1, 0x02, 0x03, 0x04], 0x0102_0304),
            (&[0xF0, 4, 3, 2, 1], 0x0102_0304),
            (&[0xFE], u32::MAX - 1),
        ] {
            assert_eq!(compressed_u32(&mut Reader::new(bytes, Endian::Little)).unwrap(), value, "{bytes:?}");
        }
    }

    #[test]
    fn signed_constants_decode_as_the_runtime_does() {
        assert_eq!((signed(56), signed(57), signed(0), signed(1)), (28, -29, 0, -1));
        assert_eq!(signed(u32::MAX), i32::MIN);
        assert_eq!(signed(u32::MAX - 1), i32::MAX);
    }

    #[test]
    fn indices_use_all_ones_for_none() {
        let mut r = Reader::new(&[0xFF, 0x07, 0xFF, 0xFF, 0x09, 0x00], Endian::Little);
        assert_eq!(index(&mut r, 1).unwrap(), None);
        assert_eq!(index(&mut r, 1).unwrap(), Some(7));
        assert_eq!(index(&mut r, 2).unwrap(), None);
        assert_eq!(index(&mut r, 2).unwrap(), Some(9));
        assert_eq!((width(0xFF), width(0x100), width(0x10000)), (1, 2, 4));
    }

    #[test]
    fn refuses_other_versions_and_files() {
        let mut bytes = vec![0; 8 + 12 * SECTIONS.len()];
        bytes[..4].copy_from_slice(&MAGIC.to_le_bytes());
        bytes[4..8].copy_from_slice(&31i32.to_le_bytes());
        assert!(matches!(Metadata::parse(bytes.clone()), Err(Error::Unsupported(m)) if m.contains("version 31")));
        bytes[..4].copy_from_slice(&0u32.to_le_bytes());
        assert!(matches!(Metadata::parse(bytes), Err(Error::Malformed(_))));
    }
}
