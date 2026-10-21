//! UnityFS asset bundles (format 6–8, as Unity 2019–6000 writes them): a header, a block table and a directory, then
//! the files the bundle holds, stored as a run of blocks that are each uncompressed or LZ4-compressed. Opening reads
//! only the header and the tables; reading a file decompresses only the blocks that cover it, and a [`Stream`] reads
//! any part of one while holding only the few blocks read last.

use crate::error::{Error, Result};
use crate::reader::{Endian, Reader};
use std::cell::RefCell;
use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::rc::Rc;

/// How many unpacked blocks a stream keeps (Unity writes 128 KiB blocks): enough for objects read in file order, as a
/// scan of a file's objects reads them.
const KEPT_BLOCKS: usize = 8;

/// Larger than anything the game ships (its largest serialized file is ~90 MB); a guard against a bad size field.
const MAX_ENTRY_BYTES: u64 = 1 << 30;
const MAX_TABLE_BYTES: u32 = 64 << 20;
const MAX_BLOCK_BYTES: u32 = 64 << 20;

const COMPRESSION_MASK: u32 = 0x3F;
const DIRECTORY_COMBINED: u32 = 0x40;
const TABLE_AT_END: u32 = 0x80;
const TABLE_PADDED: u32 = 0x200;
const ENCRYPTED: u32 = 0x1400;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Entry {
    pub name: String,
    pub offset: u64,
    pub size: u64,
}

#[derive(Clone, Copy, Debug)]
struct Block {
    unpacked: u32,
    packed: u32,
    flags: u16,
    /// Where the block starts, in the file and in the unpacked stream.
    file_offset: u64,
    stream_offset: u64,
}

pub struct Bundle {
    path: PathBuf,
    unity_version: String,
    blocks: Vec<Block>,
    entries: Vec<Entry>,
}

fn decompress(compression: u32, packed: &[u8], unpacked: u32) -> Result<Vec<u8>> {
    let mut out = vec![0; unpacked as usize];
    decompress_into(compression, packed, &mut out)?;
    Ok(out)
}

/// Unpacks a block into `out`, which is its unpacked size.
fn decompress_into(compression: u32, packed: &[u8], out: &mut [u8]) -> Result<()> {
    match compression {
        0 => {
            if packed.len() != out.len() {
                return Err(Error::malformed(format!("stored block of {} bytes declares {}", packed.len(), out.len())));
            }
            out.copy_from_slice(packed);
            Ok(())
        }
        2 | 3 => {
            let written = lz4_flex::block::decompress_into(packed, out).map_err(|e| Error::malformed(format!("LZ4 block: {e}")))?;
            if written != out.len() {
                return Err(Error::malformed(format!("LZ4 block unpacked to {written} bytes, not {}", out.len())));
            }
            Ok(())
        }
        1 => Err(Error::unsupported("LZMA-compressed bundle blocks")),
        other => Err(Error::unsupported(format!("bundle compression type {other}"))),
    }
}

impl Bundle {
    pub fn open(path: impl AsRef<Path>) -> Result<Bundle> {
        let path = path.as_ref();
        let mut file = File::open(path).map_err(|e| Error::io(path, e))?;
        let length = file.metadata().map_err(|e| Error::io(path, e))?.len();
        // The fixed header and its two short strings fit well within the first few hundred bytes.
        let mut head = vec![0; length.min(4096) as usize];
        file.read_exact(&mut head).map_err(|e| Error::io(path, e))?;
        Self::parse(path, &mut file, length, &head).map_err(|e| e.context(path.display()))
    }

    fn parse(path: &Path, file: &mut File, length: u64, head: &[u8]) -> Result<Bundle> {
        let mut reader = Reader::new(head, Endian::Big);
        if reader.cstring()? != "UnityFS" {
            return Err(Error::malformed("not a UnityFS bundle"));
        }
        let format = reader.u32()?;
        if !(6..=8).contains(&format) {
            return Err(Error::unsupported(format!("UnityFS format {format}")));
        }
        let _player_version = reader.cstring()?;
        let unity_version = reader.cstring()?;
        let size = reader.i64()?;
        let (packed_table, unpacked_table, flags) = (reader.u32()?, reader.u32()?, reader.u32()?);
        if u64::try_from(size).ok() != Some(length) {
            return Err(Error::malformed(format!("bundle declares {size} bytes but the file has {length}")));
        }
        if flags & ENCRYPTED != 0 {
            return Err(Error::unsupported("encrypted bundle"));
        }
        if flags & DIRECTORY_COMBINED == 0 {
            return Err(Error::unsupported("bundle without a combined block and directory table"));
        }
        if packed_table > MAX_TABLE_BYTES || unpacked_table > MAX_TABLE_BYTES {
            return Err(Error::malformed("block table size out of range"));
        }
        if format >= 7 {
            reader.align(16)?;
        }
        let header_end = reader.position() as u64;
        let (table_offset, mut data_start) = if flags & TABLE_AT_END != 0 {
            (
                length
                    .checked_sub(u64::from(packed_table))
                    .ok_or_else(|| Error::malformed("block table larger than the file"))?,
                header_end,
            )
        } else {
            (header_end, header_end + u64::from(packed_table))
        };
        if flags & TABLE_PADDED != 0 && flags & TABLE_AT_END == 0 {
            data_start = data_start.div_ceil(16) * 16;
        }
        let mut packed = vec![0; packed_table as usize];
        file.seek(SeekFrom::Start(table_offset)).map_err(|e| Error::io(path, e))?;
        file.read_exact(&mut packed).map_err(|e| Error::io(path, e))?;
        let table = decompress(flags & COMPRESSION_MASK, &packed, unpacked_table).map_err(|e| e.context("block table"))?;

        let mut reader = Reader::new(&table, Endian::Big);
        reader.take(16)?;
        let block_count = reader.count(10)?;
        let mut blocks = Vec::with_capacity(block_count);
        let (mut file_offset, mut stream_offset) = (data_start, 0u64);
        for _ in 0..block_count {
            let (unpacked, packed, flags) = (reader.u32()?, reader.u32()?, reader.u16()?);
            if unpacked > MAX_BLOCK_BYTES || packed > MAX_BLOCK_BYTES {
                return Err(Error::malformed("block size out of range"));
            }
            blocks.push(Block {
                unpacked,
                packed,
                flags,
                file_offset,
                stream_offset,
            });
            file_offset += u64::from(packed);
            stream_offset += u64::from(unpacked);
        }
        if file_offset > length {
            return Err(Error::malformed("blocks run past the end of the file"));
        }
        let entry_count = reader.count(21)?;
        let mut entries = Vec::with_capacity(entry_count);
        for _ in 0..entry_count {
            let (offset, size, _flags) = (reader.i64()?, reader.i64()?, reader.u32()?);
            let name = reader.cstring()?;
            let (Ok(offset), Ok(size)) = (u64::try_from(offset), u64::try_from(size)) else {
                return Err(Error::malformed(format!("entry {name} has a negative offset or size")));
            };
            if offset.checked_add(size).is_none_or(|end| end > stream_offset) {
                return Err(Error::malformed(format!("entry {name} runs past the blocks")));
            }
            entries.push(Entry { name, offset, size });
        }
        Ok(Bundle {
            path: path.to_path_buf(),
            unity_version,
            blocks,
            entries,
        })
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    /// The Unity version that built the bundle, e.g. `6000.4.8f1`.
    pub fn unity_version(&self) -> &str {
        &self.unity_version
    }

    pub fn entries(&self) -> &[Entry] {
        &self.entries
    }

    pub fn entry(&self, name: &str) -> Option<&Entry> {
        self.entries.iter().find(|entry| entry.name == name)
    }

    /// The bytes of one file in the bundle.
    pub fn read(&self, name: &str) -> Result<Vec<u8>> {
        let stream = self.stream(name)?;
        let mut out = vec![0; stream.len() as usize];
        stream.read_at(0, &mut out)?;
        Ok(out)
    }

    /// One file in the bundle, read in parts as they are asked for.
    pub fn stream(&self, name: &str) -> Result<Stream> {
        let entry = self.entry(name).ok_or_else(|| Error::data(format!("{} holds no {name}", self.path.display())))?;
        if entry.size > MAX_ENTRY_BYTES {
            return Err(Error::unsupported(format!("{name} is {} bytes", entry.size)));
        }
        let (start, end) = (entry.offset, entry.offset + entry.size);
        let blocks = self
            .blocks
            .iter()
            .filter(|b| b.stream_offset < end && b.stream_offset + u64::from(b.unpacked) > start)
            .copied()
            .collect();
        Ok(Stream {
            file: RefCell::new(File::open(&self.path).map_err(|e| Error::io(&self.path, e))?),
            label: format!("{}, {name}", self.path.display()),
            start,
            size: entry.size,
            blocks,
            kept: RefCell::new(Vec::new()),
            packed: RefCell::new(Vec::new()),
        })
    }
}

/// One file in a bundle: any part of it, unpacking only the blocks that part lies in.
pub struct Stream {
    file: RefCell<File>,
    label: String,
    /// Where the file starts in the bundle's unpacked stream, and its size.
    start: u64,
    size: u64,
    blocks: Vec<Block>,
    /// The blocks unpacked last, by index into `blocks`, most recent last.
    kept: RefCell<Vec<(usize, Rc<Vec<u8>>)>>,
    /// Scratch for a block's packed bytes.
    packed: RefCell<Vec<u8>>,
}

impl Stream {
    pub fn len(&self) -> u64 {
        self.size
    }

    pub fn is_empty(&self) -> bool {
        self.size == 0
    }

    /// Fills `out` from `at` in the file.
    pub fn read_at(&self, at: u64, out: &mut [u8]) -> Result<()> {
        if at.checked_add(out.len() as u64).is_none_or(|end| end > self.size) {
            return Err(Error::malformed(format!("{}: {} bytes at {at} run past its {} bytes", self.label, out.len(), self.size)));
        }
        let (mut position, mut written) = (self.start + at, 0);
        let first = self.blocks.partition_point(|b| b.stream_offset + u64::from(b.unpacked) <= position);
        for index in first..self.blocks.len() {
            if written == out.len() {
                break;
            }
            let block = self.blocks[index];
            let unpacked = self.block(index)?;
            let from = (position - block.stream_offset) as usize;
            let count = (unpacked.len() - from).min(out.len() - written);
            out[written..written + count].copy_from_slice(&unpacked[from..from + count]);
            written += count;
            position += count as u64;
        }
        if written != out.len() {
            return Err(Error::malformed(format!("{}: its blocks end before its {} bytes", self.label, self.size)));
        }
        Ok(())
    }

    fn block(&self, index: usize) -> Result<Rc<Vec<u8>>> {
        let mut kept = self.kept.borrow_mut();
        if let Some(at) = kept.iter().position(|(kept, _)| *kept == index) {
            let entry = kept.remove(at);
            let block = entry.1.clone();
            kept.push(entry);
            return Ok(block);
        }
        let block = self.blocks[index];
        let mut packed = self.packed.borrow_mut();
        packed.resize(block.packed as usize, 0);
        let mut file = self.file.borrow_mut();
        file.seek(SeekFrom::Start(block.file_offset))
            .map_err(|e| Error::malformed(format!("{}: {e}", self.label)))?;
        file.read_exact(&mut packed).map_err(|e| Error::malformed(format!("{}: {e}", self.label)))?;
        // A block that fell out of use lends its buffer to the next one.
        let mut unpacked = match kept.len() {
            KEPT_BLOCKS => Rc::try_unwrap(kept.remove(0).1).unwrap_or_default(),
            _ => Vec::new(),
        };
        unpacked.resize(block.unpacked as usize, 0);
        decompress_into(u32::from(block.flags) & COMPRESSION_MASK, &packed, &mut unpacked).map_err(|e| e.context(&self.label))?;
        let unpacked = Rc::new(unpacked);
        kept.push((index, unpacked.clone()));
        Ok(unpacked)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::BundleBuilder;

    #[test]
    fn reads_entries_across_compressed_and_stored_blocks() {
        let first: Vec<u8> = (0..300u32).map(|i| (i % 7) as u8).collect();
        let second = b"second file".to_vec();
        for (compress, block) in [(true, 64), (false, 50), (true, 1 << 17)] {
            let directory = tempfile::tempdir().unwrap();
            let path = directory.path().join("test.bundle");
            BundleBuilder::new()
                .compress(compress)
                .block_size(block)
                .file("CAB-one", &first)
                .file("CAB-two", &second)
                .write(&path);
            let bundle = Bundle::open(&path).unwrap();
            assert_eq!(bundle.unity_version(), "6000.4.8f1");
            assert_eq!(bundle.entries().iter().map(|e| e.name.as_str()).collect::<Vec<_>>(), ["CAB-one", "CAB-two"]);
            assert_eq!(bundle.read("CAB-one").unwrap(), first);
            assert_eq!(bundle.read("CAB-two").unwrap(), second);
            assert!(matches!(bundle.read("CAB-three"), Err(Error::Data(_))));
            // Any part, across block edges, in any order; nothing past the end.
            let stream = bundle.stream("CAB-one").unwrap();
            for (at, length) in [(290, 10), (0, 300), (63, 2), (128, 70), (1, 0), (299, 1)] {
                let mut part = vec![0; length];
                stream.read_at(at as u64, &mut part).unwrap();
                assert_eq!(part, first[at..at + length], "{at}+{length}");
            }
            assert!(stream.read_at(295, &mut [0; 6]).is_err());
            let mut tail = [0; 3];
            bundle.stream("CAB-two").unwrap().read_at(8, &mut tail).unwrap();
            assert_eq!(&tail, b"ile");
        }
    }

    #[test]
    fn refuses_what_it_cannot_read() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("bad.bundle");
        std::fs::write(&path, b"UnityWeb\0").unwrap();
        assert!(matches!(Bundle::open(&path), Err(Error::Malformed(_))));

        BundleBuilder::new().file("CAB-one", b"abc").write(&path);
        let mut bytes = std::fs::read(&path).unwrap();
        bytes.push(0);
        std::fs::write(&path, &bytes).unwrap();
        assert!(
            matches!(Bundle::open(&path), Err(Error::Malformed(m)) if m.contains("declares")),
            "a size that disagrees with the file"
        );

        BundleBuilder::new().lzma().file("CAB-one", b"abc").write(&path);
        assert!(matches!(Bundle::open(&path).and_then(|b| b.read("CAB-one")), Err(Error::Unsupported(m)) if m.contains("LZMA")));
    }
}
