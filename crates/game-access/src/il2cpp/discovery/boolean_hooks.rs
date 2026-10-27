use super::*;
use eg_trainer_wire::hooks::BooleanHook;

impl RuntimeAccess {
    /// Resolve one supported Boolean method, without invoking its body.
    /// Arguments are forwarded untouched by the leaf gateway, so their layouts
    /// need no adapter. Ambiguous overloads and shared generic bodies are refused.
    ///
    /// # Safety
    /// Same retained metadata, attached thread, MethodInfo layout and readable
    /// GameAssembly span contract as `discover`. Exclusive patch ownership is
    /// additionally required before using the returned code address.
    pub unsafe fn boolean_hook_target(
        &self,
        attached: &AttachedThread<'_>,
        hook: BooleanHook,
        module_base: NonNull<u8>,
        module_size: usize,
    ) -> Result<(NonNull<u8>, [u8; 16]), DiscoveryError> {
        let _ = attached; // Carries the caller's thread attachment for this metadata query.
        // SAFETY: caller supplies immutable metadata and a bounded retained module.
        unsafe {
            let (class, name, count) = match hook {
                BooleanHook::MysteriousRift => (c"TimeBeastTracker", c"NextEncounterShouldBeEggs", 0),
                BooleanHook::Offhand => (c"OffhandItemContainer", c"CanReceiveItem", 2),
            };
            let owner = self.class(&self.images()?, c"", class)?;
            let mut found = None;
            for method in self.methods(owner)? {
                let actual = (self.method_name)(method);
                if actual.is_null() {
                    return Err(unsupported("null Boolean hook method name"));
                }
                if metadata_text(actual) != name || (self.method_params)(method) != count {
                    continue;
                }
                let returns = (self.method_return)(method);
                let mut implementation = 0;
                if found.is_some()
                    || !(self.method_instance)(method)
                    || (self.method_generic)(method)
                    || (self.method_inflated)(method)
                    || (self.method_flags)(method, &mut implementation) & 0x2400 != 0
                    || returns.is_null()
                    || (self.type_byref)(returns)
                    || self.type_name(returns)? != "System.Boolean"
                {
                    return Err(unsupported(format!("Boolean hook contract: {}", hook.id())));
                }
                found = Some(method);
            }
            let method = found.ok_or_else(|| unsupported(format!("missing Boolean hook: {}", hook.id())))?;
            let address = read_value::<*mut u8>(method.cast());
            (address as usize)
                .checked_sub(module_base.as_ptr() as usize)
                .filter(|offset| offset.checked_add(16).is_some_and(|end| end <= module_size))
                .ok_or_else(|| unsupported("Boolean hook module range"))?;
            Ok((NonNull::new(address).ok_or_else(|| unsupported("Boolean hook address"))?, read_value(address.cast())))
        }
    }
}
