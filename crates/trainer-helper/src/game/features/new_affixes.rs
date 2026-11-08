//! Definitions added to the game's `AffixList`: built whole on the side, then two array swaps. The list keeps no index:
//! `GetAffix` scans both arrays and `AllAffixes` rebuilds its cache once it is cleared, so nothing else needs telling.
//! The marker in an entry's name says it is ours, whichever load of the helper added it. Added affixes roll nowhere: no
//! base lists them and they weigh nothing.
use crate::game::apply::uncertain;
use crate::game::{Class, GameError, Images, Method, MethodError, MethodQuery, Root, RuntimeAccess, WriteError, instance_offset, target};
use eg_trainer_wire::new_affixes::{MARKER, NewAffix, StatLine, TIERS};
use il2cpp_runtime::{Argument, AttachedThread, InvocationContext, InvocationOutcome, ReturnValue, Scalar, raw};
use std::{
    collections::BTreeSet,
    ffi::{CStr, CString},
    ptr::NonNull,
};

/// `AffixList.SpecialAffixType.FakeUniqueMod`, the game's own type for a unique's modifier held as an affix.
const FAKE_UNIQUE_MOD: i32 = 7;
/// `AffixList.ClassSpecificity.NonSpecific`.
const NON_SPECIFIC: i32 = 1;
const BOOLEAN: (&CStr, i32) = (c"Boolean", raw::type_code::BOOLEAN);
const BYTE: (&CStr, i32) = (c"Byte", raw::type_code::U1);
const INT32: (&CStr, i32) = (c"Int32", raw::type_code::I4);
const SINGLE: (&CStr, i32) = (c"Single", raw::type_code::R4);
/// An array's elements start here, as `array_count`'s checks of the runtime establish.
const MAX_LISTED: u32 = 10_000;

struct Classes {
    list: Class,
    affix: Class,
    single: Class,
    multi: Class,
    property: Class,
    tier: Class,
    pair: Class,
}

/// One of the two arrays, as it stands.
struct Held<'a, 'r> {
    field: &'static CStr,
    element: Class,
    entries: Vec<Root<'a, 'r>>,
}

fn refused(error: impl std::fmt::Display) -> WriteError {
    WriteError::Refused(error.to_string())
}

impl RuntimeAccess {
    unsafe fn affix_classes(&self, images: &Images) -> Result<Classes, GameError> {
        // SAFETY: class and nested lookups read retained metadata and invoke nothing.
        unsafe {
            let list = self.class(images, c"", c"AffixList")?;
            Ok(Classes {
                list,
                affix: self.nested(list, c"Affix")?,
                single: self.nested(list, c"SingleAffix")?,
                multi: self.nested(list, c"MultiAffix")?,
                property: self.nested(list, c"AffixProperty")?,
                tier: self.nested(list, c"Tier")?,
                pair: self.nested(list, c"RollPair")?,
            })
        }
    }

    unsafe fn affix_master<'a, 'r>(&self, attached: &'a AttachedThread<'r>, images: &Images, classes: &Classes) -> Result<Root<'a, 'r>, WriteError> {
        // SAFETY: the static is a validated reference, rooted before it is inspected.
        unsafe {
            let globals = self.class(images, c"LE.AssetManagement", c"GlobalAssets")?;
            let master = self
                .static_object(attached, globals, c"_storage_MasterAffixesList", classes.list)?
                .ok_or_else(|| WriteError::NotReady("Affix definitions are unavailable".into()))?;
            self.exact(&master, classes.list)?;
            if !self.is_live(attached, images, &master)? {
                return Err(WriteError::NotReady("Affix definitions are destroyed".into()));
            }
            Ok(master)
        }
    }

    unsafe fn held_affixes<'a, 'r>(&self, attached: &'a AttachedThread<'r>, master: &Root<'_, '_>, field: &'static CStr, element: Class) -> Result<Held<'a, 'r>, WriteError> {
        // SAFETY: the rooted list's array is checked and every entry rooted by `array`.
        unsafe {
            let array = self
                .child(attached, master, field)?
                .ok_or_else(|| WriteError::NotReady("Affix definition array is unavailable".into()))?;
            let entries = self
                .array(attached, &array, element, MAX_LISTED)?
                .into_iter()
                .map(|entry| entry.ok_or_else(|| refused("Affix definition array has a gap")))
                .collect::<Result<_, _>>()?;
            Ok(Held { field, element, entries })
        }
    }

    /// A new array of these entries, swapped into the list.
    unsafe fn store_affixes(
        &self,
        attached: &AttachedThread<'_>,
        writes: raw::FieldWriteApi,
        master: &Root<'_, '_>,
        held: &Held<'_, '_>,
        entries: &[&Root<'_, '_>],
    ) -> Result<(), WriteError> {
        // SAFETY: the new array is rooted, of the field's exact element class and long enough; every entry is rooted and of that class.
        unsafe {
            let array = NonNull::new((self.array_new)(held.element, entries.len())).ok_or_else(|| refused("Cannot allocate an affix array"))?;
            let array = attached.root(array).map_err(GameError::from)?;
            if self.array_count(&array, held.element, MAX_LISTED)? as usize != entries.len() {
                return Err(refused("Affix array length"));
            }
            for (index, entry) in entries.iter().enumerate() {
                self.check_object(held.element, target(entry)?).map_err(GameError::from)?;
                writes.set_element(target(&array)?, il2cpp_runtime::metadata::ARRAY_HEADER, index, target(entry)?);
            }
            self.set_reference(&writes, attached, master, held.field, &array)?;
            Ok(())
        }
    }

    unsafe fn clear_all_affixes(&self, writes: raw::FieldWriteApi, master: &Root<'_, '_>, list: Class) -> Result<(), WriteError> {
        // SAFETY: an instance reference field of the rooted list; null is what `AllAffixes` rebuilds from.
        unsafe {
            let field = self.field(list, c"allAffixes")?;
            if (self.field_flags)(field) & raw::FIELD_ATTRIBUTE_STATIC != 0 {
                return Err(refused("allAffixes is static"));
            }
            writes.clear_object(target(master)?, NonNull::new(field.cast()).ok_or_else(|| refused("allAffixes is absent"))?);
            Ok(())
        }
    }

    /// Adds what the list does not hold yet. An ID the game or another mod holds is refused, and nothing changes.
    ///
    /// # Safety
    /// The Forge write's main-thread runtime, graph and exclusive-write contract applies.
    pub unsafe fn register_affixes(
        &self,
        attached: &AttachedThread<'_>,
        writes: raw::FieldWriteApi,
        api: raw::InvocationApi,
        expected_manager: u64,
        affixes: &[NewAffix],
    ) -> Result<(), WriteError> {
        // SAFETY: as the function's contract says; every new object is rooted, and the live list is touched only by the last stores.
        unsafe {
            let status = self.status(attached)?;
            if status.online || status.manager == 0 {
                return Err(WriteError::Offline);
            }
            if status.manager != expected_manager {
                return Err(WriteError::Stale("The character changed".into()));
            }
            let images = self.images()?;
            let classes = self.affix_classes(&images)?;
            let master = self.affix_master(attached, &images, &classes)?;
            let singles = self.held_affixes(attached, &master, c"singleAffixes", classes.single)?;
            let multis = self.held_affixes(attached, &master, c"multiAffixes", classes.multi)?;
            let (mut ours, mut theirs) = (BTreeSet::new(), BTreeSet::new());
            for entry in singles.entries.iter().chain(&multis.entries) {
                let id: i32 = self.primitive(entry, c"affixId")?;
                if self.text(attached, entry, c"affixName")?.starts_with(MARKER) {
                    ours.insert(id);
                } else {
                    theirs.insert(id);
                }
            }
            let (mut new_singles, mut new_multis) = (Vec::new(), Vec::new());
            for affix in affixes {
                let id = i32::from(affix.id);
                if theirs.contains(&id) {
                    return Err(refused(format!("Affix ID {id} is the game's or another mod's")));
                }
                if ours.contains(&id) {
                    continue;
                }
                let object = self.new_affix(attached, writes, api, &classes, affix)?;
                if affix.properties.len() == 1 {
                    new_singles.push(object)
                } else {
                    new_multis.push(object)
                }
            }
            for (held, new) in [(&singles, &new_singles), (&multis, &new_multis)] {
                if !new.is_empty() {
                    let entries: Vec<&Root<'_, '_>> = held.entries.iter().chain(new.iter()).collect();
                    self.store_affixes(attached, writes, &master, held, &entries).map_err(uncertain)?;
                }
            }
            if !new_singles.is_empty() || !new_multis.is_empty() {
                self.clear_all_affixes(writes, &master, classes.list).map_err(uncertain)?;
            }
            Ok(())
        }
    }

    /// Takes out every entry of ours, so nothing of the helper stays once it has left. A loaded item that holds one keeps
    /// its ID and is left out of the stats until the helper returns.
    ///
    /// # Safety
    /// An attached thread of the live runtime. The main thread may read the arrays meanwhile: each is swapped whole, and
    /// the one it holds stays valid.
    pub unsafe fn remove_registered_affixes(&self, attached: &AttachedThread<'_>, writes: raw::FieldWriteApi) -> Result<(), WriteError> {
        // SAFETY: as the function's contract says; each kept entry is rooted through the store.
        unsafe {
            let images = self.images()?;
            let classes = self.affix_classes(&images)?;
            let master = self.affix_master(attached, &images, &classes)?;
            let mut removed = false;
            for (field, element) in [(c"singleAffixes", classes.single), (c"multiAffixes", classes.multi)] {
                let held = self.held_affixes(attached, &master, field, element)?;
                let mut kept = Vec::with_capacity(held.entries.len());
                for entry in &held.entries {
                    if !self.text(attached, entry, c"affixName")?.starts_with(MARKER) {
                        kept.push(entry);
                    }
                }
                if kept.len() != held.entries.len() {
                    self.store_affixes(attached, writes, &master, &held, &kept)?;
                    removed = true;
                }
            }
            if removed {
                self.clear_all_affixes(writes, &master, classes.list)?;
            }
            Ok(())
        }
    }

    unsafe fn new_affix<'a, 'r>(
        &self,
        attached: &'a AttachedThread<'r>,
        writes: raw::FieldWriteApi,
        api: raw::InvocationApi,
        classes: &Classes,
        affix: &NewAffix,
    ) -> Result<Root<'a, 'r>, WriteError> {
        // SAFETY: the object is rooted and unreachable from the game until the caller stores it; each destination is checked by name and type.
        unsafe {
            let [first, rest @ ..] = affix.properties.as_slice() else {
                return Err(refused("An affix needs a property"));
            };
            let single = rest.is_empty();
            let object = self.constructed(attached, api, if single { classes.single } else { classes.multi })?;
            let owner = classes.affix;
            writes.set_i32(target(&object)?, self.mutable_field(owner, c"affixId", INT32.0, INT32.1)?, i32::from(affix.id));
            writes.set_i32(target(&object)?, self.enum_field(owner, c"specialAffixType", INT32)?, FAKE_UNIQUE_MOD);
            writes.set_i32(target(&object)?, self.enum_field(owner, c"classSpecificity", INT32)?, NON_SPECIFIC);
            writes.set_f32(target(&object)?, self.mutable_field(owner, c"weighting", SINGLE.0, SINGLE.1)?, 0.0);
            writes.set_f32(
                target(&object)?,
                self.mutable_field(owner, c"standardAffixEffectModifier", SINGLE.0, SINGLE.1)?,
                affix.standard_effect,
            );
            self.store_text(attached, writes, &object, c"affixName", &affix.name)?;
            self.store_text(attached, writes, &object, c"affixDisplayName", &affix.display_name)?;
            self.store_text(attached, writes, &object, c"affixLootFilterOverrideName", &affix.display_name)?;
            self.store_text(attached, writes, &object, c"affixTitle", "")?;
            for name in [c"canRollOn", c"specificRerollChances"] {
                let list = self.new_list(attached, api, self.field_class(self.field(owner, name)?, "affix")?)?;
                self.set_reference(&writes, attached, &object, name, &list)?;
            }
            let tiers = self.new_list(attached, api, self.field_class(self.field(owner, c"tiers")?, "affix")?)?;
            let add = self.list_add(&tiers, classes.tier)?;
            for _ in 0..TIERS {
                let tier = self.new_tier(attached, writes, api, classes, first, rest)?;
                self.add_to_list(attached, api, &tiers, add, &tier)?;
            }
            self.set_reference(&writes, attached, &object, c"tiers", &tiers)?;
            if single {
                self.store_line(writes, &object, classes.single, first)?;
                writes.set_u8(
                    target(&object)?,
                    self.mutable_field(classes.single, c"useGeneratedNameForDisplayName", BOOLEAN.0, BOOLEAN.1)?,
                    u8::from(affix.display_name.is_empty()),
                );
            } else {
                let properties = self.new_list(attached, api, self.field_class(self.field(classes.multi, c"affixProperties")?, "affix")?)?;
                let add = self.list_add(&properties, classes.property)?;
                for line in &affix.properties {
                    let property = self.constructed(attached, api, classes.property)?;
                    self.store_line(writes, &property, classes.property, line)?;
                    self.store_text(attached, writes, &property, c"modDisplayName", &affix.display_name)?;
                    self.add_to_list(attached, api, &properties, add, &property)?;
                }
                self.set_reference(&writes, attached, &object, c"affixProperties", &properties)?;
            }
            Ok(object)
        }
    }

    unsafe fn new_tier<'a, 'r>(
        &self,
        attached: &'a AttachedThread<'r>,
        writes: raw::FieldWriteApi,
        api: raw::InvocationApi,
        classes: &Classes,
        first: &StatLine,
        rest: &[StatLine],
    ) -> Result<Root<'a, 'r>, WriteError> {
        // SAFETY: as new_affix: rooted objects the game cannot reach yet.
        unsafe {
            let tier = self.constructed(attached, api, classes.tier)?;
            writes.set_f32(target(&tier)?, self.mutable_field(classes.tier, c"minRoll", SINGLE.0, SINGLE.1)?, first.minimum);
            writes.set_f32(target(&tier)?, self.mutable_field(classes.tier, c"maxRoll", SINGLE.0, SINGLE.1)?, first.maximum);
            if !rest.is_empty() {
                let rolls = self.new_list(attached, api, self.field_class(self.field(classes.tier, c"extraRolls")?, "affix")?)?;
                let add = self.list_add(&rolls, classes.pair)?;
                for line in rest {
                    let pair = self.constructed_pair(attached, api, classes.pair, line.minimum, line.maximum)?;
                    self.add_to_list(attached, api, &rolls, add, &pair)?;
                }
                self.set_reference(&writes, attached, &tier, c"extraRolls", &rolls)?;
            }
            Ok(tier)
        }
    }

    unsafe fn store_line(&self, writes: raw::FieldWriteApi, object: &Root<'_, '_>, owner: Class, line: &StatLine) -> Result<(), WriteError> {
        // SAFETY: each destination is an exact field of the rooted object's class.
        unsafe {
            writes.set_u8(target(object)?, self.enum_field(owner, c"property", BYTE)?, line.property);
            writes.set_u8(target(object)?, self.mutable_field(owner, c"specialTag", BYTE.0, BYTE.1)?, line.special_tag);
            writes.set_i32(target(object)?, self.enum_field(owner, c"tags", INT32)?, line.tags);
            writes.set_i32(target(object)?, self.mutable_field(owner, c"extraTag", INT32.0, INT32.1)?, line.extra_tag);
            writes.set_i32(target(object)?, self.enum_field(owner, c"modifierType", INT32)?, line.modifier_type);
            Ok(())
        }
    }

    /// `owner.name` as a destination: as [`RuntimeAccess::mutable_field`], of an enum stored as `storage`.
    unsafe fn enum_field(&self, owner: Class, name: &CStr, storage: (&CStr, i32)) -> Result<NonNull<raw::FieldInfo>, WriteError> {
        // SAFETY: retained metadata only; the enum's exact underlying storage is checked before a store can use the field.
        unsafe {
            let field = self.field(owner, name)?;
            self.enum_storage(self.field_class(field, "affix")?, storage.0, storage.1)?;
            if (self.field_parent)(field) != owner
                || (self.field_flags)(field) & (raw::FIELD_ATTRIBUTE_STATIC | raw::FIELD_ATTRIBUTE_INIT_ONLY | raw::FIELD_ATTRIBUTE_LITERAL) != 0
                || !instance_offset((self.field_offset)(field))
            {
                return Err(refused(format!("{} is not a writable enum field", name.to_string_lossy())));
            }
            NonNull::new(field.cast()).ok_or_else(|| refused("Missing enum field"))
        }
    }

    unsafe fn store_text(&self, attached: &AttachedThread<'_>, writes: raw::FieldWriteApi, object: &Root<'_, '_>, name: &CStr, text: &str) -> Result<(), WriteError> {
        // SAFETY: the runtime copies the nul-terminated text; the string is rooted before the checked store.
        unsafe {
            let text = CString::new(text).map_err(refused)?;
            let string = NonNull::new((self.string_new)(text.as_ptr())).ok_or_else(|| refused("Cannot allocate a string"))?;
            let string = attached.root(string).map_err(GameError::from)?;
            self.set_reference(&writes, attached, object, name, &string)?;
            Ok(())
        }
    }

    /// Allocates and roots one object of an exact class, for its constructor.
    unsafe fn allocated<'a, 'r>(&self, attached: &'a AttachedThread<'r>, class: Class) -> Result<Root<'a, 'r>, WriteError> {
        // SAFETY: the class is retained metadata; the object is rooted before any further runtime call.
        unsafe {
            let object = NonNull::new((self.object_new)(class)).ok_or_else(|| refused("Cannot allocate"))?;
            Ok(attached.root(object).map_err(GameError::from)?)
        }
    }

    unsafe fn constructed<'a, 'r>(&self, attached: &'a AttachedThread<'r>, api: raw::InvocationApi, class: Class) -> Result<Root<'a, 'r>, WriteError> {
        // SAFETY: the parameterless constructor of an exact game class runs on a new rooted object.
        unsafe {
            let constructor = self.method(class, c".ctor", &[], true)?;
            let object = self.allocated(attached, class)?;
            let context = InvocationContext::new(attached, api);
            let constructor = context
                .bind(NonNull::new(constructor.cast_mut()).ok_or_else(|| refused("Missing constructor"))?)
                .map_err(refused)?;
            match constructor.invoke(Some(&object), &mut []).map_err(refused)? {
                InvocationOutcome::Returned(ReturnValue::Void) => Ok(object),
                _ => Err(refused("A constructor threw")),
            }
        }
    }

    unsafe fn constructed_pair<'a, 'r>(
        &self,
        attached: &'a AttachedThread<'r>,
        api: raw::InvocationApi,
        class: Class,
        minimum: f32,
        maximum: f32,
    ) -> Result<Root<'a, 'r>, WriteError> {
        // SAFETY: as constructed, with the two exact Single arguments.
        unsafe {
            let constructor = self.method(class, c".ctor", &["System.Single", "System.Single"], true)?;
            let object = self.allocated(attached, class)?;
            let context = InvocationContext::new(attached, api);
            let constructor = context
                .bind(NonNull::new(constructor.cast_mut()).ok_or_else(|| refused("Missing constructor"))?)
                .map_err(refused)?;
            match constructor
                .invoke(Some(&object), &mut [Argument::Scalar(Scalar::F32(minimum)), Argument::Scalar(Scalar::F32(maximum))])
                .map_err(refused)?
            {
                InvocationOutcome::Returned(ReturnValue::Void) => Ok(object),
                _ => Err(refused("A constructor threw")),
            }
        }
    }

    /// A new empty closed `List<T>`, of the class its field declares.
    unsafe fn new_list<'a, 'r>(&self, attached: &'a AttachedThread<'r>, api: raw::InvocationApi, class: Class) -> Result<Root<'a, 'r>, WriteError> {
        // SAFETY: the exact corlib closed List's parameterless constructor runs on a new rooted object.
        unsafe {
            self.list_class(class)?;
            let constructor = self.find_method(class, &MethodQuery::named(c".ctor", 0)).map_err(|error| match error {
                MethodError::Metadata(error) => WriteError::from(error),
                _ => refused("A list has no parameterless constructor"),
            })?;
            let closed = NonNull::new(class).ok_or_else(|| refused("Missing list class"))?;
            let object = self.allocated(attached, class)?;
            let context = InvocationContext::new(attached, api);
            let constructor = context
                .bind_with_closed_references(NonNull::new(constructor.cast_mut()).ok_or_else(|| refused("Missing constructor"))?, &[closed])
                .map_err(refused)?;
            match constructor.invoke(Some(&object), &mut []).map_err(refused)? {
                InvocationOutcome::Returned(ReturnValue::Void) => Ok(object),
                _ => Err(refused("A list constructor threw")),
            }
        }
    }

    unsafe fn list_add(&self, list: &Root<'_, '_>, element: Class) -> Result<Method, WriteError> {
        // SAFETY: the closed List's exact Add(element), checked as the Forge's own.
        unsafe { self.affix_list_method((self.object_class)(target(list)?.as_ptr()), element, c"Add", true) }
    }

    unsafe fn add_to_list(&self, attached: &AttachedThread<'_>, api: raw::InvocationApi, list: &Root<'_, '_>, add: Method, value: &Root<'_, '_>) -> Result<(), WriteError> {
        // SAFETY: Add's barrier stores the rooted value in the rooted, still private list.
        unsafe {
            let class = NonNull::new((self.object_class)(target(list)?.as_ptr())).ok_or_else(|| refused("Missing list class"))?;
            let context = InvocationContext::new(attached, api);
            let add = context
                .bind_with_closed_references(NonNull::new(add.cast_mut()).ok_or_else(|| refused("Missing Add"))?, &[class])
                .map_err(refused)?;
            match add.invoke(Some(list), &mut [Argument::Reference(Some(value))]).map_err(refused)? {
                InvocationOutcome::Returned(ReturnValue::Void) => Ok(()),
                _ => Err(refused("List Add threw")),
            }
        }
    }
}
