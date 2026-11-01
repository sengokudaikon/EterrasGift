//! Bounded PE32+ headers and file-backed RVA mapping.
use std::io;

#[derive(Clone, Copy, Debug)]
struct Section {
    va: u32,
    virtual_size: u32,
    raw_at: u32,
    raw_size: u32,
    flags: u32,
}
#[derive(Debug)]
pub struct Layout {
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
            image_size,
            exports,
            dll: word(&header, 22) & 0x2000 != 0,
            exceptions,
            sections,
        })
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
