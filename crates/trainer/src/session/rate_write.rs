use super::*;
use crate::{
    fields::{AttackLeechPlan, RateWriteRecord, RateWriteStatus},
    write::{AllowWrite, WriteControl, WriteObservation},
};

impl<T: Transport> Session<T> {
    pub fn last_rate_write(&self) -> Option<&RateWriteRecord> {
        self.last_rate_write.as_ref()
    }

    /// Convenience for the current field planner; the lifecycle is shared.
    pub async fn set_attack_leech_rate(&mut self, plan: &AttackLeechPlan) -> Result<RateWriteStatus, SessionError> {
        self.set_attack_leech_rate_controlled(plan, &mut AllowWrite).await
    }
    pub async fn set_attack_leech_rate_controlled(&mut self, plan: &AttackLeechPlan, control: &mut dyn WriteControl) -> Result<RateWriteStatus, SessionError> {
        Ok(self.write_controlled(&plan.clone().into(), control).await?.map(WriteObservation::into_field))
    }
}
