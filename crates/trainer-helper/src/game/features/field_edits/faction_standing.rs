//! Rank, favor and reputation for the four factions, in FactionID order. Resolve through the player's provider,
//! require membership for changed factions, validate every requested rank/reputation pair before any write, then
//! run SetRank(false), SetFavorAndReputation and SaveAndSync(false). Unchanged factions are left alone.
use super::*;
use il2cpp_runtime::Argument;

const LOOKUP: MethodSpec = MethodSpec {
    class: c"FactionTracker",
    method: c"TryGetFaction",
    instance: true,
    params: &["LE.Factions.FactionID", "LE.Factions.Faction&"],
    returns: "System.Boolean",
};
fn method(name: &'static CStr, params: &'static [&'static str], returns: &'static str) -> MethodSpec {
    MethodSpec {
        class: c"Faction",
        method: name,
        instance: true,
        params,
        returns,
    }
}
fn validate(rank: i32, reputation: i32, member: bool, maximum: i32, next: Option<i32>) -> Result<(), WriteError> {
    if !member {
        return Err(WriteError::Refused("Join this faction in the game first".into()));
    }
    if rank > maximum {
        return Err(WriteError::Refused(format!("This faction's ranks go up to {maximum}")));
    }
    if next.is_some_and(|next| reputation >= next) {
        return Err(WriteError::Refused("Reputation must be below the next rank's requirement; raise the rank instead".into()));
    }
    Ok(())
}
impl RuntimeAccess {
    pub(super) unsafe fn standing_graph<'a, 'r>(&self, attached: &'a AttachedThread<'r>, manager: Root<'a, 'r>) -> Result<FieldGraph<'a, 'r>, WriteError> {
        // SAFETY: offline current manager; provider owns the existing faction references returned by these lookups.
        unsafe {
            let images = self.images()?;
            self.enum_storage(self.class(&images, c"LE.Factions", c"FactionID")?, c"Byte", raw::type_code::U1)?;
            let context = InvocationContext::new(attached);
            let finder = self.class(&images, c"", c"PlayerFinder")?;
            let getter = context
                .bind(self.field_method(finder, c"GetFactionInfoProviderInSingleplayerOrOnClient", 0)?)
                .map_err(|e| WriteError::Refused(e.to_string()))?;
            let provider = reference(returned(getter.invoke(None, &mut []))?)?;
            let mut entries = Vec::new();
            let mut values = Vec::new();
            for mut id in 0..4u8 {
                let faction = self.field_lookup(attached, &provider, &LOOKUP, &mut [(&raw mut id).cast()])?;
                let identity = faction.as_ref().map(target).transpose()?.map_or(0, |p| p.as_ptr() as u64);
                for name in [c"<Rank>k__BackingField", c"<Favor>k__BackingField", c"<Reputation>k__BackingField"] {
                    let value = if let Some(faction) = &faction { self.primitive::<i32>(faction, name)? } else { 0 };
                    values.push((identity, value));
                }
                if let Some(faction) = faction {
                    entries.push(faction);
                }
            }
            let snapshot = fixed_snapshot(FieldSet::FactionStanding, &manager, &provider, values)?;
            Ok(FieldGraph {
                _manager: manager,
                container: provider,
                entries,
                snapshot,
            })
        }
    }
    pub(super) unsafe fn apply_standing(&self, attached: &AttachedThread<'_>, graph: &FieldGraph<'_, '_>, values: &[Option<i32>]) -> Result<Snapshot, WriteError> {
        // SAFETY: source retains every faction; all dynamic limits and method signatures are checked before stores.
        unsafe {
            let context = InvocationContext::new(attached);
            let mut changes = Vec::new();
            for id in 0..4 {
                let before: [i32; 3] = std::array::from_fn(|field| graph.snapshot.fields[id * 3 + field].value);
                let wanted: [i32; 3] = std::array::from_fn(|field| values[id * 3 + field].unwrap_or(before[field]));
                if wanted == before {
                    continue;
                }
                let identity = graph.snapshot.fields[id * 3].identity;
                let faction = graph
                    .entries
                    .iter()
                    .find(|root| target(root).is_ok_and(|p| p.as_ptr() as u64 == identity))
                    .ok_or_else(|| WriteError::Refused("Join this faction in the game first".into()))?;
                let member = self.boolean(faction, c"<IsMember>k__BackingField")?;
                if !member {
                    return Err(WriteError::Refused("Join this faction in the game first".into()));
                }
                let data = self.field_child(attached, faction, c"<Data>k__BackingField")?;
                let maximum = integer(returned(self.field_call(&context, &data, c"get_MaxRank", 0)?.invoke(Some(&data), &mut []))?)?;
                let next = if wanted[0] < maximum {
                    Some(integer(returned(
                        self.field_call(&context, &data, c"ReputationRequiredForNextRank", 1)?
                            .invoke(Some(&data), &mut [Argument::Scalar(Scalar::I32(wanted[0]))]),
                    )?)?)
                } else {
                    None
                };
                validate(wanted[0], wanted[2], member, maximum, next)?;
                let rank = self.field_exact(&context, faction, &method(c"SetRank", &["System.Int32", "System.Boolean"], "System.Void"))?;
                let standing = self.field_exact(&context, faction, &method(c"SetFavorAndReputation", &["System.Int32", "System.Int32"], "System.Void"))?;
                let save = self.field_exact(&context, faction, &method(c"SaveAndSync", &["System.Boolean"], "System.Void"))?;
                changes.push((faction, before[0] != wanted[0], wanted, rank, standing, save));
            }
            self.field_gate(attached, graph.snapshot.manager)?;
            stored(
                || {
                    for (faction, rank_changed, wanted, rank, standing, save) in changes {
                        if rank_changed {
                            void(returned(rank.invoke(
                                Some(faction),
                                &mut [Argument::Scalar(Scalar::I32(wanted[0])), Argument::Scalar(Scalar::Boolean(false))],
                            ))?)?;
                            self.field_gate(attached, graph.snapshot.manager)?;
                        }
                        void(returned(standing.invoke(
                            Some(faction),
                            &mut [Argument::Scalar(Scalar::I32(wanted[1])), Argument::Scalar(Scalar::I32(wanted[2]))],
                        ))?)?;
                        self.field_gate(attached, graph.snapshot.manager)?;
                        void(returned(save.invoke(Some(faction), &mut [Argument::Scalar(Scalar::Boolean(false))]))?)?;
                        self.field_gate(attached, graph.snapshot.manager)?;
                    }
                    Ok(())
                },
                || self.read_field_set(attached, graph.snapshot.manager, graph.snapshot.set),
            )
        }
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn faction_membership_and_rank_reputation_pair_are_required() {
        assert!(validate(2, 99, true, 10, Some(100)).is_ok());
        assert!(validate(10, i32::MAX, true, 10, None).is_ok());
        assert!(validate(2, 0, false, 10, Some(100)).is_err());
        assert!(validate(11, 0, true, 10, None).is_err());
        assert!(validate(2, 100, true, 10, Some(100)).is_err());
    }
}
