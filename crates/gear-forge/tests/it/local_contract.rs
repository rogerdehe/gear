//! Contract for [`Local`], the forge of a project that has none (T-78): every
//! call succeeds and does nothing, so a local-only project behaves exactly as
//! it did before the forge existed.

use gear_forge::{Filter, Forge, Local, MergeMethod, PrRef};

#[test]
fn a_local_forge_opens_nothing_and_says_so() {
    let forge = Local::new();
    assert!(forge
        .issues("owner/name", &Filter::default())
        .unwrap()
        .is_empty());

    let pr = forge
        .open_claim("owner/name", "base/t78", "dev", "a title", "a body")
        .expect("nothing to open");
    assert_eq!(pr, PrRef::none("owner/name"));
    assert!(!pr.is_open());

    forge
        .comment(&pr, "a comment")
        .expect("nothing to comment on");
    let merged = forge
        .merge(&pr, MergeMethod::Squash, "abc1234")
        .expect("nothing to merge");
    assert!(!merged.merged, "{merged:?}");
    assert_eq!(merged.message, "the project has no forge");
    forge
        .close_issue("owner/name", 12, "nothing to close")
        .expect("nothing to close");
}

#[test]
fn the_local_forge_is_the_default() {
    assert_eq!(Local, Local::default());
}
