use super::*;
use crate::{
    field_writes::{AttackLeechPlan, RateWriteRecord, RateWriteStatus},
    writes::{AllowWrite, WriteControl, WriteObservation},
};

impl<T: Transport, L: LogSource> Session<T, L> {
    pub fn last_rate_write(&self) -> Option<&RateWriteRecord> {
        self.last_rate_write.as_ref()
    }

    /// Convenience for the current field planner; the lifecycle is shared.
    pub async fn set_attack_leech_rate(&mut self, now: Timestamp, plan: &AttackLeechPlan) -> Result<RateWriteStatus, SessionError> {
        self.set_attack_leech_rate_controlled(now, plan, &mut AllowWrite).await
    }
    pub async fn set_attack_leech_rate_controlled(&mut self, now: Timestamp, plan: &AttackLeechPlan, control: &mut dyn WriteControl) -> Result<RateWriteStatus, SessionError> {
        Ok(self.write_controlled(now, &plan.clone().into(), control).await?.map(WriteObservation::into_field))
    }
}
