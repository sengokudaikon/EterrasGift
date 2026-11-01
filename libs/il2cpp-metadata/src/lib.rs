//! An IL2CPP build read as data: `global-metadata.dat` (format 39) for names, types and fields, the game assembly for the
//! types those fields have. Together they say what a class looks like without running it, for example to rebuild how a
//! serializer laid out an object stored without a type tree.

#![forbid(unsafe_code)]

mod binary;
mod metadata;

pub use binary::kind;
use binary::{Binary, RawType};
use binary_reader::{Error, Result};
use metadata::{FieldDefinition, Metadata};
pub use metadata::{TypeDefinition, type_attributes};
use std::path::Path;

/// A field's type, resolved from the binary.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Ty {
    /// `bool`, `char`, the integers, `float`, `double`, `IntPtr`, `UIntPtr`: the `Il2CppTypeEnum` kind.
    Primitive(u8),
    String,
    Object,
    Definition {
        index: u32,
    },
    Generic {
        definition: u32,
        arguments: Vec<Ty>,
    },
    /// A one-dimensional, zero-based array (`T[]`).
    Vector(Box<Ty>),
    /// A multi-dimensional array.
    Array(Box<Ty>, u8),
    /// A generic parameter of the enclosing type, by position.
    Parameter(u16),
    /// Anything a serialized field cannot be: pointers, method parameters, by-refs.
    Other(u8),
}

/// `FieldAttributes` bits used here.
pub mod field_attributes {
    pub const ACCESS: u16 = 0x7;
    pub const PUBLIC: u16 = 0x6;
    pub const STATIC: u16 = 0x10;
    pub const INIT_ONLY: u16 = 0x20;
    pub const LITERAL: u16 = 0x40;
    pub const NOT_SERIALIZED: u16 = 0x80;
}

pub struct Field<'a> {
    pub name: &'a str,
    pub ty: Ty,
    pub attributes: u16,
    token: u32,
}

pub struct Il2Cpp {
    metadata: Metadata,
    binary: Binary,
}

impl Il2Cpp {
    pub fn open(game_assembly: &Path, metadata: &Path) -> Result<Il2Cpp> {
        let metadata = Metadata::open(metadata).map_err(|e| e.context("global-metadata.dat"))?;
        let binary = Binary::open(game_assembly, metadata.types.len()).map_err(|e| e.context("GameAssembly.dll"))?;
        Ok(Il2Cpp { metadata, binary })
    }

    pub fn definition(&self, index: u32) -> Result<&TypeDefinition> {
        self.metadata.type_definition(index)
    }

    fn resolve(&self, raw: RawType) -> Result<Ty> {
        if raw.by_ref {
            return Ok(Ty::Other(kind::PTR));
        }
        Ok(match raw.kind {
            kind::BOOLEAN | kind::CHAR | kind::I1 | kind::U1 | kind::I2 | kind::U2 | kind::I4 | kind::U4 | kind::I8 | kind::U8 | kind::R4 | kind::R8 | kind::I | kind::U => {
                Ty::Primitive(raw.kind)
            }
            kind::STRING => Ty::String,
            kind::OBJECT => Ty::Object,
            kind::CLASS | kind::VALUE_TYPE => Ty::Definition {
                index: self.definition_index(raw)?,
            },
            kind::GENERIC_INSTANCE => {
                let (definition, arguments) = self.binary.generic_class(raw.data)?;
                Ty::Generic {
                    definition: self.definition_index(definition)?,
                    arguments: arguments.into_iter().map(|a| self.resolve(a)).collect::<Result<_>>()?,
                }
            }
            kind::SZ_ARRAY => Ty::Vector(Box::new(self.resolve(self.binary.type_from(raw.data)?)?)),
            kind::ARRAY => {
                let (element, rank) = self.binary.array(raw.data)?;
                Ty::Array(Box::new(self.resolve(element)?), rank)
            }
            kind::VAR => Ty::Parameter(
                self.metadata
                    .generic_position(u32::try_from(raw.data).map_err(|_| Error::malformed("generic parameter index out of range"))?)?,
            ),
            other => Ty::Other(other),
        })
    }

    fn definition_index(&self, raw: RawType) -> Result<u32> {
        match raw.kind {
            kind::CLASS | kind::VALUE_TYPE => {
                let index = u32::try_from(raw.data).map_err(|_| Error::malformed("type definition index out of range"))?;
                self.metadata.type_definition(index)?;
                Ok(index)
            }
            other => Err(Error::malformed(format!("expected a class or value type, found kind {other:#x}"))),
        }
    }

    /// The type a metadata type index names.
    pub fn type_at(&self, index: u32) -> Result<Ty> {
        self.resolve(self.binary.type_at(index)?)
    }

    /// `Namespace.Name`, with `Outer/Inner` for nested types, as Cecil names them.
    pub fn full_name(&self, index: u32) -> Result<String> {
        let definition = self.definition(index)?;
        if let Some(declaring) = definition.declaring_type {
            let Ty::Definition { index: outer } = self.type_at(declaring)? else {
                return Err(Error::malformed(format!("{} has a declaring type that is not a definition", definition.name)));
            };
            return Ok(format!("{}/{}", self.full_name(outer)?, definition.name));
        }
        Ok(if definition.namespace.is_empty() {
            definition.name.clone()
        } else {
            format!("{}.{}", definition.namespace, definition.name)
        })
    }

    /// A type definition's own fields, in declaration order, with their types.
    pub fn fields(&self, index: u32) -> Result<Vec<Field<'_>>> {
        let definition = self.definition(index)?;
        self.metadata
            .fields_of(definition)
            .iter()
            .map(|f: &FieldDefinition| {
                let raw = self.binary.type_at(f.type_index)?;
                Ok(Field {
                    name: &f.name,
                    ty: self.resolve(raw).map_err(|e| e.context(format!("{}.{}", definition.name, f.name)))?,
                    attributes: raw.attributes,
                    token: f.token,
                })
            })
            .collect()
    }

    /// The full names of the custom attributes on a type definition, or on one of its fields.
    pub fn attributes(&self, definition: u32, field: Option<&Field<'_>>) -> Result<Vec<String>> {
        let image = self
            .metadata
            .image_of(definition)
            .ok_or_else(|| Error::malformed(format!("type definition {definition} belongs to no image")))?;
        let token = field.map_or(self.definition(definition)?.token, |f| f.token);
        self.metadata.attribute_types(image, token)?.into_iter().map(|owner| self.full_name(owner)).collect()
    }

    /// The value of an `Int32` constant (`const int`) of `assembly`'s `namespace.class`, from its metadata default.
    pub fn int_constant(&self, assembly: &str, namespace: &str, class: &str, field: &str) -> Result<i32> {
        let definition = self.definition(self.find_one(assembly, namespace, class)?)?;
        let field_index = self
            .metadata
            .field_index(definition, field)
            .ok_or_else(|| Error::data(format!("{class} has no field {field}")))?;
        let raw = self.binary.type_at(self.metadata.fields[field_index as usize].type_index)?;
        if raw.kind != kind::I4 || raw.attributes & field_attributes::LITERAL == 0 || raw.attributes & field_attributes::STATIC == 0 {
            return Err(Error::data(format!("{class}.{field} is not a const int")));
        }
        let (type_index, value) = self.metadata.int_default(field_index)?;
        if self.binary.type_at(type_index)?.kind != kind::I4 {
            return Err(Error::data(format!("{class}.{field}'s default is not an Int32")));
        }
        Ok(value)
    }

    /// A `byte` enum's names by value, from its literals' metadata defaults (a `byte` default is the raw byte).
    pub fn byte_enum(&self, assembly: &str, namespace: &str, class: &str) -> Result<std::collections::BTreeMap<u8, String>> {
        let index = self.find_one(assembly, namespace, class)?;
        let definition = self.definition(index)?;
        if !definition.is_enum {
            return Err(Error::data(format!("{class} is not an enum")));
        }
        let mut values = std::collections::BTreeMap::new();
        for (position, field) in self.metadata.fields_of(definition).iter().enumerate() {
            let raw = self.binary.type_at(field.type_index)?;
            if raw.attributes & field_attributes::LITERAL == 0 {
                if raw.kind != kind::U1 {
                    return Err(Error::data(format!("{class} is not a byte enum")));
                }
                continue;
            }
            let value = self.metadata.byte_default(definition.first_field + position as u32)?;
            values.insert(value, field.name.clone());
        }
        Ok(values)
    }

    /// The one type definition named `namespace.name` (not nested) in the image `assembly`.dll: none or several is an
    /// error.
    pub fn find_one(&self, assembly: &str, namespace: &str, name: &str) -> Result<u32> {
        match self.find(assembly, namespace, name).as_slice() {
            [index] => Ok(*index),
            found => Err(Error::data(format!("{} definitions of {namespace}.{name} in {assembly}", found.len()))),
        }
    }

    /// The type definitions named `namespace.name` (not nested) in the image `assembly`.dll.
    fn find(&self, assembly: &str, namespace: &str, name: &str) -> Vec<u32> {
        // MonoScripts name most assemblies bare (`Assembly-CSharp`), some with their extension (`DOTweenPro.dll`).
        let image_name = if assembly.ends_with(".dll") { assembly.to_owned() } else { format!("{assembly}.dll") };
        let Some(image) = self.metadata.images.iter().find(|i| i.name == image_name) else {
            return Vec::new();
        };
        (image.first_type..image.first_type + image.type_count)
            .filter(|&i| {
                self.metadata
                    .types
                    .get(i as usize)
                    .is_some_and(|t| t.declaring_type.is_none() && t.namespace == namespace && t.name == name)
            })
            .collect()
    }
}
