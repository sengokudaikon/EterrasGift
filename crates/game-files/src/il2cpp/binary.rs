//! `GameAssembly.dll` as data: its PE sections, the IL2CPP metadata registration in them, and the `Il2CppType`
//! records the registration's type table points at. Only initialised data sections are read; no code is touched and
//! nothing is loaded or run.

use crate::error::{Error, Result};
use crate::reader::{Endian, Reader};
use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::path::Path;

const INITIALISED_DATA: u32 = 0x40;
const CODE: u32 = 0x20;
/// The registration's counts are sanity-bounded as Cpp2IL bounds them.
const MAX_COUNT: u64 = 0xC_0000;
const MAX_SECTION_BYTES: u32 = 512 << 20;

struct Section {
    virtual_address: u64,
    bytes: Vec<u8>,
}

pub(crate) struct Binary {
    sections: Vec<Section>,
    /// The type table: a pointer per type index.
    types: Vec<u64>,
}

/// What an `Il2CppType` record says (Unity 2021.2+ layout: data word, then attributes, kind and flags).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct RawType {
    pub(crate) data: u64,
    /// The member attributes of the use (a field's `FieldAttributes`).
    pub(crate) attributes: u16,
    pub(crate) kind: u8,
    pub(crate) by_ref: bool,
}

/// `Il2CppTypeEnum` kinds used here.
pub(crate) mod kind {
    pub(crate) const BOOLEAN: u8 = 0x02;
    pub(crate) const CHAR: u8 = 0x03;
    pub(crate) const I1: u8 = 0x04;
    pub(crate) const U1: u8 = 0x05;
    pub(crate) const I2: u8 = 0x06;
    pub(crate) const U2: u8 = 0x07;
    pub(crate) const I4: u8 = 0x08;
    pub(crate) const U4: u8 = 0x09;
    pub(crate) const I8: u8 = 0x0A;
    pub(crate) const U8: u8 = 0x0B;
    pub(crate) const R4: u8 = 0x0C;
    pub(crate) const R8: u8 = 0x0D;
    pub(crate) const STRING: u8 = 0x0E;
    pub(crate) const PTR: u8 = 0x0F;
    pub(crate) const VALUE_TYPE: u8 = 0x11;
    pub(crate) const CLASS: u8 = 0x12;
    pub(crate) const VAR: u8 = 0x13;
    pub(crate) const ARRAY: u8 = 0x14;
    pub(crate) const GENERIC_INSTANCE: u8 = 0x15;
    pub(crate) const I: u8 = 0x18;
    pub(crate) const U: u8 = 0x19;
    pub(crate) const OBJECT: u8 = 0x1C;
    pub(crate) const SZ_ARRAY: u8 = 0x1D;
}

impl Binary {
    /// Reads the data sections and finds the registration whose counts match the metadata's `type_definitions`.
    pub(crate) fn open(path: &Path, type_definitions: usize) -> Result<Binary> {
        let mut file = File::open(path).map_err(|e| Error::io(path, e))?;
        let mut head = vec![0; 4096];
        file.read_exact(&mut head).map_err(|e| Error::io(path, e))?;
        let mut r = Reader::new(&head, Endian::Little);
        if r.take(2)? != b"MZ" {
            return Err(Error::malformed("not a PE file"));
        }
        r.seek(0x3C)?;
        let pe = r.u32()? as usize;
        r.seek(pe)?;
        if r.take(4)? != b"PE\0\0" {
            return Err(Error::malformed("no PE signature"));
        }
        let machine = r.u16()?;
        if machine != 0x8664 {
            return Err(Error::unsupported(format!("PE machine {machine:#x}, not x64")));
        }
        let section_count = r.u16()?;
        r.take(12)?;
        let optional_size = r.u16()? as usize;
        r.u16()?;
        let optional = r.position();
        if r.u16()? != 0x20B {
            return Err(Error::unsupported("not a PE32+ image"));
        }
        r.seek(optional + 24)?;
        let image_base = r.u64()?;
        r.seek(optional + optional_size)?;
        let mut headers = Vec::new();
        for _ in 0..section_count {
            r.take(8)?;
            let (virtual_size, virtual_address, raw_size, raw_pointer) = (r.u32()?, r.u32()?, r.u32()?, r.u32()?);
            r.take(12)?;
            let characteristics = r.u32()?;
            headers.push((virtual_size, virtual_address, raw_size, raw_pointer, characteristics));
        }
        let mut sections = Vec::new();
        for (virtual_size, virtual_address, raw_size, raw_pointer, characteristics) in headers {
            if characteristics & INITIALISED_DATA == 0 || characteristics & CODE != 0 {
                continue;
            }
            if raw_size > MAX_SECTION_BYTES || virtual_size > MAX_SECTION_BYTES {
                return Err(Error::malformed("section size out of range"));
            }
            let mut bytes = vec![0; raw_size as usize];
            file.seek(SeekFrom::Start(raw_pointer.into())).map_err(|e| Error::io(path, e))?;
            file.read_exact(&mut bytes).map_err(|e| Error::io(path, e))?;
            // Past its raw data a section is zero-filled up to its virtual size.
            bytes.resize(virtual_size.max(raw_size) as usize, 0);
            sections.push(Section {
                virtual_address: image_base + u64::from(virtual_address),
                bytes,
            });
        }
        let mut binary = Binary { sections, types: Vec::new() };
        let (types, count) = binary.find_types(type_definitions as u64)?;
        binary.types = (0..count).map(|i| binary.u64(types + i * 8)).collect::<Result<_>>()?;
        Ok(binary)
    }

    fn slice(&self, address: u64, length: usize) -> Result<&[u8]> {
        for section in &self.sections {
            if address >= section.virtual_address {
                let start = (address - section.virtual_address) as usize;
                if start.checked_add(length).is_some_and(|end| end <= section.bytes.len()) {
                    return Ok(&section.bytes[start..start + length]);
                }
            }
        }
        Err(Error::malformed(format!("address {address:#x} is outside the data sections")))
    }

    fn mapped(&self, address: u64) -> bool {
        self.slice(address, 1).is_ok()
    }

    fn u64(&self, address: u64) -> Result<u64> {
        Ok(u64::from_le_bytes(self.slice(address, 8)?.try_into().expect("8 bytes")))
    }

    /// The metadata registration (format 39: eight count/pointer pairs) is where `typeDefinitionsSizesCount`, its
    /// 13th word, equals the number of type definitions and every other word is a plausible count or pointer.
    /// Returns the type table's address and length.
    fn find_types(&self, type_definitions: u64) -> Result<(u64, u64)> {
        let mut found = Vec::new();
        for section in &self.sections {
            for (i, word) in section.bytes.as_chunks::<8>().0.iter().enumerate() {
                if u64::from_le_bytes(*word) != type_definitions {
                    continue;
                }
                let Some(start) = (section.virtual_address + i as u64 * 8).checked_sub(12 * 8) else {
                    continue;
                };
                let Ok(words) = (0..16).map(|w| self.u64(start + w * 8)).collect::<Result<Vec<_>>>() else {
                    continue;
                };
                let plausible = words.iter().enumerate().all(|(w, &value)| match w % 2 {
                    0 => value < MAX_COUNT,
                    // metadataUsages (and rarely genericClasses) can be null.
                    _ => value == 0 && (w == 1 || w >= 14) || self.mapped(value),
                });
                // fieldOffsetsCount and typeDefinitionsSizesCount match the definitions; numTypes covers them.
                if plausible && words[10] == type_definitions && words[6] >= type_definitions {
                    found.push((words[7], words[6]));
                }
            }
        }
        match found.as_slice() {
            [one] => Ok(*one),
            [] => Err(Error::unsupported("no IL2CPP metadata registration found in GameAssembly.dll")),
            many => Err(Error::data(format!("{} candidate metadata registrations", many.len()))),
        }
    }

    /// The type a metadata type index names.
    pub(crate) fn type_at(&self, index: u32) -> Result<RawType> {
        let address = *self.types.get(index as usize).ok_or_else(|| Error::malformed(format!("type index {index} out of range")))?;
        self.type_from(address)
    }

    /// The type at an `Il2CppType*`.
    pub(crate) fn type_from(&self, address: u64) -> Result<RawType> {
        let bytes = self.slice(address, 12)?;
        let data = u64::from_le_bytes(bytes[..8].try_into().expect("8 bytes"));
        let bits = u32::from_le_bytes(bytes[8..12].try_into().expect("4 bytes"));
        Ok(RawType {
            data,
            attributes: (bits & 0xFFFF) as u16,
            kind: ((bits >> 16) & 0xFF) as u8,
            // Since Unity 2021.2 (format 27.2) num_mods is 5 bits: byref is bit 29.
            by_ref: bits >> 29 & 1 != 0,
        })
    }

    /// An `Il2CppGenericClass`: its generic type, and the type arguments of its class instance.
    pub(crate) fn generic_class(&self, address: u64) -> Result<(RawType, Vec<RawType>)> {
        let definition = self.type_from(self.u64(address)?)?;
        let instance = self.u64(address + 8)?;
        let count = self.u64(instance)?;
        if count == 0 || count > 64 {
            return Err(Error::malformed(format!("generic instance with {count} arguments")));
        }
        let arguments = self.u64(instance + 8)?;
        let arguments = (0..count).map(|i| self.type_from(self.u64(arguments + i * 8)?)).collect::<Result<_>>()?;
        Ok((definition, arguments))
    }

    /// The element type and rank of an `Il2CppArrayType`.
    pub(crate) fn array(&self, address: u64) -> Result<(RawType, u8)> {
        let element = self.type_from(self.u64(address)?)?;
        Ok((element, self.slice(address + 8, 1)?[0]))
    }
}
