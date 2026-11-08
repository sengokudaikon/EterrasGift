//! What Settings shows: its module rows, their sections and the location boxes, built from the [`State`].

use super::{Location, State};
use eg_core::module::ModuleSection;
use phrases::Phrase;

impl State {
    pub fn rows(&self) -> Vec<ModuleRowView> {
        self.modules
            .iter()
            .map(|module| ModuleRowView {
                title: module.choice.title.get(),
                description: module.choice.description.get(),
                problem: module.choice.problem.clone().unwrap_or_default(),
                enabled: module.enabled,
            })
            .collect()
    }

    pub fn sections(&self) -> Vec<SectionView> {
        let mut sections = Vec::new();
        for (module_index, module) in self.modules.iter().enumerate().filter(|(_, module)| module.enabled_at_start) {
            for (section_index, section) in module.choice.sections.iter().enumerate() {
                sections.push(match section {
                    ModuleSection::Toggle {
                        key,
                        title,
                        label,
                        description,
                        note,
                        default,
                        on_text,
                        off_text,
                    } => SectionView {
                        module: module_index,
                        section: section_index,
                        title: title.get(),
                        description: description.get(),
                        body: SectionBody::Toggle {
                            label: label.get(),
                            note: note.map(Phrase::get).unwrap_or_default(),
                            on_text: on_text.get(),
                            off_text: off_text.get(),
                            // As the module reads it at start, so the switch shows what the app does. An edit on its way
                            // shows over it.
                            checked: self.section_edits.get(&(module_index, section_index)).map_or_else(
                                || eg_core::settings::module_flag(self.snapshot.store.desired().module_settings.get(module.choice.id), key, *default),
                                |(_, on)| *on,
                            ),
                        },
                    },
                    ModuleSection::Status {
                        title,
                        description,
                        text,
                        action,
                        tip,
                        unavailable,
                    } => {
                        let live = self.live.get(&(module_index, section_index));
                        let available = live.is_some_and(|live| live.can_act);
                        let enabled = available && !live.is_some_and(|live| live.busy);
                        SectionView {
                            module: module_index,
                            section: section_index,
                            title: title.get(),
                            description: description.get(),
                            body: SectionBody::Status {
                                status: live.map_or_else(|| text.get().to_owned(), |live| live.text.clone()),
                                action: action.get(),
                                action_enabled: enabled,
                                // The tip while the action can run; the reason beside it only while it is unavailable, not
                                // while it runs.
                                action_tip: if enabled { tip.get() } else { "" },
                                unavailable: if available { "" } else { unavailable.get() },
                            },
                        }
                    }
                });
            }
        }
        sections
    }

    pub fn locations(&self) -> Vec<LocationView> {
        Location::ALL
            .map(|location| {
                let [label, name, caption, browse, _] = location.words();
                LocationView {
                    label: label.get(),
                    name: name.get(),
                    value: self.location(location).to_owned(),
                    placeholder: self.placeholder(location).to_owned(),
                    caption: caption.get(),
                    browse_label: browse.get(),
                }
            })
            .to_vec()
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ModuleRowView {
    pub title: &'static str,
    pub description: &'static str,
    pub problem: String,
    pub enabled: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SectionView {
    pub module: usize,
    pub section: usize,
    pub title: &'static str,
    pub description: &'static str,
    pub body: SectionBody,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SectionBody {
    Toggle {
        label: &'static str,
        note: &'static str,
        on_text: &'static str,
        off_text: &'static str,
        checked: bool,
    },
    Status {
        status: String,
        action: &'static str,
        action_enabled: bool,
        action_tip: &'static str,
        unavailable: &'static str,
    },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LocationView {
    pub label: &'static str,
    pub name: &'static str,
    pub value: String,
    pub placeholder: String,
    pub caption: &'static str,
    pub browse_label: &'static str,
}
