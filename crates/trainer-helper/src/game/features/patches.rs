//! Shared resolver for explicit feature-owned code recipes. Metadata, method bounds and a unique full pattern are
//! checked before any write; the native owner/transaction layer retains the original bytes and performs readback.
use super::hooks::{CodeChange, GameplayHookTarget, MethodSpec, ResolvedMethod, TargetEffect};
use crate::{game::*, hooks::patch::find_unique};

pub(crate) struct PatchSpec {
    name: &'static str,
    method: MethodSpec,
    search: usize,
    pattern: &'static [i16],
    at: usize,
    replacement: &'static [u8],
    state_machine: Option<&'static CStr>,
}
impl PatchSpec {
    const fn new(name: &'static str, method: MethodSpec, search: usize, pattern: &'static [i16], at: usize, replacement: &'static [u8]) -> Self {
        Self {
            name,
            method,
            search,
            pattern,
            at,
            replacement,
            state_machine: None,
        }
    }
    const fn state_machine(mut self, prefix: &'static CStr) -> Self {
        self.state_machine = Some(prefix);
        self
    }
}

impl RuntimeAccess {
    /// # Safety
    /// Attached immutable runtime metadata, retained readable GameAssembly and exclusive patch ownership as hook_method.
    unsafe fn patch_targets(&self, specs: &[PatchSpec], module: NonNull<u8>, size: usize) -> Result<Vec<GameplayHookTarget>, GameError> {
        // SAFETY: the caller holds the complete metadata/module span. No object or game method is accessed.
        unsafe {
            let images = self.images()?;
            let module_end = (module.as_ptr() as usize).checked_add(size).ok_or_else(|| unsupported("patch module range"))?;
            let mut targets = Vec::new();
            for spec in specs {
                let mut owner = self.class(&images, c"", spec.method.class)?;
                if let Some(prefix) = spec.state_machine {
                    let mut iterator = std::ptr::null_mut();
                    let mut selected = None;
                    let mut complete = false;
                    for _ in 0..4096 {
                        let nested = (self.class_nested)(owner, &raw mut iterator);
                        if nested.is_null() {
                            complete = true;
                            break;
                        }
                        let name = (self.class_name)(nested);
                        if name.is_null() {
                            return Err(unsupported("patch state machine name"));
                        }
                        if !metadata_text(name).to_bytes().starts_with(prefix.to_bytes()) {
                            continue;
                        }
                        if selected.replace(nested).is_some() {
                            return Err(unsupported("ambiguous patch state machine"));
                        }
                    }
                    if !complete {
                        return Err(unsupported("patch state machine bound"));
                    }
                    owner = selected.ok_or_else(|| unsupported("patch state machine missing"))?;
                }
                let method = self.hook_method_contract(owner, &spec.method)?;
                let address = read_value::<*mut u8>(method.cast());
                let start = address as usize;
                if start < module.as_ptr() as usize || start >= module_end || !(1..=0x10000).contains(&spec.search) {
                    return Err(unsupported("patch method range"));
                }
                let mut end = start.saturating_add(spec.search).min(module_end);
                // A managed method may span several Windows unwind records (its EH funclets). Keep the original
                // recipe's bounded window, additionally clipping it at the next declared method and the module end.
                for next in self.methods(owner) {
                    let next = read_value::<*mut u8>(next?.cast()) as usize;
                    if next > start {
                        end = end.min(next);
                    }
                }
                if end - start < 16 {
                    return Err(unsupported("patch method extent"));
                }
                let code = std::slice::from_raw_parts(address, end - start);
                let at = find_unique(code, spec.pattern, spec.at, spec.replacement).map_err(|error| unsupported(format!("{}: {error}", spec.name)))?;
                let change = CodeChange {
                    address: NonNull::new(address.add(at)).unwrap(),
                    expected: code[at..at + spec.replacement.len()].to_vec(),
                    replacement: spec.replacement.to_vec(),
                };
                let mut bytes = [0; 16];
                bytes.copy_from_slice(&code[..16]);
                targets.push(GameplayHookTarget {
                    method: ResolvedMethod {
                        name: spec.name.into(),
                        address: NonNull::new(address).unwrap(),
                        bytes,
                    },
                    effect: TargetEffect::Patch(change),
                });
            }
            Ok(targets)
        }
    }
}

pub(crate) mod any_mastery_passives;
pub(crate) mod class_experimentals;
pub(crate) mod endgame_cache;
pub(crate) mod equip_without_requirements;
pub(crate) mod favor_cache;
pub(crate) mod forge_above_level;
pub(crate) mod free_hunter_vendor;
pub(crate) mod full_set_bonus;
pub(crate) mod nemesis;
pub(crate) mod omen_windows;
pub(crate) mod pickup_anywhere;
pub(crate) mod rune_prison_complex;
pub(crate) mod skills_to_level_20;
pub(crate) mod unlimited_passive_points;
pub(crate) mod unlimited_skill_points;
pub(crate) mod unlimited_weaver_points;
pub(crate) mod unlimited_woven_enchanter;
