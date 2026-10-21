//! Unity serialized files (formats 17–22, Unity 2017.3–6000): a header, metadata (types with optional type trees,
//! the object table, script references, external files), then the objects' bytes.

use crate::error::{Error, Result};
use crate::reader::{Endian, Reader};
use crate::typetree::{self, Node, ReferenceType};
use crate::value::Value;
use std::sync::Arc;

pub const MONO_BEHAVIOUR: i32 = 114;
pub const MONO_SCRIPT: i32 = 115;

#[derive(Clone, Debug)]
pub struct SerializedType {
    pub class_id: i32,
    /// The type tree, when the file was written with them (bundles are; player data usually is not).
    pub tree: Option<Arc<Node>>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Object {
    pub path_id: i64,
    pub class_id: i32,
    type_index: usize,
    start: usize,
    size: usize,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct External {
    /// As Unity writes it: `archive:/CAB-…/CAB-…` for a file in a bundle, or a plain file name.
    pub path: String,
}

impl External {
    /// The file name the path ends in (`CAB-…`, `globalgamemanagers.assets`).
    pub fn file_name(&self) -> &str {
        self.path.rsplit(['/', '\\']).next().unwrap_or(&self.path)
    }
}

pub struct SerializedFile {
    pub format: u32,
    pub unity_version: String,
    types: Vec<SerializedType>,
    references: Vec<ReferenceType>,
    objects: Vec<Object>,
    pub externals: Vec<External>,
    bytes: Vec<u8>,
}

/// A type entry; a reference type also carries its class, namespace and assembly names.
fn read_type(reader: &mut Reader<'_>, format: u32, type_trees: bool, reference: bool) -> Result<(SerializedType, Option<[String; 3]>)> {
    let class_id = reader.i32()?;
    reader.bool()?;
    let script_index = reader.i16()?;
    if (reference && script_index >= 0) || class_id == MONO_BEHAVIOUR {
        reader.take(16)?;
    }
    reader.take(16)?;
    let (mut tree, mut names) = (None, None);
    if type_trees {
        tree = Some(Arc::new(typetree::parse_blob(reader, format)?));
        if format >= 21 {
            if reference {
                names = Some([reader.cstring()?, reader.cstring()?, reader.cstring()?]);
            } else {
                let count = reader.count(4)?;
                reader.take(count * 4)?;
            }
        }
    }
    Ok((SerializedType { class_id, tree }, names))
}

impl SerializedFile {
    pub fn parse(bytes: Vec<u8>) -> Result<SerializedFile> {
        let (format, unity_version, types, references, objects, externals) = Self::parse_metadata(&bytes)?;
        Ok(SerializedFile {
            format,
            unity_version,
            types,
            references,
            objects,
            externals,
            bytes,
        })
    }

    #[allow(clippy::type_complexity)]
    fn parse_metadata(bytes: &[u8]) -> Result<(u32, String, Vec<SerializedType>, Vec<ReferenceType>, Vec<Object>, Vec<External>)> {
        let mut reader = Reader::new(bytes, Endian::Big);
        let (_metadata_size, _file_size, format, early_data_offset) = (reader.u32()?, reader.u32()?, reader.u32()?, reader.u32()?);
        if !(17..=22).contains(&format) {
            return Err(Error::unsupported(format!("serialized file format {format}")));
        }
        let big_endian = reader.bool()?;
        reader.take(3)?;
        let data_offset = if format >= 22 {
            let (_metadata_size, file_size, data_offset, _reserved) = (reader.u32()?, reader.i64()?, reader.i64()?, reader.i64()?);
            if usize::try_from(file_size).ok() != Some(bytes.len()) {
                return Err(Error::malformed(format!("serialized file declares {file_size} bytes, has {}", bytes.len())));
            }
            data_offset
        } else {
            i64::from(early_data_offset)
        };
        let data_offset = usize::try_from(data_offset).map_err(|_| Error::malformed("negative data offset"))?;
        reader.endian = if big_endian { Endian::Big } else { Endian::Little };
        let unity_version = reader.cstring()?;
        let _platform = reader.i32()?;
        let type_trees = reader.bool()?;
        let type_count = reader.count(23)?;
        let types = (0..type_count)
            .map(|_| read_type(&mut reader, format, type_trees, false).map(|(t, _)| t))
            .collect::<Result<Vec<_>>>()?;

        let object_count = reader.count(20)?;
        let mut objects = Vec::with_capacity(object_count);
        for _ in 0..object_count {
            reader.align(4)?;
            let path_id = reader.i64()?;
            let start = if format >= 22 { reader.i64()? } else { reader.u32()?.into() };
            let size = reader.u32()? as usize;
            let type_index = reader.i32()?;
            let type_index = usize::try_from(type_index)
                .ok()
                .filter(|&i| i < types.len())
                .ok_or_else(|| Error::malformed(format!("object {path_id} has type {type_index}")))?;
            let start = usize::try_from(start)
                .ok()
                .and_then(|s| s.checked_add(data_offset))
                .ok_or_else(|| Error::malformed(format!("object {path_id} starts out of range")))?;
            if start.checked_add(size).is_none_or(|end| end > bytes.len()) {
                return Err(Error::malformed(format!("object {path_id} runs past the end of the file")));
            }
            objects.push(Object {
                path_id,
                class_id: types[type_index].class_id,
                type_index,
                start,
                size,
            });
        }
        let script_count = reader.count(12)?;
        for _ in 0..script_count {
            reader.i32()?;
            reader.align(4)?;
            reader.i64()?;
        }
        let external_count = reader.count(22)?;
        let mut externals = Vec::with_capacity(external_count);
        for _ in 0..external_count {
            reader.cstring()?;
            reader.take(16)?;
            reader.i32()?;
            externals.push(External { path: reader.cstring()? });
        }
        let mut references = Vec::new();
        if format >= 20 {
            let count = reader.count(23)?;
            for _ in 0..count {
                if let (SerializedType { tree: Some(tree), .. }, Some([class, namespace, assembly])) = read_type(&mut reader, format, type_trees, true)? {
                    references.push(ReferenceType {
                        class,
                        namespace,
                        assembly,
                        tree: (*tree).clone(),
                    });
                }
            }
        }
        // User information follows; nothing here needs it.
        Ok((format, unity_version, types, references, objects, externals))
    }

    pub fn objects(&self) -> &[Object] {
        &self.objects
    }

    pub fn object(&self, path_id: i64) -> Option<&Object> {
        self.objects.iter().find(|object| object.path_id == path_id)
    }

    pub fn bytes(&self, object: &Object) -> &[u8] {
        &self.bytes[object.start..object.start + object.size]
    }

    pub fn tree(&self, object: &Object) -> Result<&Arc<Node>> {
        self.types[object.type_index]
            .tree
            .as_ref()
            .ok_or_else(|| Error::unsupported(format!("object {} has no type tree", object.path_id)))
    }

    /// The whole object, which must fill its bytes exactly.
    pub fn decode(&self, object: &Object) -> Result<Value> {
        typetree::decode(self.tree(object)?, self.bytes(object), &self.references).map_err(|e| e.context(format!("object {}", object.path_id)))
    }

    /// The object's leading fields, through `last`.
    pub fn decode_prefix(&self, object: &Object, last: &str) -> Result<Value> {
        typetree::decode_prefix(self.tree(object)?, self.bytes(object), last, &self.references).map_err(|e| e.context(format!("object {}", object.path_id)))
    }

    /// The whole object read with a layout from elsewhere (for files written without type trees).
    pub fn decode_with(&self, object: &Object, tree: &Node) -> Result<Value> {
        typetree::decode(tree, self.bytes(object), &self.references).map_err(|e| e.context(format!("object {}", object.path_id)))
    }

    /// The object's leading fields through `last`, read with a layout from elsewhere.
    pub fn decode_prefix_with(&self, object: &Object, tree: &Node, last: &str) -> Result<Value> {
        typetree::decode_prefix(tree, self.bytes(object), last, &self.references).map_err(|e| e.context(format!("object {}", object.path_id)))
    }

    /// The file a `PPtr`'s file ID names: `None` for this file, else the external.
    pub fn external(&self, file_id: i32) -> Result<Option<&External>> {
        match file_id {
            0 => Ok(None),
            id => usize::try_from(id - 1)
                .ok()
                .and_then(|index| self.externals.get(index))
                .map(Some)
                .ok_or_else(|| Error::data(format!("PPtr file {id} is not among {} externals", self.externals.len()))),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::{SerializedBuilder, TreeBuilder, Writer};

    #[test]
    fn reads_types_objects_and_externals() {
        let mut tree = TreeBuilder::new("MonoBehaviour", "Base");
        tree.pptr("m_GameObject")
            .field("UInt8", "m_Enabled", 1, 0x4000)
            .pptr("m_Script")
            .string("m_Name")
            .field("int", "value", 4, 0);
        let tree = tree.build();
        let mut object = Writer::default();
        object.i32(0).i64(0).u8(1).align().i32(1).i64(42).string("First").i32(7);
        let file = SerializedBuilder::new()
            .external("archive:/CAB-monoscripts/CAB-monoscripts")
            .mono_behaviour(&tree)
            .object(10, 0, &object.bytes)
            .object(11, 0, &object.bytes)
            .build();
        let file = SerializedFile::parse(file).unwrap();
        assert_eq!(file.unity_version, "6000.4.8f1");
        assert_eq!(file.objects().len(), 2);
        let first = *file.object(10).unwrap();
        assert_eq!(first.class_id, MONO_BEHAVIOUR);
        let value = file.decode(&first).unwrap();
        assert_eq!(value.get("value").unwrap().as_i64().unwrap(), 7);
        let (file_id, path_id) = value.get("m_Script").unwrap().as_pptr().unwrap();
        assert_eq!((file_id, path_id), (1, 42));
        assert_eq!(file.external(file_id).unwrap().unwrap().file_name(), "CAB-monoscripts");
        assert!(file.external(0).unwrap().is_none());
        assert!(file.external(2).is_err());
        assert_eq!(file.decode_prefix(&first, "m_Name").unwrap().get("m_Name").unwrap().as_str().unwrap(), "First");
    }

    #[test]
    fn refuses_other_formats_and_missing_trees() {
        let mut bytes = SerializedBuilder::new().build();
        bytes[8..12].copy_from_slice(&23u32.to_be_bytes());
        assert!(matches!(SerializedFile::parse(bytes), Err(Error::Unsupported(_))));
        let file = SerializedFile::parse(
            SerializedBuilder::new()
                .without_type_trees()
                .mono_behaviour(&TreeBuilder::new("MonoBehaviour", "Base").build())
                .object(1, 0, &[])
                .build(),
        )
        .unwrap();
        assert!(matches!(file.decode(&file.objects()[0]), Err(Error::Unsupported(m)) if m.contains("type tree")));
    }
}
