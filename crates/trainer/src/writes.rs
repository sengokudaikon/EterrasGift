//! Process, generation and catalog bound plans for the first native forge write.
//! Construction is pure. Only an explicit submission may execute a plan; neither
//! failure nor a lost reply permits a retry. An address here is an opaque token.

use crate::{
    catalog::GameCatalog,
    edits::{self, CatalogProvenance, EditError, ExpectedObservation, ItemEdit, PreparedEdit, StatusEdit, StatusField},
};
use eg_game_access::{forge::ForgeSnapshot, protocol::ResponseCode};

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
pub struct LevelRequirementPlan {
    prepared: PreparedEdit,
    manager: u64,
}
impl LevelRequirementPlan {
    pub fn new(expected: &ExpectedObservation, manager: u64, catalog: &GameCatalog, provenance: &CatalogProvenance, value: i32) -> Result<Self, EditError> {
        if manager == 0 {
            return Err(EditError::InvalidObservation);
        }
        let prepared = edits::prepare(
            expected,
            catalog,
            provenance,
            ItemEdit::Status(StatusEdit {
                field: StatusField::LevelRequirement,
                value,
                unrestricted: false,
            }),
            false,
            None,
        )?;
        Ok(Self { prepared, manager })
    }
    pub fn before(&self) -> &ExpectedObservation {
        self.prepared.before()
    }
    pub fn manager(&self) -> u64 {
        self.manager
    }
    pub fn value(&self) -> i32 {
        self.prepared.projected().status.level_requirement
    }
    pub fn projected(&self) -> &ForgeSnapshot {
        self.prepared.projected()
    }
    pub fn recheck(&self, current: &ExpectedObservation, manager: u64) -> Result<(), EditError> {
        if manager != self.manager {
            return Err(EditError::IdentityChanged);
        }
        self.prepared.recheck(current, true, false)
    }
}

/// A write status remains available even after the associated connection closes.
/// Pending is set before entering transport code. If its future is abandoned,
/// cleanup converts it to Uncertain; it never becomes NotApplied by cancellation.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum WriteStatus {
    Pending,
    Confirmed(ForgeSnapshot),
    NotApplied { code: ResponseCode, message: String },
    Uncertain { message: String },
}
impl WriteStatus {
    pub fn uncertain(message: impl Into<String>) -> Self {
        Self::Uncertain { message: message.into() }
    }
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WriteRecord {
    pub plan: LevelRequirementPlan,
    pub status: WriteStatus,
}
