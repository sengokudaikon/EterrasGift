//! Bounded PE32+ headers and file-backed RVA mapping, shared by artifact readers.
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
    pub(crate) image_size: u32,
    pub(crate) exports: (u32, u32),
    pub(crate) dll: bool,
    pub(crate) exceptions: Option<(u32, u32)>,
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
    pub fn read(length: u64, mut read: impl FnMut(u64, &mut [u8]) -> io::Result<()>) -> io::Result<Self> {
        fn span<const N: usize>(length: u64, at: u64, read: &mut impl FnMut(u64, &mut [u8]) -> io::Result<()>) -> io::Result<[u8; N]> {
            if at.checked_add(N as u64).is_none_or(|end| end > length) {
                return Err(invalid("PE header bounds"));
            }
            let mut bytes = [0; N];
            read(at, &mut bytes)?;
            Ok(bytes)
        }
        let dos = span::<64>(length, 0, &mut read)?;
        if &dos[..2] != b"MZ" {
            return Err(invalid("DOS header"));
        }
        let nt = u64::from(dword(&dos, 0x3c));
        let header = span::<24>(length, nt, &mut read)?;
        if &header[..4] != b"PE\0\0" || word(&header, 4) != 0x8664 {
            return Err(invalid("Windows x64 PE header"));
        }
        let count = word(&header, 6);
        let optional_size = u64::from(word(&header, 20));
        if !(1..=96).contains(&count) || optional_size < 112 || nt + 24 + optional_size > length {
            return Err(invalid("PE table shape"));
        }
        let optional = span::<112>(length, nt + 24, &mut read)?;
        if word(&optional, 0) != 0x20b {
            return Err(invalid("PE32+ optional header"));
        }
        let image_size = dword(&optional, 56);
        let exports = if dword(&optional, 108) > 0 {
            if optional_size < 120 {
                return Err(invalid("PE data directory size"));
            }
            let directory = span::<8>(length, nt + 24 + 112, &mut read)?;
            (dword(&directory, 0), dword(&directory, 4))
        } else {
            (0, 0)
        };
        let exceptions = if dword(&optional, 108) >= 4 {
            if optional_size < 144 {
                return Err(invalid("PE exception directory size"));
            }
            let directory = span::<8>(length, nt + 24 + 112 + 3 * 8, &mut read)?;
            Some((dword(&directory, 0), dword(&directory, 4)))
        } else {
            None
        };
        let mut sections: Vec<Section> = Vec::new();
        for index in 0..count {
            let bytes = span::<40>(length, nt + 24 + optional_size + u64::from(index) * 40, &mut read)?;
            let section = Section {
                virtual_size: dword(&bytes, 8),
                va: dword(&bytes, 12),
                raw_size: dword(&bytes, 16),
                raw_at: dword(&bytes, 20),
                flags: dword(&bytes, 36),
            };
            if u64::from(section.raw_at) + u64::from(section.raw_size) > length {
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
