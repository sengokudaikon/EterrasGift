//! Edit the character's saved run selected by the game's current-or-last-requested lookup, independent of the UI.
//! Reject standalone/foreign runs. Rebuild corruption modifiers, change Gaze through its method, save the runs, then
//! optionally redraw an already open matching panel. A missing/different panel never prevents a saved-run edit.
use super::*;
use il2cpp_runtime::Argument;

const CURRENT: MethodSpec = MethodSpec {
    class: c"MonolithRunsManager",
    method: c"TryGetCurrentOrMostRecentlyRequestedRun",
    instance: true,
    params: &["MonolithRun&"],
    returns: "System.Boolean",
};
const SAVED: MethodSpec = MethodSpec {
    class: c"MonolithRunsManager",
    method: c"TryGetRun",
    instance: true,
    params: &["TimelineID", "System.Int32", "MonolithRun&"],
    returns: "System.Boolean",
};
fn method(class: &'static CStr, name: &'static CStr, params: &'static [&'static str]) -> MethodSpec {
    MethodSpec {
        class,
        method: name,
        instance: true,
        params,
        returns: "System.Void",
    }
}
fn owns(selected: u64, saved: Option<u64>) -> Result<(), WriteError> {
    if saved != Some(selected) {
        Err(WriteError::NotReady("The selected run is not this character's saved timeline run".into()))
    } else {
        Ok(())
    }
}
impl RuntimeAccess {
    pub(super) unsafe fn run_graph<'a, 'r>(&self, attached: &'a AttachedThread<'r>, manager: Root<'a, 'r>) -> Result<FieldGraph<'a, 'r>, WriteError> {
        // SAFETY: the rooted actor's runs manager retains its existing run/web throughout both checked lookups.
        unsafe {
            let actor = self.field_child(attached, &manager, c"actor")?;
            let runs = self.field_child(attached, &actor, c"monolithRunsManager")?;
            let run = self
                .field_lookup(attached, &runs, &CURRENT, &mut [])?
                .ok_or_else(|| WriteError::NotReady("No Monolith run selected; open a timeline and choose its difficulty".into()))?;
            if self.child(attached, &run, c"timeline")?.is_none() {
                return Err(WriteError::NotReady("This echo is not part of a Monolith timeline".into()));
            }
            let images = self.images()?;
            let timeline_id = self.class(&images, c"", c"TimelineID")?;
            self.enum_storage(timeline_id, c"Byte", raw::type_code::U1)?;
            let owner = (self.object_class)(target(&run)?.as_ptr());
            let field = self.instance_field_in(owner, c"timelineID", timeline_id, "timeline ID")?;
            let mut id = 0u8;
            (self.field_get)(target(&run)?.as_ptr(), field, (&raw mut id).cast());
            let mut difficulty = self.primitive::<i32>(&run, c"difficultyIndex")?;
            let saved = self.field_lookup(attached, &runs, &SAVED, &mut [(&raw mut id).cast(), (&raw mut difficulty).cast()])?;
            owns(target(&run)?.as_ptr() as u64, saved.as_ref().map(target).transpose()?.map(|p| p.as_ptr() as u64))?;
            let web = self.field_child(attached, &run, c"web")?;
            let values = [
                (target(&web)?.as_ptr() as u64, self.primitive::<i32>(&web, c"corruption")?),
                (target(&web)?.as_ptr() as u64, self.primitive::<i32>(&web, c"gazeOfOrobyss")?),
                (target(&run)?.as_ptr() as u64, i32::from(self.boolean(&run, c"hasRerolledOptions")?)),
            ];
            let snapshot = fixed_snapshot(FieldSet::MonolithRun, &manager, &runs, values)?;
            Ok(FieldGraph {
                _manager: manager,
                container: runs,
                entries: vec![run, web],
                snapshot,
            })
        }
    }
    pub(super) unsafe fn apply_run(
        &self,
        attached: &AttachedThread<'_>,
        writes: raw::FieldWriteApi,
        graph: &FieldGraph<'_, '_>,
        values: &[Option<i32>],
    ) -> Result<Snapshot, WriteError> {
        // SAFETY: all destinations and exact save/recalculation methods are prepared before the first store.
        unsafe {
            let context = InvocationContext::new(attached);
            let (run, web) = (&graph.entries[0], &graph.entries[1]);
            let run_class = (self.object_class)(target(run)?.as_ptr());
            let web_class = (self.object_class)(target(web)?.as_ptr());
            let corruption = self.mutable_field(web_class, c"corruption", c"Int32", raw::type_code::I4)?;
            let rerolled = self.mutable_field(run_class, c"hasRerolledOptions", c"Boolean", raw::type_code::BOOLEAN)?;
            let recalc = self.field_exact(&context, run, &method(c"MonolithRun", c"updateCorruptionMod", &[]))?;
            let gaze = self.field_exact(&context, web, &method(c"EchoWeb", c"ModifyGazeOfOrobyss", &["System.Int32"]))?;
            let save = self.field_exact(&context, &graph.container, &method(c"MonolithRunsManager", c"saveRuns", &[]))?;
            self.field_gate(attached, graph.snapshot.manager)?;
            stored(
                || {
                    if let Some(value) = values[0].filter(|value| *value != graph.snapshot.fields[0].value) {
                        writes.set_i32(target(web)?, corruption, value);
                        void(returned(recalc.invoke(Some(run), &mut []))?)?;
                        self.field_gate(attached, graph.snapshot.manager)?;
                    }
                    if let Some(value) = values[1].filter(|value| *value != graph.snapshot.fields[1].value) {
                        let delta = value - graph.snapshot.fields[1].value;
                        void(returned(gaze.invoke(Some(web), &mut [Argument::Scalar(Scalar::I32(delta))]))?)?;
                        self.field_gate(attached, graph.snapshot.manager)?;
                    }
                    if let Some(value) = values[2] {
                        writes.set_u8(target(run)?, rerolled, value as u8);
                    }
                    void(returned(save.invoke(Some(&graph.container), &mut []))?)?;
                    self.field_gate(attached, graph.snapshot.manager)?;
                    // Refresh is optional: an absent/loading UI or a redraw exception does not undo a successful save.
                    let _ = self.refresh_run_panel(attached, run);
                    Ok(())
                },
                || self.read_field_set(attached, graph.snapshot.manager, graph.snapshot.set),
            )
        }
    }
    unsafe fn refresh_run_panel(&self, attached: &AttachedThread<'_>, run: &Root<'_, '_>) -> Result<(), WriteError> {
        // SAFETY: admitted main thread, rooted UI graph, native pointers compared only. Never open or replace a panel.
        unsafe {
            let images = self.images()?;
            let ui_class = self.class(&images, c"", c"UIBase")?;
            let Some(ui) = self.static_object(attached, ui_class, c"instance", ui_class)? else {
                return Ok(());
            };
            if self.primitive::<usize>(&ui, c"m_CachedPtr")? == 0 {
                return Ok(());
            }
            let Some(panels) = self.child(attached, &ui, c"_panelSystem")? else { return Ok(()) };
            let panel_class = self.class(&images, c"LE.UI.PanelSystem", c"PanelSystem")?;
            let location_class = self.class(&images, c"LE.UI.PanelSystem", c"PanelLocation")?;
            let field = self.static_field(panel_class, c"AllPanelLocations", "LE.UI.PanelSystem.PanelLocation[]")?;
            let Some(locations) = NonNull::new(self.static_reference(field)) else { return Ok(()) };
            let locations = attached.root(locations).map_err(GameError::from)?;
            let length = self.array_count(&locations, location_class, 16)?;
            self.enum_storage(location_class, c"Int32", raw::type_code::I4)?;
            let values = copy_values::<i32>(array_data(target(&locations)?.as_ptr()).cast(), length as usize);
            let context = InvocationContext::new(attached);
            let get = self.field_method((self.object_class)(target(&panels)?.as_ptr()), c"GetActivePanel", 1)?;
            let class = NonNull::new(location_class).unwrap();
            let get = context
                .bind_with_value_types(get, &[ValueTypeContract::I32Enum { class }])
                .map_err(|e| WriteError::Refused(e.to_string()))?;
            let monolith_class = self.class(&images, c"LE.UI.PanelSystem", c"MonolithPanel")?;
            for location in values {
                let panel = match returned(get.invoke(Some(&panels), &mut [Argument::EnumI32 { class, value: location }])) {
                    Ok(ReturnValue::Reference(Some(panel))) => panel,
                    _ => continue,
                };
                if (self.object_class)(target(&panel)?.as_ptr()) != monolith_class {
                    continue;
                }
                let Some(timeline) = self.child(attached, &panel, c"_timelineManager")? else {
                    return Ok(());
                };
                if self.primitive::<usize>(&timeline, c"m_CachedPtr")? == 0 {
                    return Ok(());
                }
                let Some(shown) = self.child(attached, &timeline, c"run")? else { return Ok(()) };
                if target(&shown)? != target(run)? {
                    return Ok(());
                }
                let can_place = boolean(returned(
                    self.field_call(&context, &timeline, c"get_CanPlaceWovenEchoes", 0)?.invoke(Some(&timeline), &mut []),
                )?)?;
                let refresh = self.field_exact(
                    &context,
                    &timeline,
                    &method(
                        c"MonolithTimelinePanelManager",
                        c"SetRunInfo",
                        &["MonolithRun", "System.Boolean", "System.Boolean", "System.Int32"],
                    ),
                )?;
                void(returned(refresh.invoke(
                    Some(&timeline),
                    &mut [
                        Argument::Reference(Some(run)),
                        Argument::Scalar(Scalar::Boolean(true)),
                        Argument::Scalar(Scalar::Boolean(can_place)),
                        Argument::Scalar(Scalar::I32(-1)),
                    ],
                ))?)?;
                break;
            }
            Ok(())
        }
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn selected_run_must_be_the_saved_run_not_another_or_standalone_run() {
        assert!(owns(10, Some(10)).is_ok());
        assert!(owns(10, Some(11)).is_err());
        assert!(owns(10, None).is_err());
    }
}
