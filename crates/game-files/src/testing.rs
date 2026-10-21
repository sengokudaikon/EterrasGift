//! Writers for small, real bundles, serialized files and type trees, so tests read bytes in the formats the game ships
//! without the game.

use crate::common_strings::COMMON_STRINGS;
use crate::serialized::{MONO_BEHAVIOUR, MONO_SCRIPT};
use crate::typetree::Node;
use std::path::Path;
use std::sync::Arc;

/// Little-endian object bytes, aligned as Unity aligns them.
#[derive(Default)]
pub(crate) struct Writer {
    pub(crate) bytes: Vec<u8>,
}

impl Writer {
    pub(crate) fn u8(&mut self, v: u8) -> &mut Self {
        self.bytes.push(v);
        self
    }
    pub(crate) fn u16(&mut self, v: u16) -> &mut Self {
        self.bytes.extend_from_slice(&v.to_le_bytes());
        self
    }
    pub(crate) fn i16(&mut self, v: i16) -> &mut Self {
        self.bytes.extend_from_slice(&v.to_le_bytes());
        self
    }
    pub(crate) fn i32(&mut self, v: i32) -> &mut Self {
        self.bytes.extend_from_slice(&v.to_le_bytes());
        self
    }
    pub(crate) fn u32(&mut self, v: u32) -> &mut Self {
        self.bytes.extend_from_slice(&v.to_le_bytes());
        self
    }
    pub(crate) fn i64(&mut self, v: i64) -> &mut Self {
        self.bytes.extend_from_slice(&v.to_le_bytes());
        self
    }
    pub(crate) fn f32(&mut self, v: f32) -> &mut Self {
        self.bytes.extend_from_slice(&v.to_le_bytes());
        self
    }
    pub(crate) fn raw(&mut self, v: &[u8]) -> &mut Self {
        self.bytes.extend_from_slice(v);
        self
    }
    pub(crate) fn cstring(&mut self, v: &str) -> &mut Self {
        self.raw(v.as_bytes()).u8(0)
    }
    pub(crate) fn align_to(&mut self, to: usize) -> &mut Self {
        while !self.bytes.len().is_multiple_of(to) {
            self.bytes.push(0);
        }
        self
    }
    pub(crate) fn align(&mut self) -> &mut Self {
        self.align_to(4)
    }
    pub(crate) fn string(&mut self, v: &str) -> &mut Self {
        self.i32(v.len() as i32).raw(v.as_bytes()).align()
    }
}

fn node(type_name: &str, name: &str, byte_size: i32, meta_flag: u32) -> Node {
    Node {
        type_name: Arc::from(type_name),
        name: Arc::from(name),
        byte_size,
        meta_flag,
        children: Vec::new(),
    }
}

/// Builds type trees the way Unity lays out the common shapes.
pub(crate) struct TreeBuilder {
    stack: Vec<Node>,
}

impl TreeBuilder {
    pub(crate) fn new(type_name: &str, name: &str) -> Self {
        Self {
            stack: vec![node(type_name, name, -1, 0x8000)],
        }
    }

    fn add(&mut self, child: Node) -> &mut Self {
        self.stack.last_mut().expect("open node").children.push(child);
        self
    }

    pub(crate) fn field(&mut self, type_name: &str, name: &str, size: i32, flag: u32) -> &mut Self {
        self.add(node(type_name, name, size, flag))
    }

    pub(crate) fn pptr(&mut self, name: &str) -> &mut Self {
        let mut pointer = node("PPtr<Object>", name, 12, 0);
        pointer.children = vec![node("int", "m_FileID", 4, 0), node("SInt64", "m_PathID", 8, 0)];
        self.add(pointer)
    }

    pub(crate) fn string(&mut self, name: &str) -> &mut Self {
        let mut array = node("Array", "Array", -1, 0x4001);
        array.children = vec![node("int", "size", 4, 1), node("char", "data", 1, 1)];
        let mut string = node("string", name, -1, 0x8000);
        string.children = vec![array];
        self.add(string)
    }

    /// A `vector` of `element_type` objects; the fields added next go into the element until `close_vector`.
    pub(crate) fn open_vector(&mut self, name: &str, element_type: &str) -> &mut Self {
        let mut vector = node("vector", name, -1, 0);
        let mut array = node("Array", "Array", -1, 0);
        array.children.push(node("int", "size", 4, 0));
        vector.children.push(array);
        self.stack.push(vector);
        self.stack.push(node(element_type, "data", -1, 0));
        self
    }

    pub(crate) fn close_vector(&mut self) -> &mut Self {
        let element = self.stack.pop().expect("element");
        let mut vector = self.stack.pop().expect("vector");
        vector.children[0].children.push(element);
        self.add(vector)
    }

    /// A nested class; the fields added next go into it until `close_class`.
    pub(crate) fn open_class(&mut self, type_name: &str, name: &str) -> &mut Self {
        self.stack.push(node(type_name, name, -1, 0));
        self
    }

    pub(crate) fn close_class(&mut self) -> &mut Self {
        let class = self.stack.pop().expect("class");
        self.add(class)
    }

    /// A `vector` of a primitive.
    pub(crate) fn vector_of(&mut self, name: &str, element_type: &str, size: i32) -> &mut Self {
        let mut array = node("Array", "Array", -1, 0x4000);
        array.children = vec![node("int", "size", 4, 0), node(element_type, "data", size, 0)];
        let mut vector = node("vector", name, -1, 0);
        vector.children = vec![array];
        self.add(vector)
    }

    pub(crate) fn build(&mut self) -> Node {
        assert_eq!(self.stack.len(), 1, "every vector closed");
        self.stack[0].clone()
    }

    pub(crate) fn write_blob(root: &Node, out: &mut Writer) {
        fn flatten<'a>(node: &'a Node, level: u8, out: &mut Vec<(u8, &'a Node)>) {
            out.push((level, node));
            for child in &node.children {
                flatten(child, level + 1, out);
            }
        }
        let mut flat = Vec::new();
        flatten(root, 0, &mut flat);
        let mut local = Vec::new();
        let mut offset_of = |text: &str| -> u32 {
            let mut at = 0u32;
            for common in COMMON_STRINGS {
                if *common == text {
                    return at | 0x8000_0000;
                }
                at += common.len() as u32 + 1;
            }
            let offset = local.len() as u32;
            local.extend_from_slice(text.as_bytes());
            local.push(0);
            offset
        };
        let records: Vec<_> = flat
            .iter()
            .map(|(level, node)| (*level, offset_of(&node.type_name), offset_of(&node.name), node.byte_size, node.meta_flag))
            .collect();
        out.i32(records.len() as i32).i32(local.len() as i32);
        for (index, (level, type_offset, name_offset, size, flag)) in records.into_iter().enumerate() {
            out.u16(1).u8(level).u8(0).u32(type_offset).u32(name_offset).i32(size).i32(index as i32).u32(flag).i64(0);
        }
        out.raw(&local);
    }
}

/// A format-22 serialized file.
pub(crate) struct SerializedBuilder {
    type_trees: bool,
    types: Vec<(i32, Node)>,
    objects: Vec<(i64, i32, Vec<u8>)>,
    externals: Vec<String>,
}

impl SerializedBuilder {
    pub(crate) fn new() -> Self {
        Self {
            type_trees: true,
            types: Vec::new(),
            objects: Vec::new(),
            externals: Vec::new(),
        }
    }
    pub(crate) fn without_type_trees(mut self) -> Self {
        self.type_trees = false;
        self
    }
    pub(crate) fn external(mut self, path: &str) -> Self {
        self.externals.push(path.to_owned());
        self
    }
    pub(crate) fn typed(mut self, class_id: i32, tree: &Node) -> Self {
        self.types.push((class_id, tree.clone()));
        self
    }
    pub(crate) fn mono_behaviour(self, tree: &Node) -> Self {
        self.typed(MONO_BEHAVIOUR, tree)
    }
    pub(crate) fn mono_script(self, tree: &Node) -> Self {
        self.typed(MONO_SCRIPT, tree)
    }
    pub(crate) fn object(mut self, path_id: i64, type_index: i32, bytes: &[u8]) -> Self {
        self.objects.push((path_id, type_index, bytes.to_vec()));
        self
    }

    pub(crate) fn build(self) -> Vec<u8> {
        let mut w = Writer::default();
        // The 48-byte header; sizes are patched in once known.
        w.raw(&[0; 8])
            .raw(&22u32.to_be_bytes())
            .raw(&[0; 4])
            .u8(0)
            .raw(&[0; 3])
            .raw(&[0; 4])
            .raw(&[0; 8])
            .raw(&[0; 8])
            .raw(&[0; 8]);
        w.cstring("6000.4.8f1").i32(19).u8(self.type_trees.into()).i32(self.types.len() as i32);
        for (class_id, tree) in &self.types {
            w.i32(*class_id).u8(0).i16(if *class_id == MONO_BEHAVIOUR { 0 } else { -1 });
            if *class_id == MONO_BEHAVIOUR {
                w.raw(&[0; 16]);
            }
            w.raw(&[0; 16]);
            if self.type_trees {
                TreeBuilder::write_blob(tree, &mut w);
                w.i32(0);
            }
        }
        w.i32(self.objects.len() as i32);
        let mut start = 0usize;
        let mut placed = Vec::new();
        for (path_id, type_index, bytes) in &self.objects {
            w.align().i64(*path_id).i64(start as i64).u32(bytes.len() as u32).i32(*type_index);
            placed.push(start);
            start = (start + bytes.len()).div_ceil(8) * 8;
        }
        w.i32(0).i32(self.externals.len() as i32);
        for path in &self.externals {
            w.cstring("").raw(&[0; 16]).i32(0).cstring(path);
        }
        w.i32(0).cstring("");
        let metadata_end = w.bytes.len();
        w.align_to(16);
        let data_offset = w.bytes.len();
        for ((_, _, bytes), at) in self.objects.iter().zip(placed) {
            w.bytes.resize(data_offset + at, 0);
            w.raw(bytes);
        }
        let file_size = w.bytes.len();
        w.bytes[20..24].copy_from_slice(&((metadata_end - 48) as u32).to_be_bytes());
        w.bytes[24..32].copy_from_slice(&(file_size as i64).to_be_bytes());
        w.bytes[32..40].copy_from_slice(&(data_offset as i64).to_be_bytes());
        w.bytes
    }
}

/// A UnityFS format-8 bundle with its block table at the front, padded as Unity 6000 writes it.
pub(crate) struct BundleBuilder {
    compress: bool,
    lzma: bool,
    block_size: usize,
    files: Vec<(String, Vec<u8>)>,
}

impl BundleBuilder {
    pub(crate) fn new() -> Self {
        Self {
            compress: true,
            lzma: false,
            block_size: 1 << 17,
            files: Vec::new(),
        }
    }
    pub(crate) fn compress(mut self, compress: bool) -> Self {
        self.compress = compress;
        self
    }
    /// Marks the data blocks LZMA (their bytes stay stored), for the refusal path.
    pub(crate) fn lzma(mut self) -> Self {
        self.lzma = true;
        self
    }
    pub(crate) fn block_size(mut self, size: usize) -> Self {
        self.block_size = size;
        self
    }
    pub(crate) fn file(mut self, name: &str, bytes: &[u8]) -> Self {
        self.files.push((name.to_owned(), bytes.to_vec()));
        self
    }

    pub(crate) fn write(self, path: &Path) {
        let stream: Vec<u8> = self.files.iter().flat_map(|(_, bytes)| bytes.iter().copied()).collect();
        let mut blocks = Vec::new();
        for chunk in stream.chunks(self.block_size.max(1)) {
            let (flag, data) = match (self.lzma, self.compress) {
                (true, _) => (1u16, chunk.to_vec()),
                (false, true) => (3, lz4_flex::block::compress(chunk)),
                (false, false) => (0, chunk.to_vec()),
            };
            blocks.push((chunk.len() as u32, flag, data));
        }
        let mut table = Vec::new();
        table.extend_from_slice(&[0; 16]);
        table.extend_from_slice(&(blocks.len() as i32).to_be_bytes());
        for (unpacked, flag, data) in &blocks {
            table.extend_from_slice(&unpacked.to_be_bytes());
            table.extend_from_slice(&(data.len() as u32).to_be_bytes());
            table.extend_from_slice(&flag.to_be_bytes());
        }
        table.extend_from_slice(&(self.files.len() as i32).to_be_bytes());
        let mut offset = 0i64;
        for (name, bytes) in &self.files {
            table.extend_from_slice(&offset.to_be_bytes());
            table.extend_from_slice(&(bytes.len() as i64).to_be_bytes());
            table.extend_from_slice(&4u32.to_be_bytes());
            table.extend_from_slice(name.as_bytes());
            table.push(0);
            offset += bytes.len() as i64;
        }
        let (table_compression, packed_table) = if self.compress { (3u32, lz4_flex::block::compress(&table)) } else { (0, table.clone()) };
        let mut w = Writer::default();
        w.cstring("UnityFS").raw(&8u32.to_be_bytes()).cstring("5.x.x").cstring("6000.4.8f1");
        let size_at = w.bytes.len();
        w.raw(&[0; 8]).raw(&(packed_table.len() as u32).to_be_bytes()).raw(&(table.len() as u32).to_be_bytes());
        w.raw(&(0x40 | 0x200 | table_compression).to_be_bytes());
        w.align_to(16).raw(&packed_table).align_to(16);
        for (_, _, data) in &blocks {
            w.raw(data);
        }
        let size = w.bytes.len() as i64;
        w.bytes[size_at..size_at + 8].copy_from_slice(&size.to_be_bytes());
        std::fs::write(path, &w.bytes).unwrap();
    }
}
