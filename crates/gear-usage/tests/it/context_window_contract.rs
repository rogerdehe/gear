//! The context window of a seat's model, read from the CLI itself.
use gear_usage::{context_window_in, UsageLog};

#[test]
fn the_window_is_read_from_each_cli() {
    let home = tempfile::tempdir().unwrap();
    let pi = home.path().join(".pi/agent");
    std::fs::create_dir_all(&pi).unwrap();
    std::fs::write(
        pi.join("models.json"),
        r#"{"providers":{"hk-dgx-1":{"models":[{"id":"Flash","contextWindow":500000}]},"other":{"models":[{"id":"Flash","contextWindow":7}]}}}"#,
    )
    .unwrap();
    let oc = home.path().join(".config/opencode");
    std::fs::create_dir_all(&oc).unwrap();
    std::fs::write(
        oc.join("opencode.json"),
        r#"{"provider":{"hk-dgx-1":{"models":{"Flash":{"limit":{"context":600000}}}}}}"#,
    )
    .unwrap();
    let at = home.path();
    assert_eq!(
        context_window_in(UsageLog::PiJsonl, None, "hk-dgx-1/Flash", at),
        Some(500000)
    );
    assert_eq!(
        context_window_in(UsageLog::PiJsonl, None, "hk-dgx-1/Other", at),
        None
    );
    assert_eq!(
        context_window_in(UsageLog::OpencodeDb, None, "hk-dgx-1/Flash", at),
        Some(600000)
    );
    assert_eq!(
        context_window_in(UsageLog::ClaudeJsonl, None, "claude-opus-5-5[1m]", at),
        Some(1_000_000)
    );
    assert_eq!(
        context_window_in(UsageLog::ClaudeJsonl, None, "claude-sonnet-5-5", at),
        Some(200_000)
    );
    assert_eq!(context_window_in(UsageLog::AgyJsonl, None, "x", at), None);
    assert_eq!(
        context_window_in(UsageLog::CodexRollout, None, "x", at),
        None
    );
}
