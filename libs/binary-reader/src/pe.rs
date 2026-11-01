//! Bounded PE32+ headers and file-backed RVA mapping.
use std::io;

#[derive(Clone, Copy, Debug)]
pub struct Section {
    pub name: [u8; 8],
    pub va: u32,
    pub virtual_size: u32,
    pub raw_at: u32,
    pub raw_size: u32,
    pub flags: u32,
}
#[derive(Debug)]
pub struct Layout {
    pub image_base: u64,
    pub image_size: u32,
    pub exports: (u32, u32),
    pub dll: bool,
    pub exceptions: Option<(u32, u32)>,
    sections: Vec<Section>,
}
fn invalid(reason: &'static str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, reason)
}
fn word(bytes: &[u8], at: usize) -> u16 {
    u16::from_le_bytes(bytes[at..at + 2].try_into().unwrap())
}
fn dword(bytes: &[u8], at: usize) -> u32 {
    u32::from_le_bytes(bytes[at..at + 4].try_into().unwrap())
}

struct HeaderReader<R> {
    length: u64,
    read: R,
}
impl<R: FnMut(u64, &mut [u8]) -> io::Result<()>> HeaderReader<R> {
    fn span<const N: usize>(&mut self, at: u64) -> io::Result<[u8; N]> {
        if at.checked_add(N as u64).is_none_or(|end| end > self.length) {
            return Err(invalid("PE header bounds"));
        }
        let mut bytes = [0; N];
        (self.read)(at, &mut bytes)?;
        Ok(bytes)
    }

    fn nt_header(&mut self) -> io::Result<(u64, [u8; 24])> {
        let dos = self.span::<64>(0)?;
        if &dos[..2] != b"MZ" {
            return Err(invalid("DOS header"));
        }
        let nt = u64::from(dword(&dos, 0x3c));
        let header = self.span::<24>(nt)?;
        if &header[..4] != b"PE\0\0" || word(&header, 4) != 0x8664 {
            return Err(invalid("Windows x64 PE header"));
        }
        Ok((nt, header))
    }

    fn data_directory(&mut self, optional_at: u64, optional_size: u64, directory_count: u32, index: u32, reason: &'static str) -> io::Result<Option<(u32, u32)>> {
        if index >= directory_count {
            return Ok(None);
        }
        let offset = 112 + u64::from(index) * 8;
        if optional_size < offset + 8 {
            return Err(invalid(reason));
        }
        let directory = self.span::<8>(optional_at + offset)?;
        Ok(Some((dword(&directory, 0), dword(&directory, 4))))
    }

    fn sections(&mut self, at: u64, count: u16, image_size: u32) -> io::Result<Vec<Section>> {
        let mut sections: Vec<Section> = Vec::new();
        for index in 0..count {
            let bytes = self.span::<40>(at + u64::from(index) * 40)?;
            let section = Section {
                name: bytes[..8].try_into().unwrap(),
                virtual_size: dword(&bytes, 8),
                va: dword(&bytes, 12),
                raw_size: dword(&bytes, 16),
                raw_at: dword(&bytes, 20),
                flags: dword(&bytes, 36),
            };
            if u64::from(section.raw_at) + u64::from(section.raw_size) > self.length {
                return Err(invalid("PE section file bounds"));
            }
            let end = section
                .va
                .checked_add(section.virtual_size.max(section.raw_size))
                .ok_or_else(|| invalid("PE section overflow"))?;
            if image_size != 0 && end > image_size {
                return Err(invalid("PE section image bounds"));
            }
            if sections.iter().any(|other| {
                let other_end = other.va + other.virtual_size.max(other.raw_size);
                section.va < end && other.va < other_end && section.va < other_end && other.va < end
            }) {
                return Err(invalid("PE overlapping mapped sections"));
            }
            sections.push(section);
        }
        Ok(sections)
    }
}

impl Layout {
    pub fn from_bytes(bytes: &[u8]) -> io::Result<Self> {
        Self::read(bytes.len() as u64, |offset, output| {
            let offset = usize::try_from(offset).map_err(|_| invalid("PE file offset"))?;
            let end = offset.checked_add(output.len()).ok_or_else(|| invalid("PE file offset overflow"))?;
            output.copy_from_slice(bytes.get(offset..end).ok_or_else(|| invalid("PE file bounds"))?);
            Ok(())
        })
    }
    /// Reads only headers; section data remains with the caller's retained file.
    pub fn read(length: u64, read: impl FnMut(u64, &mut [u8]) -> io::Result<()>) -> io::Result<Self> {
        let mut reader = HeaderReader { length, read };
        let (nt, header) = reader.nt_header()?;
        let count = word(&header, 6);
        let optional_size = u64::from(word(&header, 20));
        let optional_at = nt + 24;
        let sections_at = optional_at + optional_size;
        if !(1..=96).contains(&count) || optional_size < 112 || sections_at > length {
            return Err(invalid("PE table shape"));
        }
        let optional = reader.span::<112>(optional_at)?;
        if word(&optional, 0) != 0x20b {
            return Err(invalid("PE32+ optional header"));
        }
        let image_size = dword(&optional, 56);
        let directory_count = dword(&optional, 108);
        let exports = reader
            .data_directory(optional_at, optional_size, directory_count, 0, "PE data directory size")?
            .unwrap_or_default();
        let exceptions = reader.data_directory(optional_at, optional_size, directory_count, 3, "PE exception directory size")?;
        let sections = reader.sections(sections_at, count, image_size)?;
        Ok(Self {
            image_base: u64::from_le_bytes(optional[24..32].try_into().unwrap()),
            image_size,
            exports,
            dll: word(&header, 22) & 0x2000 != 0,
            exceptions,
            sections,
        })
    }
    pub fn sections(&self) -> &[Section] {
        &self.sections
    }
    /// `length` file-backed bytes at `rva` in `image`, the whole file these headers came from.
    pub fn span<'a>(&self, image: &'a [u8], rva: u32, length: usize, executable: bool) -> io::Result<&'a [u8]> {
        let offset = usize::try_from(self.file_offset(u64::from(rva), length, executable)?).map_err(|_| invalid("PE file offset"))?;
        let end = offset.checked_add(length).ok_or_else(|| invalid("PE integer overflow"))?;
        image.get(offset..end).ok_or_else(|| invalid("PE file bounds"))
    }
    /// The RVA of the one export named `name` in `image`, the whole file these headers came from. Bounded, and refusing
    /// an ambiguous or forwarded export.
    pub fn export(&self, image: &[u8], name: &[u8]) -> io::Result<u32> {
        let (at, size) = self.exports;
        if at == 0 || size < 40 || at.checked_add(size).is_none_or(|end| end > self.image_size) {
            return Err(invalid("PE export directory bounds"));
        }
        let span = |rva: u32, length: usize, executable: bool| self.span(image, rva, length, executable);
        let directory = span(at, 40, false)?;
        let (count, names) = (dword(directory, 20), dword(directory, 24));
        if count == 0 || count > 4096 || names == 0 || names > 4096 {
            return Err(invalid("PE export count"));
        }
        let functions = span(dword(directory, 28), count as usize * 4, false)?;
        let name_rvas = span(dword(directory, 32), names as usize * 4, false)?;
        let ordinals = span(dword(directory, 36), names as usize * 2, false)?;
        let mut found = None;
        for index in 0..names as usize {
            let rva = dword(name_rvas, index * 4);
            let mut matches = true;
            let mut length = 0;
            loop {
                if length == 256 {
                    return Err(invalid("PE export name bound"));
                }
                let byte = span(rva.checked_add(length as u32).ok_or_else(|| invalid("PE name overflow"))?, 1, false)?[0];
                if byte == 0 {
                    matches &= length == name.len();
                    break;
                }
                matches &= name.get(length) == Some(&byte);
                length += 1;
            }
            if matches {
                let ordinal = usize::from(word(ordinals, index * 2));
                if ordinal >= count as usize || found.is_some() {
                    return Err(invalid("PE export ordinal/ambiguity"));
                }
                let function = dword(functions, ordinal * 4);
                if (at..at + size).contains(&function) {
                    return Err(invalid("forwarded export"));
                }
                span(function, 1, true)?;
                found = Some(function);
            }
        }
        found.ok_or_else(|| invalid("required export missing"))
    }
    pub fn file_offset(&self, rva: u64, length: usize, executable: bool) -> io::Result<u64> {
        let end = rva.checked_add(length as u64).ok_or_else(|| invalid("PE RVA overflow"))?;
        let mut result = None;
        for section in &self.sections {
            if rva >= u64::from(section.va) && end <= u64::from(section.va) + u64::from(section.raw_size) {
                if result.is_some() || executable && section.flags & 0x2000_0000 == 0 {
                    return Err(invalid("PE ambiguous/nonexecutable mapping"));
                }
                result = Some(u64::from(section.raw_at) + rva - u64::from(section.va));
            }
        }
        result.ok_or_else(|| invalid("PE RVA not file-backed"))
    }
}

/// One record of an x64 exception directory.
#[derive(Clone, Copy, Debug)]
pub struct RuntimeFunction {
    pub begin: u32,
    pub end: u32,
    pub unwind: u32,
}

/// The records of a whole x64 exception directory. Its size is checked first.
pub fn runtime_functions(table: &[u8]) -> io::Result<impl Iterator<Item = RuntimeFunction> + '_> {
    if table.is_empty() || !table.len().is_multiple_of(12) || table.len() > 16 * 1024 * 1024 {
        return Err(invalid("runtime function table bounds"));
    }
    Ok(table.as_chunks::<12>().0.iter().map(|record| RuntimeFunction {
        begin: dword(record, 0),
        end: dword(record, 4),
        unwind: dword(record, 8),
    }))
}

/// The length of an `UNWIND_INFO` from its first four bytes: its fixed part, codes, and handler or chained record. Refuses
/// another version, unknown flags, a chain with a handler, and more than 64 codes.
pub fn unwind_info_length(header: &[u8; 4]) -> io::Result<usize> {
    let flags = header[0] >> 3;
    if header[0] & 7 != 1 || flags & !7 != 0 || flags & 4 != 0 && flags & 3 != 0 || header[2] > 64 {
        return Err(invalid("unwind version/flags/count"));
    }
    let trailer = if flags & 4 != 0 {
        12
    } else if flags & 3 != 0 {
        4
    } else {
        0
    };
    Ok(4 + ((usize::from(header[2]) + 1) & !1) * 2 + trailer)
}
