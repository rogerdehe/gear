//! The forge of a project that has none (T-78).
//!
//! A project may keep its work in GEAR alone: no issues to import, no pull
//! requests to open. [`Local`] is that forge — every call succeeds and does
//! nothing, so the engine calls the same methods in both modes and a
//! local-only project behaves exactly as it did before the forge existed.

use crate::{Filter, Forge, ForgeResult, Issue, MergeMethod, MergeResult, PrRef, PrState};

/// A project without a forge: every call is a no-op that succeeds.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Local;

impl Local {
    pub fn new() -> Local {
        Local
    }
}

impl Forge for Local {
    fn issues(&self, _repo: &str, _filter: &Filter) -> ForgeResult<Vec<Issue>> {
        Ok(Vec::new())
    }

    /// Nothing is opened: [`PrRef::is_open`] is false and every later call on
    /// the ref is a no-op too.
    fn open_claim(
        &self,
        repo: &str,
        _branch: &str,
        _base: &str,
        _title: &str,
        _body: &str,
    ) -> ForgeResult<PrRef> {
        Ok(PrRef::none(repo))
    }

    fn comment(&self, _pr: &PrRef, _body: &str) -> ForgeResult<()> {
        Ok(())
    }

    /// Nothing is merged here: the merge is GEAR's own, as before the forge.
    fn merge(&self, _pr: &PrRef, _method: MergeMethod, _head: &str) -> ForgeResult<MergeResult> {
        Ok(MergeResult {
            merged: false,
            message: "the project has no forge".to_string(),
        })
    }

    /// A pull request of a project without a forge is never open: nothing is
    /// merged, so nothing is to reconcile.
    fn pr_state(&self, _pr: &PrRef) -> ForgeResult<PrState> {
        Ok(PrState::default())
    }

    /// There is no pull request to adopt.
    fn pr_by_head(&self, _repo: &str, _branch: &str) -> ForgeResult<Option<PrRef>> {
        Ok(None)
    }

    fn close_issue(&self, _repo: &str, _number: u64, _comment: &str) -> ForgeResult<()> {
        Ok(())
    }
}
