//! The forge of a project that has none (T-78).
//!
//! A project may keep its work in GEAR alone: no issues to import, no pull
//! requests to open. [`Local`] is that forge — every call succeeds and does
//! nothing, so the engine calls the same methods in both modes and a
//! local-only project behaves exactly as it did before the forge existed.

use crate::{Filter, Forge, ForgeResult, Issue, MergeMethod, MergeResult, PrRef};

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
    fn merge(&self, _pr: &PrRef, _method: MergeMethod) -> ForgeResult<MergeResult> {
        Ok(MergeResult {
            merged: false,
            message: "the project has no forge".to_string(),
        })
    }

    fn close_issue(&self, _repo: &str, _number: u64, _comment: &str) -> ForgeResult<()> {
        Ok(())
    }
}
