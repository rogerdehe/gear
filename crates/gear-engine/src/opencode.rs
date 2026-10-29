//! Session-owned OpenCode models (T-44). No terminal keys on the API path.

use gear_app::config::ProjectConfig;
use gear_herdr::HerdrAct;
use gear_store::Store;
use serde_json::{json, Value};

pub(crate) fn data(value: &Value) -> &Value {
    value.get("data").unwrap_or(value)
}

/// What a `model.list` lookup found (T-67).
pub(crate) enum Lookup {
    /// The model (and its variant, when asked for) is in the catalogue.
    Target(Value),
    /// The directory's catalogue is empty: OpenCode has not loaded it yet.
    /// That is "not yet", never an invalid model.
    NotLoaded,
    /// The catalogue is there and does not hold the model or its variant: a
    /// configuration fault, not permission to pick another model.
    Invalid(String),
    /// The service could not answer (an older session, a transport error):
    /// the typed-key path's job. The reason is not needed by either caller,
    /// and neither may pick another model on it.
    Unavailable,
}

/// Invalid model/variant is a configuration fault, not permission to silently
/// select another model or let the TUI discard an unknown variant.
pub(crate) fn target(
    herdr: &mut dyn HerdrAct,
    model: &str,
    variant: Option<&str>,
    directory: Option<&str>,
) -> Lookup {
    let Some((provider, id)) = model.split_once('/') else {
        return Lookup::Invalid("invalid OpenCode model: needs provider/id".into());
    };
    let mut args = vec!["model.list".into()];
    if let Some(directory) = directory {
        args.extend(["--param".into(), format!("location[directory]={directory}")]);
    }
    let catalog = match herdr.opencode_api(&args) {
        Ok(catalog) => catalog,
        Err(_) => return Lookup::Unavailable,
    };
    let Some(models) = data(&catalog).as_array() else {
        return Lookup::Unavailable;
    };
    // An empty catalogue for a directory is "not loaded yet" (T-67): OpenCode
    // lists a directory's models once its service has read them.
    if models.is_empty() {
        return Lookup::NotLoaded;
    }
    let Some(info) = models
        .iter()
        .find(|m| m["providerID"] == provider && m["id"] == id)
    else {
        return Lookup::Invalid(format!("invalid OpenCode model {model}: absent from model.list"));
    };
    if let Some(variant) = variant {
        if !info["variants"]
            .as_array()
            .is_some_and(|variants| variants.iter().any(|v| v["id"] == variant))
        {
            return Lookup::Invalid(format!(
                "invalid OpenCode variant {variant} for {model}: absent from model.list"
            ));
        }
    }
    let mut target = json!({"providerID": provider, "id": id});
    if let Some(variant) = variant {
        target["variant"] = variant.into();
    }
    Lookup::Target(target)
}

/// The labels a model switch needs, derived from OpenCode's own catalogue
/// (T-94): the status bar and the picker both show `<model name> <provider
/// name>` as OpenCode renders them. `variant` is the chain's, when it names
/// one.
///
/// `Ok(None)` means the list was read, is not empty, and has no such model
/// (or the id has no `/`): the caller keeps its own "no labels" refusal.
/// `Err` means the catalogue could not be read at all — the model is then not
/// known to be absent, and must never be reported as absent (T-307).
pub(crate) fn derived_labels(
    herdr: &mut dyn HerdrAct,
    model: &str,
    variant: Option<&str>,
    directory: Option<&str>,
) -> Result<Option<gear_app::config::ModelSwitch>, String> {
    let Some((provider, id)) = model.split_once('/') else {
        return Ok(None);
    };
    let mut args = vec!["model.list".into()];
    if let Some(directory) = directory {
        args.extend(["--param".into(), format!("location[directory]={directory}")]);
    }
    let catalog = herdr.opencode_api(&args)?;
    let Some(models) = data(&catalog).as_array() else {
        return Err("model.list did not return a list".into());
    };
    // An empty list is "not loaded yet", never "the model is absent".
    if models.is_empty() {
        return Err("the model list is not loaded yet".into());
    }
    let Some(info) = models
        .iter()
        .find(|m| m["providerID"] == provider && m["id"] == id)
    else {
        return Ok(None);
    };
    let model_name = info["name"].as_str().unwrap_or(id);
    // The display name as OpenCode renders it in the status bar: the provider's
    // own name from `provider.list` (F1 of the T-94 review), e.g. "MiniMax
    // (minimax.cn)" or "OpenCode Go", not its id. Without it there is no label
    // OpenCode would show, and no fallback to the id.
    let Some(provider_name) = provider_display_name(herdr, provider, directory) else {
        return Err(format!("provider.list has no name for {provider}"));
    };
    let text = format!("{model_name} {provider_name}");
    Ok(Some(gear_app::config::ModelSwitch {
        picker: text.clone(),
        bar: text,
        variant: variant.map(str::to_string),
    }))
}

/// The provider's display name from OpenCode's `provider.list` (F1 of the
/// T-94 review), for the directory when one is known. The model list carries
/// only the provider id.
fn provider_display_name(
    herdr: &mut dyn HerdrAct,
    provider: &str,
    directory: Option<&str>,
) -> Option<String> {
    let mut args = vec!["provider.list".into()];
    if let Some(directory) = directory {
        args.extend(["--param".into(), format!("location[directory]={directory}")]);
    }
    let catalog = herdr.opencode_api(&args).ok()?;
    let providers = data(&catalog).as_array()?;
    providers
        .iter()
        .find(|p| p["id"] == provider)
        .and_then(|p| p["name"].as_str().map(str::to_string))
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
    model_list_wait_s: u64,
    model_list_poll_ms: u64,
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
    // An empty catalogue for this directory means OpenCode has not loaded it
    // yet (trial G01): wait for it, bounded, and never pick another model or
    // provider meanwhile. A catalogue that is there without the model is a
    // config fault; the seat parks with that reason.
    let began = std::time::Instant::now();
    let limit = std::time::Duration::from_secs(model_list_wait_s);
    let target = loop {
        match target(herdr, model, variant, Some(directory)) {
            Lookup::Target(target) => break target,
            Lookup::Invalid(error) => return Err(error),
            Lookup::Unavailable => return Ok(Vec::new()),
            Lookup::NotLoaded if began.elapsed() < limit => {
                // The wait between two asks for the catalogue. A poll of 0
                // asks again at once: the caller drives the answers (T-127
                // F5 — no test sleeps on the wall clock). Production keeps
                // its positive poll interval.
                if model_list_poll_ms > 0 {
                    std::thread::sleep(std::time::Duration::from_millis(model_list_poll_ms));
                }
            }
            Lookup::NotLoaded => {
                return Err(format!(
                    "the OpenCode model list is not loaded for {directory} after {}s: the model {model} is neither valid nor invalid yet, start the lane again when OpenCode has listed it",
                    model_list_wait_s
                ));
            }
        }
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
                    "opencode-go/deepseek-v4.1-flash",
                    0,
                    0,
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
            "opencode-go/deepseek-v4.1-flash",
            0,
            0,
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
            "opencode-go/deepseek-v4.1-flash",
            0,
            0,
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
    /// T-94: a model without `[models.switch]` labels takes them from
    /// OpenCode's catalogue: `<model name> <provider name>` for both the bar
    /// and the picker, and no variant unless one is given.
    #[test]
    fn a_model_without_labels_takes_them_from_the_catalogue() {
        let mut herdr = FakeHerdr::new();
        herdr.api_replies.push_back(Ok(json!({"data": [{
            "providerID": "minimax-cn",
            "id": "MiniMax-M3.1-Flash",
            "name": "MiniMax-M3.1-Flash"
        }]})));
        // The display name is the provider's own, from provider.list (F1).
        herdr.api_replies.push_back(Ok(json!({"data": [
            {"id": "minimax-cn", "name": "MiniMax (minimax.cn)"},
            {"id": "opencode-go", "name": "OpenCode Go"}
        ]})));
        let labels = derived_labels(&mut herdr, "minimax-cn/MiniMax-M3.1-Flash", None, Some("/repo"))
            .expect("the catalogue is readable")
            .expect("the catalogue holds it");
        assert_eq!(labels.bar, "MiniMax-M3.1-Flash MiniMax (minimax.cn)");
        assert_eq!(labels.picker, "MiniMax-M3.1-Flash MiniMax (minimax.cn)");
        assert_eq!(labels.variant, None);

        // A model the catalogue does not hold has no labels to derive.
        herdr.api_replies.push_back(Ok(json!({"data": [{"providerID": "x", "id": "y"}]})));
        assert_eq!(
            derived_labels(&mut herdr, "minimax-cn/nope", None, Some("/repo")),
            Ok(None)
        );

        // The chain's variant rides along when it names one.
        herdr.api_replies.push_back(Ok(json!({"data": [{
            "providerID": "minimax-cn",
            "id": "MiniMax-M3.1-Flash",
            "name": "MiniMax-M3.1-Flash"
        }]})));
        herdr.api_replies.push_back(Ok(json!({"data": [{"id": "minimax-cn", "name": "MiniMax (minimax.cn)"}]})));
        let labels = derived_labels(&mut herdr, "minimax-cn/MiniMax-M3.1-Flash", Some("max"), None)
            .expect("the catalogue is readable")
            .expect("the catalogue holds it");
        assert_eq!(labels.variant.as_deref(), Some("max"));
    }

    /// T-307: a catalogue that cannot be read is an error, never "the model is
    /// absent": the captain must not be sent after a model id that is right.
    #[test]
    fn an_unreadable_catalogue_is_an_error_not_an_absent_model() {
        // The API call itself fails: its own text is the reason.
        let mut herdr = FakeHerdr::new();
        herdr.api_replies.push_back(Err("connection refused".into()));
        assert_eq!(
            derived_labels(&mut herdr, "minimax-cn/MiniMax-M3.1-Flash", None, Some("/repo")),
            Err("connection refused".into())
        );

        // The reply is not a list.
        let mut herdr = FakeHerdr::new();
        herdr.api_replies.push_back(Ok(json!({"data": {"models": []}})));
        assert_eq!(
            derived_labels(&mut herdr, "minimax-cn/MiniMax-M3.1-Flash", None, Some("/repo")),
            Err("model.list did not return a list".into())
        );

        // An empty list is "not loaded yet", not "absent".
        let mut herdr = FakeHerdr::new();
        herdr.api_replies.push_back(Ok(json!({"data": []})));
        assert_eq!(
            derived_labels(&mut herdr, "minimax-cn/MiniMax-M3.1-Flash", None, Some("/repo")),
            Err("the model list is not loaded yet".into())
        );

        // The model is there but provider.list gives no name: a reason, and no
        // fallback to the provider id as a label.
        let mut herdr = FakeHerdr::new();
        herdr.api_replies.push_back(Ok(json!({"data": [{
            "providerID": "minimax-cn",
            "id": "MiniMax-M3.1-Flash",
            "name": "MiniMax-M3.1-Flash"
        }]})));
        herdr.api_replies.push_back(Ok(json!({"data": [{"id": "opencode-go", "name": "OpenCode Go"}]})));
        assert_eq!(
            derived_labels(&mut herdr, "minimax-cn/MiniMax-M3.1-Flash", None, Some("/repo")),
            Err("provider.list has no name for minimax-cn".into())
        );
    }
}
