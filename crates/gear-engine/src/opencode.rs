//! Session-owned OpenCode models (T-44). No terminal keys on the API path.

use gear_app::config::ProjectConfig;
use gear_herdr::HerdrAct;
use gear_store::Store;
use serde_json::{json, Value};

pub(crate) fn data(value: &Value) -> &Value {
    value.get("data").unwrap_or(value)
}

/// Invalid model/variant is a configuration fault, not permission to silently
/// select another model or let the TUI discard an unknown variant.
pub(crate) fn target(
    herdr: &mut dyn HerdrAct,
    model: &str,
    variant: Option<&str>,
    directory: Option<&str>,
) -> Result<Value, String> {
    let (provider, id) = model
        .split_once('/')
        .ok_or("invalid OpenCode model: needs provider/id")?;
    let mut args = vec!["model.list".into()];
    if let Some(directory) = directory {
        args.extend(["--param".into(), format!("location[directory]={directory}")]);
    }
    let catalog = herdr.opencode_api(&args)?;
    let info = data(&catalog)
        .as_array()
        .and_then(|models| {
            models
                .iter()
                .find(|m| m["providerID"] == provider && m["id"] == id)
        })
        .ok_or_else(|| format!("invalid OpenCode model {model}: absent from model.list"))?;
    if let Some(variant) = variant {
        if !info["variants"]
            .as_array()
            .is_some_and(|variants| variants.iter().any(|v| v["id"] == variant))
        {
            return Err(format!(
                "invalid OpenCode variant {variant} for {model}: absent from model.list"
            ));
        }
    }
    let mut target = json!({"providerID": provider, "id": id});
    if let Some(variant) = variant {
        target["variant"] = variant.into();
    }
    Ok(target)
}

pub(crate) fn matches(session: &Value, target: &Value) -> bool {
    let model = &data(session)["model"];
    model["providerID"] == target["providerID"]
        && model["id"] == target["id"]
        && (target.get("variant").is_none() || model["variant"] == target["variant"])
}

/// Make the session before starting the TUI; persist its ID on the reserved
/// binding before Herdr starts anything. Repeated starts reuse the same ID.
/// API failures leave the existing typed-switch fallback available.
pub(crate) fn start_args(
    store: &mut Store,
    herdr: &mut dyn HerdrAct,
    seat: &str,
    generation: i64,
    directory: &str,
    config: &ProjectConfig,
    model: &str,
) -> Result<Vec<String>, String> {
    let binding = store
        .read(|tx| tx.get_binding(seat, generation))
        .map_err(|e| e.detail)?
        .ok_or("OpenCode start has no binding")?;
    if let Some(id) = binding.session_ref {
        return Ok(vec!["--session".into(), id]);
    }
    let variant = config
        .models
        .switch
        .get(model)
        .and_then(|labels| labels.variant.as_deref());
    let target = match target(herdr, model, variant, Some(directory)) {
        Ok(target) => target,
        Err(error) if error.starts_with("invalid OpenCode") => return Err(error),
        Err(_) => return Ok(Vec::new()),
    };
    let body = json!({"location": {"directory": directory}, "agent": "build", "model": target});
    let response =
        match herdr.opencode_api(&["session.create".into(), "-d".into(), body.to_string()]) {
            Ok(value) => value,
            Err(_) => return Ok(Vec::new()),
        };
    let Some(id) = data(&response)["id"].as_str().filter(|id| !id.is_empty()) else {
        return Ok(Vec::new());
    };
    store
        .write(|tx| tx.set_binding_session_ref(seat, generation, id))
        .map_err(|e| e.detail)?;
    Ok(vec!["--session".into(), id.into()])
}

#[cfg(test)]
mod tests {
    use super::*;
    use gear_herdr::fake::{Call, FakeHerdr};

    fn world() -> Store {
        let mut store = Store::open_in_memory().unwrap();
        store
            .write(|tx| {
                gear_store::testing::seed_project(tx, "p", "sess", 1)?;
                gear_store::testing::seed_active_seat(
                    tx,
                    "p",
                    "lane",
                    gear_model::SeatRole::Lane,
                    "w1:p1",
                    "opencode",
                    1,
                )
            })
            .unwrap();
        store
    }

    fn config() -> ProjectConfig {
        gear_app::config::parse(
            r#"
[project]
id = "p"
repo = "/repo"
herdr_session = "sess"
[quota]
lanes = 1
temps = 0
[captain]
pane = "unused"
kind = "claude"
profile = "claude"
[models.switch."opencode-go/deepseek-v4.1-flash"]
picker = "DeepSeek"
bar = "DeepSeek"
variant = "max"
"#,
        )
        .unwrap()
    }

    fn catalog() -> Value {
        json!({"data":[{"providerID":"opencode-go", "id":"deepseek-v4.1-flash", "variants":[{"id":"max"}]}]})
    }

    #[test]
    fn session_creation_carries_the_validated_variant_and_retries_reuse_its_id() {
        let mut store = world();
        let mut herdr = FakeHerdr::new();
        herdr
            .api_replies
            .extend([Ok(catalog()), Ok(json!({"data":{"id":"ses_created"}}))]);
        for _ in 0..2 {
            assert_eq!(
                start_args(
                    &mut store,
                    &mut herdr,
                    "p:lane",
                    1,
                    "/repo/worktree",
                    &config(),
                    "opencode-go/deepseek-v4.1-flash"
                )
                .unwrap(),
                ["--session", "ses_created"]
            );
        }
        assert_eq!(
            herdr.calls.len(),
            2,
            "no duplicate session on a busy-start retry"
        );
        let Call::OpenCodeApi { args } = &herdr.calls[1] else {
            panic!("expected API")
        };
        let body: Value = serde_json::from_str(&args[2]).unwrap();
        assert_eq!(body["model"]["variant"], "max");
        assert_eq!(body["location"]["directory"], "/repo/worktree");
    }

    #[test]
    fn unknown_variant_blocks_start_instead_of_being_silently_dropped() {
        let mut store = world();
        let mut herdr = FakeHerdr::new();
        herdr.api_replies.push_back(Ok(catalog()));
        let mut config = config();
        config
            .models
            .switch
            .get_mut("opencode-go/deepseek-v4.1-flash")
            .unwrap()
            .variant = Some("bogus".into());
        assert!(start_args(
            &mut store,
            &mut herdr,
            "p:lane",
            1,
            "/repo/worktree",
            &config,
            "opencode-go/deepseek-v4.1-flash"
        )
        .unwrap_err()
        .contains("variant bogus"));
        assert_eq!(herdr.calls.len(), 1);
        assert!(store
            .read(|tx| tx.get_binding("p:lane", 1))
            .unwrap()
            .unwrap()
            .session_ref
            .is_none());
    }

    #[test]
    fn failed_session_create_leaves_the_typed_switch_fallback_available() {
        let mut store = world();
        let mut herdr = FakeHerdr::new();
        herdr
            .api_replies
            .extend([Ok(catalog()), Err("service unavailable".into())]);
        assert!(start_args(
            &mut store,
            &mut herdr,
            "p:lane",
            1,
            "/repo/worktree",
            &config(),
            "opencode-go/deepseek-v4.1-flash"
        )
        .unwrap()
        .is_empty());
        assert!(store
            .read(|tx| tx.get_binding("p:lane", 1))
            .unwrap()
            .unwrap()
            .session_ref
            .is_none());
    }
}
