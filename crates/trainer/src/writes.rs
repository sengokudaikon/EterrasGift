//! Process, generation and catalog bound plans for the supported native Forge edits.
//! Construction is pure. Only an explicit submission may execute a plan; neither
//! failure nor a lost reply permits a retry. An address here is an opaque token.

use crate::field_writes::AttackLeechPlan;
use crate::{
    catalog::GameCatalog,
    edits::{self, CatalogProvenance, EditError, ExpectedObservation, ItemEdit, PreparedEdit, StatusEdit, StatusField},
};
pub use eg_game_access::protocol::ResponseCode;
use eg_game_access::{fields::AttackLeechObservation, forge::ForgeSnapshot};

/// Checked after potentially lengthy local validation and immediately before
/// entering transport. A false result cancels only work that has not been sent.
pub trait WriteControl: Send {
    fn allowed(&mut self) -> bool;
}
pub(crate) struct AllowWrite;
impl WriteControl for AllowWrite {
    fn allowed(&mut self) -> bool {
        true
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ForgePlan {
    prepared: PreparedEdit,
    manager: u64,
}
impl ForgePlan {
    pub fn level_requirement(expected: &ExpectedObservation, manager: u64, catalog: &GameCatalog, provenance: &CatalogProvenance, value: i32) -> Result<Self, EditError> {
        Self::new(expected, manager, catalog, provenance, StatusField::LevelRequirement, value)
    }
    pub fn new(expected: &ExpectedObservation, manager: u64, catalog: &GameCatalog, provenance: &CatalogProvenance, field: StatusField, value: i32) -> Result<Self, EditError> {
        Self::prepare(
            expected,
            manager,
            catalog,
            provenance,
            StatusEdit {
                field,
                value,
                unrestricted: false,
            },
            false,
        )
    }
    pub fn prepare(
        expected: &ExpectedObservation,
        manager: u64,
        catalog: &GameCatalog,
        provenance: &CatalogProvenance,
        edit: StatusEdit,
        override_enabled: bool,
    ) -> Result<Self, EditError> {
        if manager == 0 {
            return Err(EditError::InvalidObservation);
        }
        Self::edit(expected, manager, catalog, provenance, ItemEdit::Status(edit), override_enabled)
    }
    pub fn edit(
        expected: &ExpectedObservation,
        manager: u64,
        catalog: &GameCatalog,
        provenance: &CatalogProvenance,
        edit: ItemEdit,
        override_enabled: bool,
    ) -> Result<Self, EditError> {
        if manager == 0
            || !matches!(
                edit,
                ItemEdit::Status(_)
                    | ItemEdit::BaseType(_)
                    | ItemEdit::UniqueIdentity(_)
                    | ItemEdit::Roll(edits::RollEdit {
                        target: edits::RollTarget::Affix,
                        ..
                    })
            )
        {
            return Err(EditError::InvalidValues);
        }
        let prepared = edits::prepare(expected, catalog, provenance, edit, override_enabled, None)?;
        Ok(Self { prepared, manager })
    }
    pub fn before(&self) -> &ExpectedObservation {
        self.prepared.before()
    }
    pub fn manager(&self) -> u64 {
        self.manager
    }
    pub fn edit_intent(&self) -> &ItemEdit {
        self.prepared.edit()
    }
    pub fn mutation(&self) -> eg_game_access::write::ForgeMutation {
        use eg_game_access::write::ForgeMutation;
        let item = self.projected();
        match self.edit_intent() {
            ItemEdit::Roll(edit) => ForgeMutation::AffixRolls {
                rolls: eg_game_access::write::AffixRolls::new(
                    &edit
                        .changes
                        .iter()
                        .map(|c| eg_game_access::write::AffixRollChange {
                            slot: c.index as u8,
                            value: c.value as u8,
                        })
                        .collect::<Vec<_>>(),
                )
                .expect("validated affix roll batch"),
                unrestricted: edit.allow_incompatible,
            },
            ItemEdit::Status(edit) => ForgeMutation::Status {
                field: edit.field,
                value: edit.field.value(item) as u8,
                unrestricted: edit.unrestricted,
            },
            ItemEdit::BaseType(_) => ForgeMutation::Base {
                item_type: item.item_type,
                sub_type: item.sub_type,
                unrestricted: self.unrestricted(),
            },
            ItemEdit::UniqueIdentity(_) => ForgeMutation::Identity {
                id: item.identity.as_ref().expect("validated identity edit").id,
                sub_type: item.sub_type as u8,
                rarity: item.rarity,
                unrestricted: self.unrestricted(),
            },
            _ => unreachable!("ForgePlan only admits implemented native edits"),
        }
    }
    pub fn unrestricted(&self) -> bool {
        self.edit_intent().needs_override()
    }
    pub fn matches(&self, actual: &ForgeSnapshot) -> bool {
        if matches!(
            self.edit_intent(),
            ItemEdit::Status(StatusEdit {
                field: StatusField::LevelRequirement,
                ..
            })
        ) {
            actual == self.projected()
        } else {
            self.prepared.inspect_readback(Some(actual)).confirmed()
        }
    }
    pub fn projected(&self) -> &ForgeSnapshot {
        self.prepared.projected()
    }
    pub fn recheck(&self, current: &ExpectedObservation, manager: u64) -> Result<(), EditError> {
        self.recheck_with_override(current, manager, false)
    }
    pub fn recheck_with_override(&self, current: &ExpectedObservation, manager: u64, override_enabled: bool) -> Result<(), EditError> {
        if manager != self.manager {
            return Err(EditError::IdentityChanged);
        }
        self.prepared.recheck(current, true, override_enabled)
    }
}

/// A write status remains available even after the associated connection closes.
/// Pending is set before entering transport code. If its future is abandoned,
/// cleanup converts it to Uncertain; it never becomes NotApplied by cancellation.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum WriteOutcome<T> {
    Pending,
    Confirmed(T),
    /// A complete independent observation of the same target differs from the
    /// request. This is known state, not proof that the setter never ran.
    NotKept(T),
    NotApplied {
        code: ResponseCode,
        message: String,
    },
    Uncertain {
        message: String,
    },
}
impl<T> WriteOutcome<T> {
    pub fn uncertain(message: impl Into<String>) -> Self {
        Self::Uncertain { message: message.into() }
    }
    pub fn map<U>(self, mut map: impl FnMut(T) -> U) -> WriteOutcome<U> {
        match self {
            Self::Pending => WriteOutcome::Pending,
            Self::Confirmed(actual) => WriteOutcome::Confirmed(map(actual)),
            Self::NotKept(actual) => WriteOutcome::NotKept(map(actual)),
            Self::NotApplied { code, message } => WriteOutcome::NotApplied { code, message },
            Self::Uncertain { message } => WriteOutcome::Uncertain { message },
        }
    }
}
pub type WriteStatus = WriteOutcome<ForgeSnapshot>;
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WriteRecord<P = ForgePlan, T = ForgeSnapshot> {
    pub plan: P,
    pub status: WriteOutcome<T>,
}

/// The service accepts domain plans; native operation numbers stay internal.
/// More supported edits/field sets extend these variants, not the outcome model.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum WritePlan {
    Forge(Box<ForgePlan>),
    Field(Box<AttackLeechPlan>),
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum WriteObservation {
    Forge(Box<ForgeSnapshot>),
    Field(AttackLeechObservation),
}
impl WriteObservation {
    pub(crate) fn into_forge(self) -> ForgeSnapshot {
        match self {
            Self::Forge(actual) => *actual,
            _ => unreachable!("forge plan returned field data"),
        }
    }
    pub(crate) fn into_field(self) -> AttackLeechObservation {
        match self {
            Self::Field(actual) => actual,
            _ => unreachable!("field plan returned forge data"),
        }
    }
    pub(crate) fn forge(actual: ForgeSnapshot) -> Self {
        Self::Forge(Box::new(actual))
    }
}
impl WritePlan {
    fn matches(&self, actual: &WriteObservation) -> bool {
        match (self, actual) {
            (Self::Forge(plan), WriteObservation::Forge(actual)) => plan.matches(actual),
            (Self::Field(plan), WriteObservation::Field(actual)) => actual == plan.projected(),
            _ => false,
        }
    }
    pub fn projected(&self) -> WriteObservation {
        match self {
            Self::Forge(plan) => WriteObservation::Forge(Box::new(plan.projected().clone())),
            Self::Field(plan) => WriteObservation::Field(*plan.projected()),
        }
    }
    /// Compare only identity here. Full value equality is checked separately;
    /// neither tokens nor this check establish native lifetime across requests.
    pub(crate) fn same_target(&self, actual: &WriteObservation) -> bool {
        match (self, actual) {
            (Self::Forge(plan), WriteObservation::Forge(actual)) => actual.item == plan.before().snapshot().item && actual.individual_id == plan.before().snapshot().individual_id,
            (Self::Field(plan), WriteObservation::Field(actual)) => {
                let expected = plan.projected();
                actual.manager == expected.manager && actual.actor == expected.actor && actual.stats == expected.stats
            }
            _ => false,
        }
    }
    pub(crate) fn observe(&self, actual: WriteObservation) -> WriteOutcome<WriteObservation> {
        if self.matches(&actual) {
            WriteOutcome::Confirmed(actual)
        } else if self.same_target(&actual) {
            WriteOutcome::NotKept(actual)
        } else {
            WriteOutcome::uncertain("independent readback belongs to a different target")
        }
    }
    pub(crate) fn completed(&self, outcome: WriteOutcome<WriteObservation>) -> WriteOutcome<WriteObservation> {
        match outcome {
            WriteOutcome::Confirmed(actual) if self.matches(&actual) => WriteOutcome::Confirmed(actual),
            WriteOutcome::NotKept(actual) if self.same_target(&actual) && !self.matches(&actual) => WriteOutcome::NotKept(actual),
            WriteOutcome::Confirmed(_) | WriteOutcome::NotKept(_) => WriteOutcome::uncertain("connection returned a mismatched write outcome"),
            WriteOutcome::Pending => WriteOutcome::uncertain("connection returned before write completion"),
            WriteOutcome::NotApplied {
                code: ResponseCode::Done | ResponseCode::Failed,
                ..
            } => WriteOutcome::uncertain("connection mislabeled a success/failure code as pre-store refusal"),
            outcome => outcome,
        }
    }
}
impl From<ForgePlan> for WritePlan {
    fn from(plan: ForgePlan) -> Self {
        Self::Forge(Box::new(plan))
    }
}
impl From<AttackLeechPlan> for WritePlan {
    fn from(plan: AttackLeechPlan) -> Self {
        Self::Field(Box::new(plan))
    }
}
