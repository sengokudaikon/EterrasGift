//! `GameAssembly.dll` as data: its PE sections, the IL2CPP metadata registration in them, and the `Il2CppType`
//! records the registration's type table points at. Only initialised data sections are read; no code is touched and
//! nothing is loaded or run. The sections are large, so they are read in place: the registration is searched for a
//! chunk at a time, and records are read through a small cache of pages.

use eg_binary::{Endian, Error, Reader, Result, read_at};
use std::cell::RefCell;
use std::collections::HashMap;
use std::fs::File;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::rc::Rc;

const PAGE: u64 = 4096;
/// The pages kept for record reads; the few classes a layout needs touch far fewer.
const KEPT_PAGES: usize = 2048;
/// How much of a section the registration search reads at once (a multiple of the word size).
const SCAN_BYTES: usize = 1 << 20;

const INITIALISED_DATA: u32 = 0x40;
const CODE: u32 = 0x20;
/// The registration's counts are sanity-bounded as Cpp2IL bounds them.
const MAX_COUNT: u64 = 0xC_0000;
const MAX_SECTION_BYTES: u32 = 512 << 20;

struct Section {
    virtual_address: u64,
    /// Its size in memory: its raw data, then zeroes up to its virtual size.
    size: u64,
    raw_pointer: u64,
    raw_size: u64,
}

pub struct Binary {
    file: File,
    path: PathBuf,
    length: u64,
    sections: Vec<Section>,
    pages: RefCell<HashMap<u64, Rc<[u8]>>>,
    /// The type table: a pointer per type index.
    types: Vec<u64>,
}

/// What an `Il2CppType` record says (Unity 2021.2+ layout: data word, then attributes, kind and flags).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RawType {
    pub data: u64,
    /// The member attributes of the use (a field's `FieldAttributes`).
    pub attributes: u16,
    pub kind: u8,
    pub by_ref: bool,
}

/// `Il2CppTypeEnum` kinds used here.
pub mod kind {
    pub const BOOLEAN: u8 = 0x02;
    pub const CHAR: u8 = 0x03;
    pub const I1: u8 = 0x04;
    pub const U1: u8 = 0x05;
    pub const I2: u8 = 0x06;
    pub const U2: u8 = 0x07;
    pub const I4: u8 = 0x08;
    pub const U4: u8 = 0x09;
    pub const I8: u8 = 0x0A;
    pub const U8: u8 = 0x0B;
    pub const R4: u8 = 0x0C;
    pub const R8: u8 = 0x0D;
    pub const STRING: u8 = 0x0E;
    pub const PTR: u8 = 0x0F;
    pub const VALUE_TYPE: u8 = 0x11;
    pub const CLASS: u8 = 0x12;
    pub const VAR: u8 = 0x13;
    pub const ARRAY: u8 = 0x14;
    pub const GENERIC_INSTANCE: u8 = 0x15;
    pub const I: u8 = 0x18;
    pub const U: u8 = 0x19;
    pub const OBJECT: u8 = 0x1C;
    pub const SZ_ARRAY: u8 = 0x1D;
}

impl Binary {
    /// Reads the data sections and finds the registration whose counts match the metadata's `type_definitions`.
    pub fn open(path: &Path, type_definitions: usize) -> Result<Binary> {
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
            // Unwind and relocation tables are data too, but nothing the registration points at, so the search skips them.
            let tables = matches!(r.take(8)?, [b'.', b'p', b'd', b'a', b't', b'a', 0, 0] | [b'.', b'r', b'e', b'l', b'o', b'c', 0, 0]);
            let (virtual_size, virtual_address, raw_size, raw_pointer) = (r.u32()?, r.u32()?, r.u32()?, r.u32()?);
            r.take(12)?;
            let characteristics = r.u32()?;
            headers.push((virtual_size, virtual_address, raw_size, raw_pointer, characteristics, tables));
        }
        let length = file.metadata().map_err(|e| Error::io(path, e))?.len();
        let mut sections = Vec::new();
        for (virtual_size, virtual_address, raw_size, raw_pointer, characteristics, tables) in headers {
            if characteristics & INITIALISED_DATA == 0 || characteristics & CODE != 0 || tables {
                continue;
            }
            if raw_size > MAX_SECTION_BYTES || virtual_size > MAX_SECTION_BYTES {
                return Err(Error::malformed("section size out of range"));
            }
            if u64::from(raw_pointer) + u64::from(raw_size) > length {
                return Err(Error::malformed("section runs past the end of the file"));
            }
            sections.push(Section {
                virtual_address: image_base + u64::from(virtual_address),
                size: virtual_size.max(raw_size).into(),
                raw_pointer: raw_pointer.into(),
                raw_size: raw_size.into(),
            });
        }
        let mut binary = Binary {
            file,
            path: path.to_path_buf(),
            length,
            sections,
            pages: RefCell::new(HashMap::new()),
            types: Vec::new(),
        };
        let (types, count) = binary.find_types(type_definitions as u64)?;
        binary.types = (0..count).map(|i| binary.u64(types + i * 8)).collect::<Result<_>>()?;
        Ok(binary)
    }

    /// The section `length` bytes at `address` lie in, and where they start in it.
    fn locate(&self, address: u64, length: u64) -> Option<(&Section, u64)> {
        self.sections.iter().find_map(|section| {
            let start = address.checked_sub(section.virtual_address)?;
            start.checked_add(length).is_some_and(|end| end <= section.size).then_some((section, start))
        })
    }

    /// `N` bytes at `address`, within one data section.
    fn bytes<const N: usize>(&self, address: u64) -> Result<[u8; N]> {
        let (section, start) = self
            .locate(address, N as u64)
            .ok_or_else(|| Error::malformed(format!("address {address:#x} is outside the data sections")))?;
        let mut out = [0; N];
        // Past its raw data a section is zero-filled up to its virtual size.
        let raw = section.raw_size.saturating_sub(start).min(N as u64) as usize;
        self.file_bytes(section.raw_pointer + start, &mut out[..raw])?;
        Ok(out)
    }

    fn file_bytes(&self, mut offset: u64, mut out: &mut [u8]) -> Result<()> {
        while !out.is_empty() {
            let page = self.page(offset / PAGE)?;
            let from = (offset % PAGE) as usize;
            let count = (page.len() - from).min(out.len());
            if count == 0 {
                return Err(Error::malformed("read past the end of GameAssembly.dll"));
            }
            out[..count].copy_from_slice(&page[from..from + count]);
            out = &mut out[count..];
            offset += count as u64;
        }
        Ok(())
    }

    fn page(&self, index: u64) -> Result<Rc<[u8]>> {
        if let Some(page) = self.pages.borrow().get(&index) {
            return Ok(page.clone());
        }
        let start = index * PAGE;
        let mut page = vec![0; PAGE.min(self.length.saturating_sub(start)) as usize];
        read_at(&self.file, start, &mut page).map_err(|e| Error::io(&self.path, e))?;
        let page: Rc<[u8]> = page.into();
        let mut pages = self.pages.borrow_mut();
        if pages.len() >= KEPT_PAGES {
            pages.clear();
        }
        pages.insert(index, page.clone());
        Ok(page)
    }

    fn mapped(&self, address: u64) -> bool {
        self.locate(address, 1).is_some()
    }

    fn u64(&self, address: u64) -> Result<u64> {
        Ok(u64::from_le_bytes(self.bytes(address)?))
    }

    /// The metadata registration (format 39: eight count/pointer pairs) is where `typeDefinitionsSizesCount`, its
    /// 13th word, equals the number of type definitions and every other word is a plausible count or pointer.
    /// Returns the type table's address and length.
    fn find_types(&self, type_definitions: u64) -> Result<(u64, u64)> {
        let mut found = Vec::new();
        let mut chunk = vec![0; SCAN_BYTES];
        // The zeroes past a section's raw data are no count of type definitions, which is never 0 here.
        for section in &self.sections {
            let mut done = 0;
            while done < section.raw_size {
                let length = (section.raw_size - done).min(SCAN_BYTES as u64) as usize;
                read_at(&self.file, section.raw_pointer + done, &mut chunk[..length]).map_err(|e| Error::io(&self.path, e))?;
                let at = section.virtual_address + done;
                done += length as u64;
                for (i, word) in chunk[..length].as_chunks::<8>().0.iter().enumerate() {
                    if u64::from_le_bytes(*word) != type_definitions {
                        continue;
                    }
                    let Some(start) = (at + i as u64 * 8).checked_sub(12 * 8) else {
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
        }
        match found.as_slice() {
            [one] => Ok(*one),
            [] => Err(Error::unsupported("no IL2CPP metadata registration found in GameAssembly.dll")),
            many => Err(Error::data(format!("{} candidate metadata registrations", many.len()))),
        }
    }

    /// The type a metadata type index names.
    pub fn type_at(&self, index: u32) -> Result<RawType> {
        let address = *self.types.get(index as usize).ok_or_else(|| Error::malformed(format!("type index {index} out of range")))?;
        self.type_from(address)
    }

    /// The type at an `Il2CppType*`.
    pub fn type_from(&self, address: u64) -> Result<RawType> {
        let bytes = self.bytes::<12>(address)?;
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
    pub fn generic_class(&self, address: u64) -> Result<(RawType, Vec<RawType>)> {
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
    pub fn array(&self, address: u64) -> Result<(RawType, u8)> {
        let element = self.type_from(self.u64(address)?)?;
        Ok((element, self.bytes::<1>(address + 8)?[0]))
    }
}
