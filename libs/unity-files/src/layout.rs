//! Rebuilds the type tree Unity would write for a MonoBehaviour or ScriptableObject class, from the IL2CPP build. Unity
//! strips type trees from player data (`resources.assets`), so objects there decode only with a layout made this way.
//!
//! The rules are Unity's own serialization logic (`Unity.SerializationLogic`, as AssetStudio vendors it, with its
//! `TypeDefinitionConverter`), with three changes Unity made since and the shipped trees confirm: generic types
//! serialize (2020.1+), enums take their underlying type, and the outer collection node is named after its element.
//! Anything outside these rules ([SerializeReference], fixed buffers, Unity types with native layouts not listed here)
//! is refused by name rather than guessed. A layout is only trusted once an object decodes with it exactly.

use crate::il2cpp::{Field, Il2Cpp, Ty, field_attributes as fa, kind, type_attributes};
use crate::typetree::{ALIGN, Node};
use binary_reader::{Error, Result};
use std::sync::Arc;

/// Past this depth of nested serializable classes Unity writes only a class's primitive fields: no enums, collections
/// or classes. A recursive class therefore stops some levels in, its last level keeping its floats, ints and bools.
const MAX_DEPTH: usize = 10;

/// Unity value types it serializes field by field although they are not marked `[Serializable]`.
const UNITY_VALUE_TYPES: [&str; 14] = [
    "Vector3",
    "Vector2",
    "Vector4",
    "Rect",
    "RectInt",
    "Quaternion",
    "Matrix4x4",
    "Color",
    "Color32",
    "LayerMask",
    "Bounds",
    "BoundsInt",
    "Vector3Int",
    "Vector2Int",
];
/// Unity classes and structs with a native serialized layout of their own.
const UNITY_CLASSES: [&str; 4] = ["UnityEngine.AnimationCurve", "UnityEngine.Gradient", "UnityEngine.GUIStyle", "UnityEngine.RectOffset"];
const UNITY_STRUCTS: [&str; 4] = [
    "UnityEngine.Color32",
    "UnityEngine.Matrix4x4",
    "UnityEngine.Rendering.SphericalHarmonicsL2",
    "UnityEngine.PropertyName",
];
/// Unity value types whose serialized field names differ from their managed ones.
const RENAMED_UNITY_STRUCTS: [&str; 6] = [
    "UnityEngine.LayerMask",
    "UnityEngine.Rect",
    "UnityEngine.Vector2Int",
    "UnityEngine.Vector3Int",
    "UnityEngine.RectInt",
    "UnityEngine.BoundsInt",
];

fn node(type_name: &str, name: &str, align: bool, children: Vec<Node>) -> Node {
    Node {
        type_name: Arc::from(type_name),
        name: Arc::from(name),
        byte_size: -1,
        meta_flag: if align { ALIGN } else { 0 },
        children,
    }
}

fn leaf(type_name: &str, name: &str, align: bool) -> Node {
    node(type_name, name, align, Vec::new())
}

fn string(name: &str) -> Node {
    node(
        "string",
        name,
        false,
        vec![node("Array", "Array", true, vec![leaf("int", "size", false), leaf("char", "data", false)])],
    )
}

fn pptr(type_name: &str, name: &str) -> Node {
    node(
        &format!("PPtr<{type_name}>"),
        name,
        false,
        vec![leaf("int", "m_FileID", false), leaf("SInt64", "m_PathID", false)],
    )
}

/// A collection. Its `Array` is aligned (`packed`) after primitives, strings and Unity's own value types, not after
/// other classes or structs.
fn vector(type_name: &str, name: &str, align: bool, packed: bool, element: Node) -> Node {
    node(type_name, name, align, vec![node("Array", "Array", packed, vec![leaf("int", "size", false), element])])
}

/// The fields every MonoBehaviour starts with, whatever its script: enough to read its name and script.
pub fn mono_behaviour_header() -> Node {
    node(
        "MonoBehaviour",
        "Base",
        false,
        vec![
            pptr("GameObject", "m_GameObject"),
            leaf("UInt8", "m_Enabled", true),
            pptr("MonoScript", "m_Script"),
            string("m_Name"),
        ],
    )
}

fn primitive_name(primitive: u8) -> Option<&'static str> {
    Some(match primitive {
        // Unity 6 writes bool fields as UInt8.
        kind::BOOLEAN => "UInt8",
        // char serializes as its UTF-16 unit.
        kind::CHAR => "UInt16",
        kind::I1 => "SInt8",
        kind::U1 => "UInt8",
        kind::I2 => "SInt16",
        kind::U2 => "UInt16",
        kind::I4 => "int",
        kind::U4 => "unsigned int",
        kind::I8 => "SInt64",
        kind::U8 => "UInt64",
        kind::R4 => "float",
        kind::R8 => "double",
        _ => return None,
    })
}

pub struct Layouts<'a> {
    il2cpp: &'a Il2Cpp,
}

impl<'a> Layouts<'a> {
    pub fn new(il2cpp: &'a Il2Cpp) -> Self {
        Self { il2cpp }
    }

    /// The tree of a MonoBehaviour (or ScriptableObject) whose script is `assembly`'s `namespace.class`.
    pub fn mono_behaviour(&self, assembly: &str, namespace: &str, class: &str) -> Result<Node> {
        let definition = self.il2cpp.find_one(assembly, namespace, class)?;
        let mut root = mono_behaviour_header();
        root.children
            .extend(self.class_fields(definition, &[], 0).map_err(|e| e.context(format!("{namespace}.{class}")))?);
        Ok(root)
    }

    fn name(&self, ty: &Ty) -> Result<String> {
        match ty {
            Ty::Definition { index } | Ty::Generic { definition: index, .. } => self.il2cpp.full_name(*index),
            Ty::String => Ok("System.String".into()),
            Ty::Object => Ok("System.Object".into()),
            _ => Ok(String::new()),
        }
    }

    fn definition_of(ty: &Ty) -> Option<u32> {
        match ty {
            Ty::Definition { index } | Ty::Generic { definition: index, .. } => Some(*index),
            _ => None,
        }
    }

    fn substitute(ty: &Ty, arguments: &[Ty]) -> Ty {
        match ty {
            Ty::Parameter(position) => arguments.get(usize::from(*position)).cloned().unwrap_or_else(|| ty.clone()),
            Ty::Generic { definition, arguments: inner } => Ty::Generic {
                definition: *definition,
                arguments: inner.iter().map(|a| Self::substitute(a, arguments)).collect(),
            },
            Ty::Vector(element) => Ty::Vector(Box::new(Self::substitute(element, arguments))),
            Ty::Array(element, rank) => Ty::Array(Box::new(Self::substitute(element, arguments)), *rank),
            other => other.clone(),
        }
    }

    /// The parent of a type, with the type's arguments applied.
    fn parent(&self, ty: &Ty) -> Result<Option<Ty>> {
        let Some(index) = Self::definition_of(ty) else { return Ok(None) };
        let Some(parent) = self.il2cpp.definition(index)?.parent else { return Ok(None) };
        let arguments = match ty {
            Ty::Generic { arguments, .. } => arguments.as_slice(),
            _ => &[],
        };
        Ok(Some(Self::substitute(&self.il2cpp.type_at(parent)?, arguments)))
    }

    fn is_subclass_of(&self, ty: &Ty, names: &[&str]) -> Result<bool> {
        let mut current = self.parent(ty)?;
        while let Some(ty) = current {
            if names.contains(&self.name(&ty)?.as_str()) {
                return Ok(true);
            }
            current = self.parent(&ty)?;
        }
        Ok(false)
    }

    fn is_value_type(&self, ty: &Ty) -> Result<bool> {
        Ok(match ty {
            Ty::Primitive(_) => true,
            _ => match Self::definition_of(ty) {
                Some(index) => self.il2cpp.definition(index)?.is_value_type,
                None => false,
            },
        })
    }

    fn is_enum(&self, ty: &Ty) -> Result<bool> {
        Ok(matches!(ty, Ty::Definition { index } if self.il2cpp.definition(*index)?.is_enum))
    }

    fn is_struct(&self, ty: &Ty) -> Result<bool> {
        Ok(!matches!(ty, Ty::Primitive(_)) && self.is_value_type(ty)? && !self.is_enum(ty)?)
    }

    fn is_unity_value_type(&self, ty: &Ty) -> Result<bool> {
        Ok(match Self::definition_of(ty) {
            Some(index) => {
                let definition = self.il2cpp.definition(index)?;
                definition.namespace == "UnityEngine" && UNITY_VALUE_TYPES.contains(&definition.name.as_str())
            }
            None => false,
        })
    }

    fn is_unity_object(&self, ty: &Ty) -> Result<bool> {
        Ok(!matches!(ty, Ty::Vector(_) | Ty::Array(..)) && (self.name(ty)? == "UnityEngine.Object" || self.is_subclass_of(ty, &["UnityEngine.Object"])?))
    }

    fn is_list(&self, ty: &Ty) -> Result<bool> {
        Ok(matches!(ty, Ty::Generic { arguments, .. } if arguments.len() == 1) && self.name(ty)? == "System.Collections.Generic.List`1")
    }

    /// The element of a `T[]` or `List<T>`.
    fn element(&self, ty: &Ty) -> Result<Option<Ty>> {
        Ok(match ty {
            Ty::Vector(element) => Some((**element).clone()),
            Ty::Generic { arguments, .. } if self.is_list(ty)? => Some(arguments[0].clone()),
            _ => None,
        })
    }

    fn is_supported_collection(&self, ty: &Ty) -> Result<bool> {
        Ok(match self.element(ty)? {
            Some(element) => self.is_type_serializable(&element)?,
            None => false,
        })
    }

    fn is_serializable_primitive(ty: &Ty) -> bool {
        matches!(ty, Ty::Primitive(kind) if primitive_name(*kind).is_some()) || matches!(ty, Ty::String)
    }

    fn is_unity_class(&self, ty: &Ty) -> Result<bool> {
        let name = self.name(ty)?;
        Ok(UNITY_CLASSES.contains(&name.as_str()) || self.is_subclass_of(ty, &UNITY_CLASSES)?)
    }

    fn is_unity_struct(&self, ty: &Ty) -> Result<bool> {
        let name = self.name(ty)?;
        Ok(UNITY_STRUCTS.contains(&name.as_str()) || name.starts_with("UnityEngine.LazyLoadReference`1"))
    }

    fn is_non_serialized(&self, ty: &Ty) -> Result<bool> {
        if matches!(ty, Ty::Object | Ty::Vector(_) | Ty::Array(..)) {
            return Ok(true);
        }
        if let Ty::Definition { index } = ty
            && self.il2cpp.definition(*index)?.has_generic_parameters
        {
            return Ok(true);
        }
        let name = self.name(ty)?;
        Ok(name.is_empty() || name.starts_with("System.") || name == "UnityEngine.MonoBehaviour" || name == "UnityEngine.ScriptableObject" || self.is_enum(ty)?)
    }

    fn is_compiler_generated(&self, index: u32) -> Result<bool> {
        Ok(self
            .il2cpp
            .attributes(index, None)?
            .iter()
            .any(|a| a.contains("System.Runtime.CompilerServices.CompilerGenerated")))
    }

    fn should_implement_deserializable(&self, ty: &Ty) -> Result<bool> {
        if self.name(ty)? == "UnityEngine.ExposedReference`1" {
            return Ok(true);
        }
        if self.is_non_serialized(ty)? {
            return Ok(false);
        }
        if self.is_unity_value_type(ty)? {
            return Ok(true);
        }
        let Some(index) = Self::definition_of(ty) else { return Ok(false) };
        let definition = self.il2cpp.definition(index)?;
        // Abstract classes and interfaces only serialize by reference.
        if definition.flags & (type_attributes::INTERFACE | type_attributes::ABSTRACT) != 0 {
            return Ok(false);
        }
        let serializable = definition.flags & type_attributes::SERIALIZABLE != 0 && !self.is_compiler_generated(index)?;
        Ok(if definition.is_value_type {
            serializable
        } else {
            serializable || self.is_subclass_of(ty, &["UnityEngine.MonoBehaviour", "UnityEngine.ScriptableObject"])?
        })
    }

    fn is_value_type_serializable(&self, ty: &Ty) -> Result<bool> {
        if let Ty::Primitive(_) = ty {
            return Ok(Self::is_serializable_primitive(ty));
        }
        Ok(self.is_unity_struct(ty)? || self.is_enum(ty)? || self.should_implement_deserializable(ty)?)
    }

    fn is_reference_type_serializable(&self, ty: &Ty) -> Result<bool> {
        if matches!(ty, Ty::String) {
            return Ok(true);
        }
        if self.name(ty)? == "System.Collections.Generic.Dictionary`2" {
            return Ok(false);
        }
        Ok(self.is_unity_object(ty)? || self.should_implement_deserializable(ty)? || self.is_unity_class(ty)?)
    }

    fn is_type_serializable(&self, ty: &Ty) -> Result<bool> {
        Ok(match ty {
            Ty::String => true,
            Ty::Parameter(_) | Ty::Other(_) | Ty::Array(..) => false,
            _ if self.is_value_type(ty)? => self.is_value_type_serializable(ty)?,
            _ => self.is_reference_type_serializable(ty)?,
        })
    }

    fn requires_alignment(&self, ty: &Ty) -> Result<bool> {
        Ok(match self.underlying(ty)? {
            Ty::Primitive(primitive) => matches!(primitive, kind::BOOLEAN..=kind::U2),
            other => match self.element(&other)? {
                Some(element) if self.is_supported_collection(&other)? => self.requires_alignment(&element)?,
                _ => false,
            },
        })
    }

    /// An enum's underlying primitive (its `value__` field); any other type as it is.
    fn underlying(&self, ty: &Ty) -> Result<Ty> {
        if let Ty::Definition { index } = ty
            && self.is_enum(ty)?
        {
            let fields = self.il2cpp.fields(*index)?;
            let value = fields
                .iter()
                .find(|f| f.attributes & fa::STATIC == 0)
                .ok_or_else(|| Error::data(format!("enum {} has no value field", self.il2cpp.full_name(*index).unwrap_or_default())))?;
            return Ok(value.ty.clone());
        }
        Ok(ty.clone())
    }

    /// Unity's `WillUnitySerialize`, for a field of `owner` whose type is already resolved to `ty`.
    fn will_serialize(&self, owner: u32, field: &Field<'_>, ty: &Ty) -> Result<bool> {
        let a = field.attributes;
        let constant = a & fa::LITERAL != 0 && a & fa::INIT_ONLY == 0;
        if a & fa::STATIC != 0 || constant || a & fa::NOT_SERIALIZED != 0 || a & fa::INIT_ONLY != 0 {
            return Ok(false);
        }
        let attributes = self.il2cpp.attributes(owner, Some(field))?;
        let has = |name: &str| attributes.iter().any(|a| a == name);
        let reference = has("UnityEngine.SerializeReference");
        if a & fa::ACCESS != fa::PUBLIC && !self.is_unity_value_type(&Ty::Definition { index: owner })? && !reference && !has("UnityEngine.SerializeField") {
            return Ok(false);
        }
        if matches!(ty, Ty::Parameter(_) | Ty::Other(_)) {
            return Ok(false);
        }
        // A field of its own type is not serialized (unless it is a Unity object reference).
        if !self.is_unity_object(ty)? && Self::definition_of(ty) == Some(owner) {
            return Ok(false);
        }
        if has("System.Runtime.CompilerServices.FixedBufferAttribute") {
            return Err(Error::unsupported(format!("fixed buffer field {}", field.name)));
        }
        let serialized = match ty {
            Ty::String => true,
            _ if self.is_value_type(ty)? => self.is_value_type_serializable(ty)?,
            Ty::Vector(_) | Ty::Array(..) if !reference => self.is_supported_collection(ty)?,
            _ if self.is_list(ty)? && !reference => self.is_supported_collection(ty)?,
            _ if !self.is_reference_type_serializable(ty)? && !reference => false,
            _ => !self.is_subclass_of(ty, &["System.Delegate"])?,
        };
        if serialized && reference {
            return Err(Error::unsupported(format!("[SerializeReference] field {}", field.name)));
        }
        Ok(serialized)
    }

    /// The serialized fields of a class or struct, base classes first.
    fn class_fields(&self, index: u32, arguments: &[Ty], depth: usize) -> Result<Vec<Node>> {
        let this = if arguments.is_empty() {
            Ty::Definition { index }
        } else {
            Ty::Generic {
                definition: index,
                arguments: arguments.to_vec(),
            }
        };
        let mut chain = Vec::new();
        let mut current = self.parent(&this)?;
        while let Some(parent) = current {
            if self.is_non_serialized(&parent)? {
                break;
            }
            current = self.parent(&parent)?;
            chain.push(parent);
        }
        chain.reverse();
        chain.push(this);
        // Each level's fields, read once: a base field is checked against every more derived level's names.
        let levels = chain
            .iter()
            .map(|ty| {
                let owner = Self::definition_of(ty).expect("a definition");
                Ok((owner, ty, self.il2cpp.fields(owner)?))
            })
            .collect::<Result<Vec<_>>>()?;
        let mut nodes = Vec::new();
        for (level, (owner, ty, fields)) in levels.iter().enumerate() {
            let arguments = match ty {
                Ty::Generic { arguments, .. } => arguments.as_slice(),
                _ => &[],
            };
            for field in fields {
                let resolved = Self::substitute(&field.ty, arguments);
                if !self.will_serialize(*owner, field, &resolved)? {
                    continue;
                }
                // A base field a more derived type redeclares is hidden by it.
                let hidden = levels[level + 1..].iter().any(|(_, _, derived)| derived.iter().any(|f| f.name == field.name));
                if hidden || depth > MAX_DEPTH && !matches!(resolved, Ty::Primitive(_)) {
                    continue;
                }
                nodes.push(self.field_node(index, &resolved, field.name, false, depth).map_err(|e| e.context(field.name))?);
            }
        }
        Ok(nodes)
    }

    /// The node of one field (or collection element) of type `ty`, in a class `context`.
    fn field_node(&self, context: u32, ty: &Ty, name: &str, element: bool, depth: usize) -> Result<Node> {
        let context_ty = Ty::Definition { index: context };
        let in_unity_struct = self.is_struct(&context_ty)? && self.is_unity_value_type(&context_ty)?;
        // Unlike Unity's older logic, struct fields are not aligned, and nor is any collection element.
        let align = !in_unity_struct && !element && self.requires_alignment(ty)?;
        let ty = self.underlying(ty)?;
        if let Ty::Primitive(kind) = ty {
            let type_name = primitive_name(kind).ok_or_else(|| Error::unsupported(format!("primitive kind {kind:#x}")))?;
            return Ok(leaf(type_name, name, align));
        }
        if matches!(ty, Ty::String) {
            return Ok(string(name));
        }
        if let Some(element_ty) = self.element(&ty)? {
            let packed = matches!(self.underlying(&element_ty)?, Ty::Primitive(_) | Ty::String) || self.is_unity_value_type(&element_ty)?;
            let element_node = self.field_node(context, &element_ty, "data", true, depth)?;
            return Ok(vector(&element_node.type_name.clone(), name, align, packed, element_node));
        }
        if self.is_unity_object(&ty)? {
            let type_name = Self::definition_of(&ty)
                .map(|i| self.il2cpp.definition(i).map(|d| d.name.clone()))
                .transpose()?
                .unwrap_or_default();
            return Ok(pptr(&type_name, name));
        }
        let full_name = self.name(&ty)?;
        if self.is_unity_class(&ty)? || self.is_unity_struct(&ty)? || RENAMED_UNITY_STRUCTS.contains(&full_name.as_str()) {
            return unity_native(&full_name, name);
        }
        let Some(index) = Self::definition_of(&ty) else {
            return Err(Error::unsupported(format!("field type {ty:?}")));
        };
        let definition = self.il2cpp.definition(index)?;
        let arguments = match &ty {
            Ty::Generic { arguments, .. } => arguments.as_slice(),
            _ => &[],
        };
        let children = self.class_fields(index, arguments, depth + 1).map_err(|e| e.context(&full_name))?;
        Ok(node(&definition.name, name, align, children))
    }
}

/// The Unity types whose serialized form is native, not their managed fields. Only those met so far are written out.
fn unity_native(full_name: &str, name: &str) -> Result<Node> {
    let floats = |type_name: &str, name: &str, fields: &[&str]| node(type_name, name, false, fields.iter().map(|f| leaf("float", f, false)).collect());
    let ints = |type_name: &str, name: &str, fields: &[&str]| node(type_name, name, false, fields.iter().map(|f| leaf("int", f, false)).collect());
    Ok(match full_name {
        "UnityEngine.LayerMask" => node("BitField", name, false, vec![leaf("unsigned int", "m_Bits", false)]),
        "UnityEngine.Rect" => floats("Rectf", name, &["x", "y", "width", "height"]),
        "UnityEngine.Vector2Int" => ints("int2_storage", name, &["x", "y"]),
        "UnityEngine.Vector3Int" => ints("int3_storage", name, &["x", "y", "z"]),
        "UnityEngine.RectInt" => ints("RectInt", name, &["x", "y", "width", "height"]),
        "UnityEngine.BoundsInt" => node(
            "BoundsInt",
            name,
            false,
            vec![ints("int3_storage", "m_Position", &["x", "y", "z"]), ints("int3_storage", "m_Size", &["x", "y", "z"])],
        ),
        "UnityEngine.Color32" => node("ColorRGBA", name, false, vec![leaf("unsigned int", "rgba", false)]),
        "UnityEngine.Matrix4x4" => floats(
            "Matrix4x4f",
            name,
            &[
                "e00", "e01", "e02", "e03", "e10", "e11", "e12", "e13", "e20", "e21", "e22", "e23", "e30", "e31", "e32", "e33",
            ],
        ),
        "UnityEngine.RectOffset" => node(
            "RectOffset",
            name,
            false,
            ["m_Left", "m_Right", "m_Top", "m_Bottom"].iter().map(|f| leaf("int", f, false)).collect(),
        ),
        "UnityEngine.AnimationCurve" => {
            let key = node(
                "Keyframe",
                "data",
                false,
                ["time", "value", "inSlope", "outSlope"]
                    .iter()
                    .map(|f| leaf("float", f, false))
                    .chain([leaf("int", "weightedMode", false), leaf("float", "inWeight", false), leaf("float", "outWeight", false)])
                    .collect(),
            );
            node(
                "AnimationCurve",
                name,
                false,
                vec![
                    node("vector", "m_Curve", false, vec![node("Array", "Array", true, vec![leaf("int", "size", false), key])]),
                    leaf("int", "m_PreInfinity", false),
                    leaf("int", "m_PostInfinity", false),
                    leaf("int", "m_RotationOrder", false),
                ],
            )
        }
        "UnityEngine.Gradient" => {
            let mut children: Vec<Node> = (0..8).map(|i| floats("ColorRGBA", &format!("key{i}"), &["r", "g", "b", "a"])).collect();
            children.extend((0..8).map(|i| leaf("UInt16", &format!("ctime{i}"), false)));
            children.extend((0..8).map(|i| leaf("UInt16", &format!("atime{i}"), false)));
            children.extend([
                leaf("UInt8", "m_Mode", false),
                leaf("SInt8", "m_ColorSpace", false),
                leaf("UInt8", "m_NumColorKeys", false),
                leaf("UInt8", "m_NumAlphaKeys", true),
            ]);
            node("Gradient", name, false, children)
        }
        other => return Err(Error::unsupported(format!("Unity's native layout of {other}"))),
    })
}
