//! Cached metadata for the menu/play distinction; object and state values are
//! always read afresh on the verified Unity dispatcher thread.
use super::{DiscoveryError, RuntimeAccess, unsupported};
use crate::il2cpp::{AttachedThread, InvocationContext, InvocationOutcome, ReturnValue, ValueTypeContract, raw};
use std::ptr::NonNull;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PlayPresence {
    Online,
    Menu,
    InGame,
    Unknown,
}

pub(super) struct PresenceBindings {
    online: usize,
    getter: usize,
    state_class: usize,
    login: i32,
    select: i32,
    game: i32,
}

fn classify(online: bool, value: i32, login: i32, select: i32, game: i32) -> PlayPresence {
    if online {
        PlayPresence::Online
    } else if value == login || value == select {
        PlayPresence::Menu
    } else if value == game {
        PlayPresence::InGame
    } else {
        PlayPresence::Unknown
    }
}

impl RuntimeAccess {
    unsafe fn presence_bindings(&self) -> Result<PresenceBindings, DiscoveryError> {
        // SAFETY: descriptors belong to this attached, immutable supported runtime;
        // only exact static fields/methods and Int32 enum literals are retained.
        unsafe {
            let images = self.images()?;
            let environment = self.class(&images, c"EHG.Multiplayer", c"GameplayEnvironment")?;
            let online = self.static_field(environment, c"_isOnlinePlay", "System.Boolean")?;
            let owner = self.class(&images, c"ClientAppState", c"ClientStateManager")?;
            let mut getter = None;
            for candidate in self.methods(owner) {
                let candidate = candidate?;
                let name = (self.method_name)(candidate);
                if name.is_null() {
                    return Err(unsupported("null app state getter name"));
                }
                if super::metadata_text(name) != c"get_CurrentClientAppStateType" {
                    continue;
                }
                let mut implementation_flags = 0;
                if getter.is_some()
                    || (self.method_params)(candidate) != 0
                    || (self.method_instance)(candidate)
                    || (self.method_generic)(candidate)
                    || (self.method_inflated)(candidate)
                    || (self.method_flags)(candidate, &raw mut implementation_flags)
                        & (raw::METHOD_ATTRIBUTE_PINVOKE_IMPL | raw::METHOD_ATTRIBUTE_ABSTRACT | raw::METHOD_ATTRIBUTE_VIRTUAL)
                        != 0
                {
                    return Err(unsupported("client app state getter signature/flags"));
                }
                getter = Some(candidate);
            }
            let getter = getter.ok_or_else(|| unsupported("client app state getter missing"))?;
            let state_class = self.class(&images, c"ClientAppState", c"ClientAppStateType")?;
            self.enum_storage(state_class, c"Int32", 0x08)?;
            if (self.type_class)((self.method_return)(getter)) != state_class {
                return Err(unsupported("client app state getter return type"));
            }
            let literal = |name| -> Result<i32, DiscoveryError> {
                let field = self.field(state_class, name)?;
                let ty = (self.field_type)(field);
                if ty.is_null()
                    || (self.type_byref)(ty)
                    || (self.type_class)(ty) != state_class
                    || (self.field_parent)(field) != state_class
                    || (self.field_flags)(field) & (raw::FIELD_ATTRIBUTE_STATIC | raw::FIELD_ATTRIBUTE_LITERAL) != (raw::FIELD_ATTRIBUTE_STATIC | raw::FIELD_ATTRIBUTE_LITERAL)
                {
                    return Err(unsupported("client app state literal identity/storage"));
                }
                let mut value = 0_i32;
                (self.static_get)(field, (&raw mut value).cast());
                Ok(value)
            };
            let (login, select, game) = (literal(c"Login")?, literal(c"CharacterSelect")?, literal(c"InGame")?);
            if login == select || login == game || select == game {
                return Err(unsupported("client app state literals overlap"));
            }
            Ok(PresenceBindings {
                online: online as usize,
                getter: getter as usize,
                state_class: state_class as usize,
                login,
                select,
                game,
            })
        }
    }

    /// Read the online flag and ClientStateManager.CurrentClientAppStateType.
    /// Gameplay scene transitions remain InGame, including missing player wrappers.
    ///
    /// # Safety
    /// All status runtime/main-thread assumptions apply. `api` belongs to the same
    /// runtime; the zero-argument state getter and its runtime callbacks must be
    /// trusted, initialized, and non-reentrant through this synchronous invocation.
    pub unsafe fn play_presence(&self, attached: &AttachedThread<'_>, api: raw::InvocationApi) -> Result<PlayPresence, DiscoveryError> {
        // SAFETY: caller retains initialized runtime metadata and authorizes only
        // the checked static state getter; enum boxes remain rooted until copied.
        unsafe {
            let local;
            let bindings = if let Some(cache) = self.metadata {
                if cache.presence.get().is_none() {
                    let _ = cache.presence.set(self.presence_bindings()?);
                }
                cache.presence.get().unwrap()
            } else {
                local = self.presence_bindings()?;
                &local
            };
            let mut online = 1_u8;
            (self.static_get)(bindings.online as _, (&raw mut online).cast());
            if online > 1 {
                return Err(unsupported("online Boolean representation"));
            }
            if online != 0 {
                return Ok(PlayPresence::Online);
            }
            let context = InvocationContext::new(attached, api);
            let method = context
                .bind_with_value_types(
                    NonNull::new(bindings.getter as *mut raw::MethodInfo).unwrap(),
                    &[ValueTypeContract::I32Enum {
                        class: NonNull::new(bindings.state_class as *mut raw::Class).unwrap(),
                    }],
                )
                .map_err(|e| unsupported(e.to_string()))?;
            match method.invoke(None, &mut []).map_err(|e| unsupported(e.to_string()))? {
                InvocationOutcome::Returned(ReturnValue::EnumI32 { value, .. }) => Ok(classify(false, value, bindings.login, bindings.select, bindings.game)),
                _ => Err(unsupported("client app state getter failed or threw")),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn menu_and_game_states_use_runtime_literals_and_online_always_wins() {
        assert_eq!(classify(false, 17, 17, 31, 42), PlayPresence::Menu);
        assert_eq!(classify(false, 31, 17, 31, 42), PlayPresence::Menu);
        assert_eq!(classify(false, 42, 17, 31, 42), PlayPresence::InGame);
        assert_eq!(classify(false, -1, 17, 31, 42), PlayPresence::Unknown);
        assert_eq!(classify(true, 42, 17, 31, 42), PlayPresence::Online);
    }
}
