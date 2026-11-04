use serde_json::{Value, json};
use std::{
    collections::VecDeque,
    sync::{Arc, Mutex},
};
use webdock_neon::{Host, Neon, validate_manifest};
use webdock_plugin_protocol::{InvocationClaims, Manifest, PluginError};
use webdock_plugin_runtime::ActionHandler;

#[derive(Clone, Default)]
struct Fake {
    replies: Arc<Mutex<VecDeque<Value>>>,
    calls: Arc<Mutex<Vec<Value>>>,
}
impl Fake {
    fn with(bodies: Vec<Value>) -> Self {
        Self {
            replies: Arc::new(Mutex::new(
                bodies
                    .into_iter()
                    .map(|body| json!({"status":200,"body":body}))
                    .collect(),
            )),
            ..Self::default()
        }
    }
}
#[tonic::async_trait]
impl Host for Fake {
    async fn call(
        &self,
        token: &str,
        service: &str,
        method: &str,
        input: Value,
    ) -> Result<Value, PluginError> {
        assert_eq!(
            (token, service, method),
            ("fixture-token", "http", "request"),
            "Published generic caller ABI must not change"
        );
        self.calls.lock().unwrap().push(input);
        Ok(self
            .replies
            .lock()
            .unwrap()
            .pop_front()
            .expect("Unexpected callback"))
    }
}
fn claims(action: &str) -> InvocationClaims {
    serde_json::from_value(json!({"v":1,"id":"00000000-0000-4000-8000-000000000001","pluginID":"neon","generation":1,"action":action,"projectID":"201","customerID":"101","iat":0,"exp":60})).unwrap()
}
async fn invoke(fake: &Fake, action: &str, input: Value) -> Result<Value, PluginError> {
    Neon::new(fake.clone())
        .invoke(&claims(action), "fixture-token", input, json!({}))
        .await
}
fn tuple() -> Value {
    json!({"name":"Existing database","neon_project_id":"cold-river-123","branch_id":"br-main-123","database_name":"app-db","role_name":"app-owner","endpoint_id":"ep-ready-123"})
}
#[tokio::test]
async fn original_write_input_and_core_resource_are_preserved() {
    let fake = Fake::with(vec![
        json!({"resource":{"id":"core-binding-1","name":"Existing database"},"password":"must-not-escape"}),
        json!({"detached":true,"resource_id":"core-binding-1"}),
    ]);
    assert_eq!(
        invoke(&fake, "databases.attach", tuple()).await.unwrap(),
        json!({"resource":{"id":"core-binding-1","name":"Existing database"}})
    );
    let input = json!({"resource_id":"core-binding-1"});
    assert_eq!(
        invoke(&fake, "databases.detach", input.clone())
            .await
            .unwrap(),
        json!({"detached":true,"resource_id":"core-binding-1"})
    );
    let calls = fake.calls.lock().unwrap();
    assert_eq!(
        calls[0],
        json!({"binding":"neon","method":"POST","path":"/_spitzli/databases/attach","body":tuple()})
    );
    assert_eq!(
        calls[1],
        json!({"binding":"neon","method":"POST","path":"/_spitzli/databases/detach","body":input})
    );
    assert!(!calls.iter().any(|request| request["method"] == "DELETE"));
}
#[tokio::test]
async fn reveal_is_write_callback_and_only_returns_ephemeral_value() {
    let uri = "postgresql://fixture:synthetic@localhost/app?sslmode=require";
    let fake = Fake::with(vec![
        json!({"value":uri,"password":"discard"}),
        json!({"value":uri}),
    ]);
    let input = json!({"resource_id":"resource-1","pooled":false});
    assert_eq!(
        invoke(&fake, "databases.credentials", input.clone())
            .await
            .unwrap(),
        json!({"value":uri})
    );
    let default_input = json!({"resource_id":"resource-1"});
    invoke(&fake, "databases.credentials", default_input.clone())
        .await
        .unwrap();
    let calls = fake.calls.lock().unwrap();
    assert_eq!(
        calls[0],
        json!({"binding":"neon","method":"POST","path":"/_spitzli/databases/credentials","body":input})
    );
    assert_eq!(
        calls[1]["body"], default_input,
        "Default handling belongs to Core; original admitted input must not change"
    );
    assert!(!serde_json::to_string(&*calls).unwrap().contains(uri));
}
#[tokio::test]
async fn inventory_projects_only_safe_fields_and_filters_selected_endpoint() {
    let fake = Fake::with(vec![
        json!({"projects":[{"id":"project-1","name":"EU fixture","region_id":"aws-eu-central-1","api_key":"discard"}],"pagination":{"cursor":null}}),
        json!({"branches":[{"id":"br-main","name":"main","project_id":"project-1"}],"pagination":{"cursor":"next-safe"}}),
        json!({"databases":[{"name":1,"branch_id":"br-main"}]}),
        json!({"roles":[{"name":"app_owner","branch_id":"br-main","protected":false,"password":"discard"}]}),
        json!({"endpoints":[{"id":"ep-main","branch_id":"br-main","current_state":"idle","host":"fixture.invalid"},{"id":"ep-other","branch_id":"br-other"}]}),
    ]);
    // A non-string scalar in a field admitted as text is rejected instead of guessing its type.
    assert!(
        invoke(
            &fake,
            "databases.discover",
            json!({"neon_project_id":"project-1","branch_id":"br-main"})
        )
        .await
        .is_err()
    );
    assert_eq!(fake.calls.lock().unwrap().len(), 3);
    let fake = Fake::with(vec![
        json!({"projects":[{"id":"project-1","name":"EU fixture","region_id":"aws-eu-central-1","api_key":"discard"}]}),
        json!({"branches":[{"id":"br-main","name":"main"}],"pagination":{"cursor":"next-safe"}}),
        json!({"databases":[{"name":"app","branch_id":"br-main"}]}),
        json!({"roles":[{"name":"app_owner","password":"discard"}]}),
        json!({"endpoints":[{"id":"ep-main","branch_id":"br-main","host":"fixture.invalid"},{"id":"ep-other","branch_id":"br-other"}]}),
    ]);
    let result = invoke(
        &fake,
        "databases.discover",
        json!({"neon_project_id":"project-1","branch_id":"br-main"}),
    )
    .await
    .unwrap();
    assert_eq!(result["endpoints"].as_array().unwrap().len(), 1);
    assert_eq!(result["cursor"], "next-safe");
    assert!(!result.to_string().contains("discard"));
    assert!(!result.to_string().contains("password"));
    assert!(
        fake.calls
            .lock()
            .unwrap()
            .iter()
            .all(|request| request["method"] == "GET" && request["binding"] == "neon")
    );
}
#[tokio::test]
async fn invalid_authority_fields_and_path_traversal_never_reach_host() {
    let fake = Fake::default();
    for (action, input) in [
        ("databases.discover", json!({"neon_project_id":"../other"})),
        ("databases.discover", json!({"branch_id":"br-main"})),
        (
            "databases.discover",
            json!({"neon_project_id":"p","origin":"https://elsewhere.invalid"}),
        ),
        (
            "databases.credentials",
            json!({"resource_id":"r","neon_project_id":"other"}),
        ),
        (
            "databases.view",
            json!({"resource_id":"r","customer_id":"other"}),
        ),
        (
            "databases.detach",
            json!({"resource_id":"r","delete_provider":true}),
        ),
        ("databases.create", json!({})),
    ] {
        assert!(invoke(&fake, action, input).await.is_err(), "{action}");
    }
    assert!(fake.calls.lock().unwrap().is_empty());
    let mut wrong = claims("databases.discover");
    wrong.plugin_id = "other".into();
    assert!(
        Neon::new(fake.clone())
            .invoke(&wrong, "fixture-token", json!({}), json!({}))
            .await
            .is_err()
    );
    let mut job = claims("databases.discover");
    job.job = Some(webdock_plugin_protocol::JobIdentity {
        id: format!("job-{}", "a".repeat(64)),
        attempt: 1,
    });
    assert!(
        Neon::new(fake.clone())
            .invoke(&job, "fixture-token", json!({}), json!({}))
            .await
            .is_err()
    );
    assert!(
        Neon::new(fake.clone())
            .invoke(
                &claims("databases.discover"),
                "fixture-token",
                json!({}),
                json!({"api_key":"synthetic-forbidden"})
            )
            .await
            .is_err()
    );
    assert!(fake.calls.lock().unwrap().is_empty());
}
#[tokio::test]
async fn safe_panels_never_contain_uri_and_resource_context_is_not_forwarded() {
    let fake = Fake::with(vec![
        json!({"resource":{"id":"core-1","name":"Fixture"},"binding":{"neon_project_id":"project-1","branch_id":"br-main","database_name":"app","role_name":"owner","endpoint_id":"ep-main","uri":"discard"},"status":"active","value":"discard"}),
        json!({"branches":[{"id":"br-main","name":"main","password":"discard"}]}),
    ]);
    for action in ["databases.view", "databases.branches"] {
        let result = invoke(&fake, action, json!({"resource_id":"core-1"}))
            .await
            .unwrap();
        assert_eq!(result["version"], 2);
        assert!(!result.to_string().contains("discard"));
        assert!(!result.to_string().contains("postgresql://"));
    }
    assert!(
        fake.calls
            .lock()
            .unwrap()
            .iter()
            .all(|request| request.get("body").is_none() && request.get("query").is_none())
    );
}
#[tokio::test]
async fn malformed_uris_and_provider_errors_never_become_diagnostics() {
    for body in [
        json!({"value":"https://user:secret@localhost"}),
        json!({"value":"postgresql://user:secret@localhost/db\n"}),
        json!({"value":false}),
    ] {
        let fake = Fake::with(vec![body]);
        let error = invoke(&fake, "databases.credentials", json!({"resource_id":"r"}))
            .await
            .unwrap_err();
        assert!(!format!("{error:?}").contains("secret"));
    }
    let fake = Fake::default();
    fake.replies
        .lock()
        .unwrap()
        .push_back(json!({"status":403,"body":{"message":"postgresql://secret@localhost"}}));
    let error = invoke(&fake, "databases.discover", json!({}))
        .await
        .unwrap_err();
    assert!(!format!("{error:?}").contains("secret"));
}
#[test]
fn manifest_pins_complete_action_and_privilege_boundary() {
    let value: Value = serde_json::from_str(include_str!("../manifest.json")).unwrap();
    let parse = |v: &Value| Manifest::parse(&serde_json::to_vec(v).unwrap());
    validate_manifest(&parse(&value).unwrap()).unwrap();
    assert_eq!(value["actions"].as_array().unwrap().len(), 6);
    assert_eq!(value["tables"], json!([]));
    assert_eq!(value["secrets"], json!([]));
    assert_eq!(value["httpBindings"], json!(["neon"]));
    for action in value["actions"].as_array().unwrap() {
        assert!(action.get("httpBinding").is_none());
        assert_eq!(action["input"]["additionalProperties"], false);
        if action["name"] == "databases.credentials" {
            assert_eq!(action["intent"], "write");
            assert_eq!(action["capabilities"], json!(["plugin.http.request"]));
        }
    }
    let mut changed = value.clone();
    changed["actions"][0]["httpBinding"] = json!("neon");
    assert!(validate_manifest(&parse(&changed).unwrap()).is_err());
    let mut changed = value.clone();
    changed["actions"][4]["intent"] = json!("read");
    assert!(
        parse(&changed)
            .map(|m| validate_manifest(&m).is_err())
            .unwrap_or(true)
    );
    let mut changed = value.clone();
    changed["actions"][0]["input"]["additionalProperties"] = json!(true);
    assert!(
        parse(&changed)
            .map(|m| validate_manifest(&m).is_err())
            .unwrap_or(true)
    );
    let mut release = value.clone();
    release["image"] = json!(format!(
        "ghcr.io/spitzli/cloud-plugin-neon@sha256:{}",
        "a".repeat(64)
    ));
    validate_manifest(&parse(&release).unwrap()).unwrap();
}

#[tokio::test]
async fn actual_endpoint_states_are_rendered_without_claiming_idle_is_healthy() {
    for state in ["active", "idle", "init"] {
        let fake = Fake::with(vec![
            json!({"resource":{"id":"core-1","name":"Fixture"},"binding":{"neon_project_id":"project-1","branch_id":"br-main","database_name":"app","role_name":"owner","endpoint_id":"ep-main"},"status":state}),
        ]);
        let result = invoke(&fake, "databases.view", json!({"resource_id":"core-1"}))
            .await
            .unwrap();
        assert_eq!(result["version"], 2);
        assert!(
            result["blocks"][1]["rows"]
                .as_array()
                .unwrap()
                .iter()
                .any(|row| row["field"] == "Compute state" && row["value"] == state)
        );
        if state != "active" {
            assert_ne!(result["blocks"][0]["state"], "healthy");
        }
    }
}

#[tokio::test]
async fn original_post_field_order_survives_standalone_json_parsing() {
    let raw = r#"{"role_name":"app-owner","name":"Existing database","endpoint_id":"ep-ready-123","database_name":"app-db","neon_project_id":"cold-river-123","branch_id":"br-main-123"}"#;
    let input: Value = serde_json::from_str(raw).unwrap();
    let fake = Fake::with(vec![
        json!({"resource":{"id":"core-binding-1","name":"Existing database"}}),
    ]);
    invoke(&fake, "databases.attach", input).await.unwrap();
    let calls = fake.calls.lock().unwrap();
    assert_eq!(serde_json::to_string(&calls[0]["body"]).unwrap(), raw);
}

#[tokio::test]
async fn selected_inventory_overlaps_independent_host_round_trips() {
    #[derive(Clone)]
    struct Fanout(Arc<tokio::sync::Barrier>);
    #[tonic::async_trait]
    impl Host for Fanout {
        async fn call(
            &self,
            token: &str,
            service: &str,
            method: &str,
            input: Value,
        ) -> Result<Value, PluginError> {
            assert_eq!(
                (token, service, method),
                ("fixture-token", "http", "request")
            );
            assert_eq!(input["binding"], "neon");
            assert_eq!(input["method"], "GET");
            assert!(input.get("body").is_none());
            let body = match input["path"].as_str().unwrap() {
                "/projects" => {
                    json!({"projects":[{"id":"project-1","name":"EU fixture","region_id":"aws-eu-central-1"}]})
                }
                "/projects/project-1/branches" => {
                    json!({"branches":[{"id":"br-main","name":"main"}]})
                }
                "/projects/project-1/branches/br-main/databases" => {
                    json!({"databases":[{"name":"app"}]})
                }
                "/projects/project-1/branches/br-main/roles" => {
                    json!({"roles":[{"name":"app_owner","password":"must-not-escape"}]})
                }
                "/projects/project-1/endpoints" => {
                    json!({"endpoints":[{"id":"ep-main","branch_id":"br-main"},{"id":"ep-foreign","branch_id":"br-foreign"}]})
                }
                path => panic!("Unexpected inventory path: {path}"),
            };
            // Completion requires exactly the five bounded reads to overlap; no network timing.
            self.0.wait().await;
            Ok(json!({"status":200,"body":body}))
        }
    }
    let plugin = Neon::new(Fanout(Arc::new(tokio::sync::Barrier::new(5))));
    let result = tokio::time::timeout(
        std::time::Duration::from_secs(3),
        plugin.invoke(
            &claims("databases.discover"),
            "fixture-token",
            json!({"neon_project_id":"project-1","branch_id":"br-main"}),
            json!({}),
        ),
    )
    .await
    .expect("Independent reads must overlap within one bounded action")
    .unwrap();
    assert_eq!(result["endpoints"].as_array().unwrap().len(), 1);
    assert!(!result.to_string().contains("must-not-escape"));
}
