//! Read-only runtime status.

use serde::Serialize;

use crate::{
    error::Result,
    scheduler::{ActiveMetadata, Runtime},
};

#[derive(Debug, Serialize)]
pub struct Status {
    pub schema_version: u8,
    pub capacity: usize,
    pub active: Vec<SlotStatus>,
    pub waiting: usize,
    pub max_waiting: usize,
    pub draining: bool,
}

#[derive(Debug, Serialize)]
pub struct SlotStatus {
    pub slot: usize,
    pub locked: bool,
    pub metadata: Option<ActiveMetadata>,
}

impl Status {
    pub fn collect(runtime: &Runtime, max_waiting: usize) -> Result<Self> {
        let capacity = runtime.capacity()?;
        let mut active = Vec::with_capacity(capacity);
        for slot in 0..capacity {
            active.push(SlotStatus {
                slot,
                locked: runtime.slot_locked(slot)?,
                metadata: runtime.active_metadata(slot)?,
            });
        }
        Ok(Self {
            schema_version: 1,
            capacity,
            active,
            waiting: runtime.live_waiters()?.len(),
            max_waiting,
            draining: runtime.is_draining(),
        })
    }

    #[must_use]
    pub fn human(&self) -> String {
        let active_count = self.active.iter().filter(|slot| slot.locked).count();
        let mut lines = vec![
            "policy: enforce".to_owned(),
            format!("heavy capacity: {}", self.capacity),
            format!("active: {active_count}/{}", self.capacity),
        ];
        for slot in &self.active {
            if let Some(metadata) = &slot.metadata {
                lines.push(format!(
                    "  slot-{}: job {}, pid {}, class {}",
                    slot.slot, metadata.job_id, metadata.supervisor_pid, metadata.label
                ));
            }
        }
        lines.push(format!("waiting: {}/{}", self.waiting, self.max_waiting));
        lines.push(format!(
            "draining: {}",
            if self.draining { "yes" } else { "no" }
        ));
        lines.join("\n")
    }
}
