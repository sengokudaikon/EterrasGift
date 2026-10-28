//! Type trees: the field layout a serialized file stores beside its objects, and the decoder that reads an object's
//! bytes with one. A decode must consume every byte of the object, so a layout that does not fit is caught rather than
//! read as plausible nonsense.

use crate::common_strings::COMMON_STRINGS;
use crate::error::{Error, Result};
use crate::reader::{Endian, Reader};
use crate::value::Value;
use std::sync::Arc;

/// Unity's `kAlignBytes`: after this node the stream moves to the next multiple of 4.
pub(crate) const ALIGN: u32 = 0x4000;
const COMMON_STRING: u32 = 0x8000_0000;
const MAX_NODES: usize = 1 << 20;

/// A `[SerializeReference]` type a serialized file declares, with its layout.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ReferenceType {
    pub class: String,
    pub namespace: String,
    pub assembly: String,
    pub tree: Arc<Node>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Node {
    pub type_name: Arc<str>,
    pub name: Arc<str>,
    pub byte_size: i32,
    pub meta_flag: u32,
    pub children: Vec<Node>,
}

impl Node {
    fn aligned(&self) -> bool {
        self.meta_flag & ALIGN != 0
    }

    /// `Array` under `vector`, `string`, `map` and friends: its children are the size and the element.
    fn array(&self) -> Option<&Node> {
        self.children.first().filter(|child| &*child.type_name == "Array" && child.children.len() == 2)
    }
}

fn common_string(offset: u32) -> Result<&'static str> {
    let mut at = 0usize;
    for text in COMMON_STRINGS {
        if at == offset as usize {
            return Ok(text);
        }
        at += text.len() + 1;
    }
    Err(Error::unsupported(format!("Unity common string at offset {offset}, newer than this reader")))
}

fn node_string(buffer: &[u8], value: u32) -> Result<Arc<str>> {
    if value & COMMON_STRING != 0 {
        return common_string(value & !COMMON_STRING).map(Arc::from);
    }
    let mut reader = Reader::new(buffer, Endian::Little);
    reader.seek(value as usize)?;
    reader.cstring().map(Arc::from)
}

/// Reads a type tree blob (serialized file formats 12–22): the node count, the string buffer size, fixed-size node
/// records in depth-first order with their levels, then the string buffer.
pub(crate) fn parse_blob(reader: &mut Reader<'_>, format: u32) -> Result<Node> {
    let record = if format >= 19 { 32 } else { 24 };
    let count = reader.count(record)?;
    let buffer_size = reader.count(0)?;
    if count == 0 || count > MAX_NODES {
        return Err(Error::malformed(format!("type tree with {count} nodes")));
    }
    let mut raw = Vec::with_capacity(count);
    for _ in 0..count {
        let (_version, level, _type_flags) = (reader.u16()?, reader.u8()?, reader.u8()?);
        let (type_offset, name_offset) = (reader.u32()?, reader.u32()?);
        let (byte_size, _index, meta_flag) = (reader.i32()?, reader.i32()?, reader.u32()?);
        if format >= 19 {
            reader.u64()?;
        }
        raw.push((level, type_offset, name_offset, byte_size, meta_flag));
    }
    let buffer = reader.take(buffer_size)?;
    let mut flat = Vec::with_capacity(count);
    for (level, type_offset, name_offset, byte_size, meta_flag) in raw {
        let node = Node {
            type_name: node_string(buffer, type_offset)?,
            name: node_string(buffer, name_offset)?,
            byte_size,
            meta_flag,
            children: Vec::new(),
        };
        flat.push((level, node));
    }
    if flat[0].0 != 0 || flat[1..].iter().any(|(level, _)| *level == 0) {
        return Err(Error::malformed("type tree without a single root"));
    }
    // Rebuild the tree from levels: a stack of open nodes, closed into their parents as the level drops.
    let mut stack: Vec<(u8, Node)> = Vec::new();
    for (level, node) in flat {
        while stack.last().is_some_and(|(open, _)| *open >= level) {
            close(&mut stack);
        }
        if stack.last().is_some_and(|(open, _)| level != open + 1) {
            return Err(Error::malformed(format!("type tree node {} skips a level", node.name)));
        }
        stack.push((level, node));
    }
    while stack.len() > 1 {
        close(&mut stack);
    }
    Ok(stack.pop().expect("a root").1)
}

fn close(stack: &mut Vec<(u8, Node)>) {
    let (_, node) = stack.pop().expect("an open node");
    stack.last_mut().expect("a parent").1.children.push(node);
}

/// Decodes a whole object. Fails unless the layout consumes exactly its bytes. `references` are the file's
/// `[SerializeReference]` types, for the objects a managed reference registry holds.
pub fn decode(root: &Node, bytes: &[u8], references: &[ReferenceType]) -> Result<Value> {
    let mut reader = Reader::new(bytes, Endian::Little);
    let value = read(root, &mut reader, references)?;
    if reader.remaining() != 0 {
        return Err(Error::data(format!("the {} layout read {} of {} bytes", root.type_name, reader.position(), reader.len())));
    }
    Ok(value)
}

/// Decodes the root's fields up to and including `last`, without the whole-object check. For a cheap look at the
/// leading fields (a MonoBehaviour's `m_Name`, say) before deciding to decode the rest.
pub fn decode_prefix(root: &Node, bytes: &[u8], last: &str, references: &[ReferenceType]) -> Result<Value> {
    let mut reader = Reader::new(bytes, Endian::Little);
    let mut fields = Vec::new();
    for child in &root.children {
        fields.push((child.name.clone(), read(child, &mut reader, references)?));
        if &*child.name == last {
            return Ok(Value::Object(fields));
        }
    }
    Err(Error::data(format!("the {} layout has no field {last}", root.type_name)))
}

fn read(node: &Node, reader: &mut Reader<'_>, references: &[ReferenceType]) -> Result<Value> {
    // A node with an `Array` child is a collection whatever it is called: Unity names some after their element
    // (`Element elements`), and an element can be a primitive. Strings keep their own reading.
    if let Some(array) = node.array().filter(|_| &*node.type_name != "string") {
        let value = read_array(node, array, reader, references)?;
        if node.aligned() {
            reader.align(4)?;
        }
        return Ok(value);
    }
    let value = match &*node.type_name {
        "SInt8" => Value::Int(reader.i8()?.into()),
        "UInt8" | "char" => Value::UInt(reader.u8()?.into()),
        "short" | "SInt16" => Value::Int(reader.i16()?.into()),
        "unsigned short" | "UInt16" => Value::UInt(reader.u16()?.into()),
        "int" | "SInt32" => Value::Int(reader.i32()?.into()),
        "unsigned int" | "UInt32" | "Type*" => Value::UInt(reader.u32()?.into()),
        "long long" | "SInt64" => Value::Int(reader.i64()?),
        "unsigned long long" | "UInt64" | "FileSize" => Value::UInt(reader.u64()?),
        "float" => Value::Float(reader.f32()?.into()),
        "double" => Value::Float(reader.f64()?),
        "bool" => Value::Bool(reader.bool()?),
        "string" => {
            let length = reader.count(1)?;
            let text = std::str::from_utf8(reader.take(length)?).map_err(|_| Error::malformed(format!("field {} is not UTF-8", node.name)))?;
            // Strings always end aligned, whatever their flags say.
            reader.align(4)?;
            Value::String(text.to_owned())
        }
        "TypelessData" => {
            let length = reader.count(1)?;
            Value::Bytes(reader.take(length)?.to_vec())
        }
        "pair" => match node.children.as_slice() {
            [first, second] => Value::Pair(Box::new((read(first, reader, references)?, read(second, reader, references)?))),
            _ => return Err(Error::malformed(format!("pair {} without two halves", node.name))),
        },
        "ReferencedObject" => read_reference(node, reader, references)?,
        "ReferencedObjectData" => return Err(Error::malformed(format!("reference data {} outside a ReferencedObject", node.name))),
        _ => match node.array() {
            Some(_) => unreachable!("collections are read above"),
            None => Value::Object(
                node.children
                    .iter()
                    .map(|child| Ok((child.name.clone(), read(child, reader, references).map_err(|e| e.context(&child.name))?)))
                    .collect::<Result<_>>()?,
            ),
        },
    };
    if node.aligned() {
        reader.align(4)?;
    }
    Ok(value)
}

/// One entry of a managed reference registry (format 2): its ID, its type's names, then data in that type's layout.
fn read_reference(node: &Node, reader: &mut Reader<'_>, references: &[ReferenceType]) -> Result<Value> {
    let mut fields: Vec<(Arc<str>, Value)> = Vec::new();
    for child in &node.children {
        let value = if &*child.type_name == "ReferencedObjectData" {
            let kind = fields
                .iter()
                .find(|(name, _)| &**name == "type")
                .map(|(_, v)| v)
                .ok_or_else(|| Error::malformed("reference data before its type"))?;
            let class = kind.field("class", Value::as_str)?;
            if class.is_empty() {
                // A null reference stores no data.
                Value::Object(Vec::new())
            } else {
                let (namespace, assembly) = (kind.field("ns", Value::as_str)?, kind.field("asm", Value::as_str)?);
                let layout = references
                    .iter()
                    .find(|r| r.class == class && r.namespace == namespace && r.assembly == assembly)
                    .ok_or_else(|| Error::data(format!("no layout for the referenced type {namespace}.{class}")))?;
                read(&layout.tree, reader, references)?
            }
        } else {
            read(child, reader, references)?
        };
        fields.push((child.name.clone(), value));
    }
    Ok(Value::Object(fields))
}

fn read_array(node: &Node, array: &Node, reader: &mut Reader<'_>, references: &[ReferenceType]) -> Result<Value> {
    let element = &array.children[1];
    let value = match &*element.type_name {
        // Byte runs stay bytes rather than one value per byte.
        "UInt8" | "SInt8" | "char" if !element.aligned() => {
            let length = reader.count(1)?;
            Value::Bytes(reader.take(length)?.to_vec())
        }
        _ => {
            let count = reader.count(element_floor(element))?;
            let mut items = Vec::with_capacity(count);
            for index in 0..count {
                items.push(read(element, reader, references).map_err(|e| e.context(format!("[{index}]")))?);
            }
            Value::Array(items)
        }
    };
    if array.aligned() && !node.aligned() {
        reader.align(4)?;
    }
    Ok(value)
}

/// The fewest bytes one element can take, so a corrupt count is refused before allocating for it.
fn element_floor(element: &Node) -> usize {
    match usize::try_from(element.byte_size) {
        Ok(size) if size > 0 => size,
        // Variable-size elements (strings, vectors, classes holding them) take at least their length field.
        _ => 4,
    }
    .min(4)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::{TreeBuilder, Writer};

    fn affix_tree() -> Node {
        let mut tree = TreeBuilder::new("MonoBehaviour", "Base");
        tree.pptr("m_GameObject").field("UInt8", "m_Enabled", 1, ALIGN).pptr("m_Script").string("m_Name");
        tree.open_vector("tiers", "Tier")
            .field("float", "minRoll", 4, 0)
            .field("float", "maxRoll", 4, 0)
            .close_vector();
        tree.field("bool", "flag", 1, ALIGN).field("UInt16", "id", 2, 0).field("SInt64", "big", 8, 0);
        tree.build()
    }

    fn affix_bytes(name: &str, tiers: &[(f32, f32)]) -> Vec<u8> {
        let mut w = Writer::default();
        w.i32(0).i64(0).u8(1).align().i32(1).i64(77).string(name);
        w.i32(tiers.len() as i32);
        for (min, max) in tiers {
            w.f32(*min).f32(*max);
        }
        w.u8(1).align().u16(25).i64(-3);
        w.bytes
    }

    #[test]
    fn decodes_fields_vectors_alignment_and_strings() {
        let tree = affix_tree();
        let value = decode(&tree, &affix_bytes("Health", &[(5.0, 15.0), (16.0, 25.0)]), &[]).unwrap();
        assert_eq!(value.get("m_Name").unwrap().as_str().unwrap(), "Health");
        assert_eq!(value.get("m_Script").unwrap().get("m_PathID").unwrap().as_i64().unwrap(), 77);
        let tiers = value.get("tiers").unwrap().as_array().unwrap();
        assert_eq!(tiers.len(), 2);
        assert_eq!(tiers[1].get("maxRoll").unwrap().as_f32().unwrap(), 25.0);
        assert!(value.get("flag").unwrap().as_bool().unwrap());
        assert_eq!(value.get("id").unwrap().int::<u16>().unwrap(), 25);
        assert_eq!(value.get("big").unwrap().as_i64().unwrap(), -3);
    }

    #[test]
    fn a_layout_that_does_not_fit_is_refused() {
        let tree = affix_tree();
        let mut bytes = affix_bytes("Health", &[(5.0, 15.0)]);
        bytes.extend_from_slice(&[0; 4]);
        assert!(matches!(decode(&tree, &bytes, &[]), Err(Error::Data(m)) if m.contains("read")), "bytes left over");
        let bytes = affix_bytes("Health", &[(5.0, 15.0)]);
        assert!(decode(&tree, &bytes[..bytes.len() - 1], &[]).is_err(), "bytes missing");
        let mut bytes = affix_bytes("Health", &[]);
        let count_at = 32 + 8;
        bytes[count_at..count_at + 4].copy_from_slice(&1_000_000i32.to_le_bytes());
        assert!(
            matches!(decode(&tree, &bytes, &[]), Err(Error::Malformed(m)) if m.contains("count")),
            "a count past the end"
        );
    }

    #[test]
    fn a_prefix_stops_at_the_named_field() {
        let tree = affix_tree();
        let bytes = affix_bytes("Health", &[(5.0, 15.0)]);
        let prefix = decode_prefix(&tree, &bytes[..40], "m_Name", &[]).unwrap();
        assert_eq!(prefix.get("m_Name").unwrap().as_str().unwrap(), "Health");
        assert!(decode_prefix(&tree, &bytes, "missing", &[]).is_err());
    }

    #[test]
    fn a_collection_named_after_its_primitive_element_is_still_a_collection() {
        // Unity's naming (and the rebuilt layouts') can call a `List<byte>` node `UInt8`.
        let mut tree = TreeBuilder::new("UniqueItem", "Base");
        tree.vector_of("subTypes", "UInt8", 1).field("UInt8", "baseType", 1, ALIGN);
        let mut tree = tree.build();
        tree.children[0].type_name = "UInt8".into();
        let mut w = Writer::default();
        w.i32(2).u8(5).u8(6).align().u8(9).align();
        let value = decode(&tree, &w.bytes, &[]).unwrap();
        assert_eq!(value.get("subTypes").unwrap(), &Value::Bytes(vec![5, 6]));
        assert_eq!(value.get("baseType").unwrap().as_i64().unwrap(), 9);
    }

    #[test]
    fn managed_references_take_their_layout_from_the_reference_types() {
        let mut tree = TreeBuilder::new("StringTable", "Base");
        tree.open_class("ManagedReferencesRegistry", "references").field("int", "version", 4, 0);
        tree.open_vector("RefIds", "ReferencedObject").field("SInt64", "rid", 8, 0);
        tree.open_class("ReferencedManagedType", "type").string("class").string("ns").string("asm").close_class();
        tree.field("ReferencedObjectData", "data", -1, 0).close_vector().close_class();
        let tree = tree.build();
        let mut tag = TreeBuilder::new("SmartFormatTag", "Base");
        tag.field("int", "value", 4, 0);
        let references = [ReferenceType {
            class: "SmartFormatTag".into(),
            namespace: "UnityEngine.Localization.Metadata".into(),
            assembly: "Unity.Localization".into(),
            tree: Arc::new(tag.build()),
        }];
        let mut w = Writer::default();
        w.i32(2).i32(2);
        w.i64(0)
            .string("SmartFormatTag")
            .string("UnityEngine.Localization.Metadata")
            .string("Unity.Localization")
            .i32(5);
        w.i64(1).string("").string("").string("");
        let value = decode(&tree, &w.bytes, &references).unwrap();
        let ids = value.get("references").unwrap().get("RefIds").unwrap().as_array().unwrap();
        assert_eq!(ids[0].get("data").unwrap().get("value").unwrap().as_i64().unwrap(), 5);
        assert_eq!(ids[1].get("data").unwrap(), &Value::Object(Vec::new()), "a null reference stores no data");
        assert!(decode(&tree, &w.bytes, &[]).unwrap_err().to_string().contains("no layout for the referenced type"));
    }

    #[test]
    fn blobs_round_trip_with_common_and_local_strings() {
        let tree = affix_tree();
        let mut w = Writer::default();
        TreeBuilder::write_blob(&tree, &mut w);
        let mut reader = Reader::new(&w.bytes, Endian::Little);
        assert_eq!(parse_blob(&mut reader, 22).unwrap(), tree);
        assert_eq!(reader.remaining(), 0);
        assert_eq!(common_string(0).unwrap(), "AABB");
        assert!(common_string(1).is_err(), "only offsets that start a string");
    }
}
