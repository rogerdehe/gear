//! Fixed caller context (design/03).
//!
//! The engine exports `GEAR_CTX=<project>:<seat>:<generation>` in the pane
//! shell before starting an agent, so the agent and every `gear` it runs
//! inherit it. `gear` never looks up "the latest generation" for a pane.

use crate::error::{ErrorCode, GearError, GearResult};

pub const ENV_CTX: &str = "GEAR_CTX";
pub const ENV_DB: &str = "GEAR_DB";
pub const ENV_ADMIN: &str = "GEAR_ADMIN";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GearCtx {
    pub project: String,
    /// Seat name within the project, e.g. `captain`, `heat`, `R-4`.
    pub seat: String,
    pub generation: i64,
}

impl GearCtx {
    /// Seat primary key as stored: `<project>:<seat>`.
    pub fn seat_id(&self) -> String {
        format!("{}:{}", self.project, self.seat)
    }

    pub fn encode(&self) -> String {
        format!("{}:{}:{}", self.project, self.seat, self.generation)
    }

    pub fn parse(s: &str) -> GearResult<GearCtx> {
        let bad = || GearError::new(ErrorCode::BadFormat, format!("{ENV_CTX} malformed: {s:?}"));
        let mut parts = s.splitn(3, ':');
        let project = parts.next().filter(|p| valid_name(p)).ok_or_else(bad)?;
        let seat = parts.next().filter(|p| valid_name(p)).ok_or_else(bad)?;
        let generation = parts
            .next()
            .and_then(|g| g.parse::<i64>().ok())
            .filter(|g| *g > 0)
            .ok_or_else(bad)?;
        Ok(GearCtx {
            project: project.to_string(),
            seat: seat.to_string(),
            generation,
        })
    }
}

/// Project and seat names: `[A-Za-z0-9_-]{1,32}` (also valid Herdr metadata keys/names).
pub fn valid_name(s: &str) -> bool {
    !s.is_empty()
        && s.len() <= 32
        && s.bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip() {
        let c = GearCtx {
            project: "harbor".into(),
            seat: "R-4".into(),
            generation: 7,
        };
        assert_eq!(GearCtx::parse(&c.encode()).unwrap(), c);
        assert_eq!(c.seat_id(), "harbor:R-4");
    }

    #[test]
    fn rejects_malformed() {
        for s in ["", "a", "a:b", "a:b:0", "a:b:x", "a b:c:1", "a::1"] {
            assert!(GearCtx::parse(s).is_err(), "{s}");
        }
    }
}
