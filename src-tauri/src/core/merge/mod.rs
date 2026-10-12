//! Object-level three-way merge of the skills library (merge-engine design,
//! `docs/merge-engine-design.md`). Phase 3d-α introduced protocol markers on
//! every app commit plus the engine itself; 3d-β made the object merge the
//! default, and Sync (SID-105) removed the `merge_engine=system` opt-out: a
//! line-level merge reads as an old-client write on every other device. Only
//! a remote written by pre-protocol clients still takes the legacy git merge.

pub mod apply;
pub mod decision;
#[cfg(test)]
mod integration_tests;
pub mod pending;
pub mod protocol;
pub mod resolve;
pub mod snapshot;
pub mod treebuild;
pub mod validate;

pub use apply::{object_merge_pull_unlocked, recover_on_startup, MergeSummary};
