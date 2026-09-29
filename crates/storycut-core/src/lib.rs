//! StoryCut shared timeline model and transaction engine.

mod model;
mod store;
mod transaction;
mod validation;

pub use model::*;
pub use store::{PlannedApplyResult, ProjectStore, StoreError};
pub use transaction::{
    ApplyResult, KeyframePolicy, Operation, OverlapResolution, RemoveMode, SubtitleEditMode,
    TrackChanges, Transaction,
};
pub use validation::{CoreError, CoreErrorCode, ProjectValidationError, validate_project};
