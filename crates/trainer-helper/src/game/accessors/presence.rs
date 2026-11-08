//! Cached metadata for the menu/play distinction; object and state values are
//! always read afresh on the verified Unity dispatcher thread.
use crate::game::{GameError, MethodError, MethodQuery, Others, RuntimeAccess, unsupported};
use il2cpp_runtime::{AttachedThread, InvocationContext, InvocationOutcome, ReturnValue, ValueTypeContract, raw};
use std::ptr::NonNull;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PlayPresence {
    Online,
    Menu,
    InGame,
    Unknown,
}

pub(crate) struct PresenceBindings {
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
    unsafe fn presence_bindings(&self) -> Result<PresenceBindings, GameError> {
        // SAFETY: descriptors belong to this attached, immutable supported runtime;
        // only exact static fields/methods and Int32 enum literals are retained.
        unsafe {
            let images = self.images()?;
            let environment = self.class(&images, c"EHG.Multiplayer", c"GameplayEnvironment")?;
            let online = self.static_field(environment, c"_isOnlinePlay", "System.Boolean")?;
            let owner = self.class(&images, c"ClientAppState", c"ClientStateManager")?;
            let query = MethodQuery {
                others: Others::Fail,
                instance: Some(false),
                refused: raw::METHOD_ATTRIBUTE_PINVOKE_IMPL | raw::METHOD_ATTRIBUTE_ABSTRACT | raw::METHOD_ATTRIBUTE_VIRTUAL,
                refuse_generic: true,
                refuse_inflated: true,
                ..MethodQuery::named(c"get_CurrentClientAppStateType", 0)
            };
            let getter = self.find_method(owner, &query).map_err(|error| match error {
                MethodError::Metadata(error) => error.into(),
                MethodError::Unnamed => unsupported("null app state getter name"),
                MethodError::Missing => unsupported("client app state getter missing"),
                MethodError::Untyped | MethodError::Twice | MethodError::Signature | MethodError::Parameters => unsupported("client app state getter signature/flags"),
            })?;
            let state_class = self.class(&images, c"ClientAppState", c"ClientAppStateType")?;
            self.enum_storage(state_class, c"Int32", raw::type_code::I4)?;
            if (self.type_class)((self.method_return)(getter)) != state_class {
                return Err(unsupported("client app state getter return type"));
            }
            let literal = |name| -> Result<i32, GameError> {
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
    pub unsafe fn play_presence(&self, attached: &AttachedThread<'_>, api: raw::InvocationApi) -> Result<PlayPresence, GameError> {
        // SAFETY: caller retains initialized runtime metadata and authorizes only
        // the checked static state getter; enum boxes remain rooted until copied.
        unsafe {
            let bindings = self.cached(|caches| &caches.presence, || self.presence_bindings())?;
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
