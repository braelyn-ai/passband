//! Model-owned triage, independent notification assessment, and ranking.
//!
//! The durable executor is `sync::triage_worker`. Legacy record types and
//! extraction modules remain for stored-data compatibility; ingest and workers
//! do not execute the retired classifier or deterministic detectors.

pub mod access;
pub mod agent;
pub mod agent_config;
pub mod calendar;
pub mod context;
pub mod deadline;
pub mod decision;
pub mod events;
pub mod extract;
pub mod llm;
pub mod money;
pub mod notify_llm;
pub mod ranking;
pub mod receipt;
pub mod receipt_match;
pub mod revisit;
pub mod router;
pub mod rule_infer;
pub mod rules;
pub mod shipment;
pub mod stage1_llm;
pub mod stage2;
pub(crate) mod text;

pub use calendar::{CalendarInfo, CalendarKind, detect_calendar};
pub use deadline::DeadlineHit;
pub use receipt::{ReceiptInfo, detect_receipt, recompute_total};
pub use shipment::{
    CarrierTrack, ShipmentInfo, ShipmentStatus, detect_shipment, is_ambiguous_tracking_shape,
};

/// Historical marker still read by migration of pre-agent extraction rows.
pub const NO_BODY_SKIP_MODEL: &str = "skip-no-body";

/// Historical age-skip marker, retained to interpret pre-agent records.
pub const STALE_SKIP_MODEL: &str = "stale-skip";
