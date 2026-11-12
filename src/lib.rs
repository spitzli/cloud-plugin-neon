//! Stateless Neon database actions. Core owns keys, authorization, bindings and receipts.
use serde::Deserialize;
use serde_json::{Value, json};
use spitzli_plugin_protocol::{ErrorCode, InvocationClaims, Manifest, PluginError};
use spitzli_plugin_runtime::{ActionHandler, HostClient};
use std::sync::Arc;

type Result<T> = std::result::Result<T, PluginError>;
pub const PLUGIN_ID: &str = "neon";

/// This seam preserves the published generic HostClient ABI and permits isolated fixtures.
#[tonic::async_trait]
pub trait Host: Send + Sync {
    async fn call(&self, token: &str, service: &str, method: &str, input: Value) -> Result<Value>;
}
#[tonic::async_trait]
impl Host for HostClient {
    async fn call(&self, token: &str, service: &str, method: &str, input: Value) -> Result<Value> {
        HostClient::call(self, token, service, method, input).await
    }
}
pub struct Neon {
    host: Arc<dyn Host>,
}
fn invalid() -> PluginError {
    PluginError::new(ErrorCode::InvalidInput, "Invalid Neon database request")
}
fn unavailable() -> PluginError {
    PluginError::new(
        ErrorCode::Unavailable,
        "Neon database information unavailable",
    )
}
fn text(value: &str, max: usize) -> Result<&str> {
    if value.is_empty()
        || value.chars().count() > max
        || value.chars().any(char::is_control)
        || value.contains("postgres://")
        || value.contains("postgresql://")
    {
        return Err(invalid());
    }
    Ok(value)
}
fn identifier(value: &str) -> Result<&str> {
    text(value, 128)?;
    if !value
        .bytes()
        .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
    {
        return Err(invalid());
    }
    Ok(value)
}
fn parse<T: for<'a> Deserialize<'a>>(input: &Value) -> Result<T> {
    serde_json::from_value(input.clone()).map_err(|_| invalid())
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Discover {
    neon_project_id: Option<String>,
    branch_id: Option<String>,
    cursor: Option<String>,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Attach {
    name: String,
    neon_project_id: String,
    branch_id: String,
    database_name: String,
    role_name: String,
    endpoint_id: String,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Resource {
    resource_id: String,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Credentials {
    resource_id: String,
    pooled: Option<bool>,
}
fn safe_scalar(value: &Value) -> Result<()> {
    match value {
        Value::String(s) => {
            text(s, 256).map_err(|_| unavailable())?;
        }
        Value::Null | Value::Bool(_) => {}
        _ => return Err(unavailable()),
    }
    Ok(())
}
/// Copy only documented safe scalar fields, even when a malformed host sends extra secrets.
fn rows(body: &Value, key: &str, fields: &[&str], required: &[&str]) -> Result<Value> {
    let input = body[key].as_array().ok_or_else(unavailable)?;
    if input.len() > 100 {
        return Err(unavailable());
    }
    let mut output = Vec::with_capacity(input.len());
    for row in input {
        let map = row.as_object().ok_or_else(unavailable)?;
        let mut safe = serde_json::Map::new();
        for field in fields {
            if let Some(value) = map.get(*field) {
                safe_scalar(value)?;
                safe.insert((*field).into(), value.clone());
            }
        }
        if required
            .iter()
            .any(|key| safe.get(*key).and_then(Value::as_str).is_none())
        {
            return Err(unavailable());
        }
        output.push(Value::Object(safe));
    }
    Ok(Value::Array(output))
}
fn branches(body: &Value) -> Result<Value> {
    rows(
        body,
        "branches",
        &["id", "name", "project_id", "parent_id", "current_state"],
        &["id", "name"],
    )
}
fn cursor(body: &Value) -> Result<Value> {
    match &body["pagination"]["cursor"] {
        Value::Null => Ok(Value::Null),
        Value::String(s) => {
            text(s, 512).map_err(|_| unavailable())?;
            Ok(json!(s))
        }
        _ => Err(unavailable()),
    }
}
impl Neon {
    pub fn new(host: impl Host + 'static) -> Self {
        Self {
            host: Arc::new(host),
        }
    }
    async fn request(
        &self,
        token: &str,
        method: &str,
        path: &str,
        query: Option<Value>,
        body: Option<Value>,
    ) -> Result<Value> {
        let mut request = json!({"binding":"neon","method":method,"path":path});
        if let Some(query) = query {
            request["query"] = query;
        }
        if let Some(body) = body {
            request["body"] = body;
        }
        // Never surface transport diagnostics or provider error bodies: either can contain a URI.
        let response = self
            .host
            .call(token, "http", "request", request)
            .await
            .map_err(|_| unavailable())?;
        let status = response["status"].as_u64().ok_or_else(unavailable)?;
        if !(200..300).contains(&status) {
            return Err(unavailable());
        }
        response
            .get("body")
            .filter(|body| body.is_object())
            .cloned()
            .ok_or_else(unavailable)
    }
    async fn discover(&self, token: &str, input: &Value) -> Result<Value> {
        let selected: Discover = parse(input)?;
        if selected.branch_id.is_some() && selected.neon_project_id.is_none() {
            return Err(invalid());
        }
        if let Some(project) = &selected.neon_project_id {
            identifier(project)?;
        }
        if let Some(branch) = &selected.branch_id {
            identifier(branch)?;
        }
        let mut query = json!({"limit":"100"});
        if let Some(value) = &selected.cursor {
            text(value, 512)?;
            query["cursor"] = json!(value);
        }
        let project = selected.neon_project_id.as_deref();
        let branch = selected.branch_id.as_deref();
        let projects = self.request(
            token,
            "GET",
            "/projects",
            Some(if project.is_none() {
                query.clone()
            } else {
                json!({"limit":"100"})
            }),
            None,
        );
        let branch_inventory = async {
            match project {
                Some(project) => {
                    self.request(
                        token,
                        "GET",
                        &format!("/projects/{project}/branches"),
                        Some(query.clone()),
                        None,
                    )
                    .await
                }
                None => Ok(json!({"branches":[]})),
            }
        };
        let details = async {
            let (Some(project), Some(branch)) = (project, branch) else {
                return Ok::<_, PluginError>(None);
            };
            let databases_path = format!("/projects/{project}/branches/{branch}/databases");
            let roles_path = format!("/projects/{project}/branches/{branch}/roles");
            let endpoints_path = format!("/projects/{project}/endpoints");
            Ok(Some(tokio::try_join!(
                self.request(
                    token,
                    "GET",
                    &databases_path,
                    Some(json!({"limit":"100"})),
                    None
                ),
                self.request(
                    token,
                    "GET",
                    &roles_path,
                    Some(json!({"limit":"100"})),
                    None
                ),
                self.request(
                    token,
                    "GET",
                    &endpoints_path,
                    Some(json!({"limit":"100"})),
                    None
                ),
            )?))
        };
        // At most five fixed GETs overlap; each retains Core's own current-authority checks.
        let (project_body, branch_body, details) =
            tokio::try_join!(projects, branch_inventory, details)?;
        let mut result = json!({
            "projects":rows(&project_body,"projects", &["id","name","region_id"], &["id","name"] )?,
            "branches":[],"databases":[],"roles":[],"endpoints":[],"cursor":cursor(&project_body)?
        });
        if project.is_none() {
            return Ok(result);
        }
        result["branches"] = branches(&branch_body)?;
        result["cursor"] = cursor(&branch_body)?;
        let Some((databases, roles, endpoints)) = details else {
            return Ok(result);
        };
        for (key, body, fields, required) in [
            (
                "databases",
                databases,
                &["name", "branch_id", "owner_name"][..],
                &["name"][..],
            ),
            (
                "roles",
                roles,
                &["name", "branch_id", "protected"][..],
                &["name"][..],
            ),
            (
                "endpoints",
                endpoints,
                &["id", "branch_id", "type", "current_state", "host"][..],
                &["id", "branch_id"][..],
            ),
        ] {
            result[key] = rows(&body, key, fields, required)?;
        }
        let branch = branch.ok_or_else(invalid)?;
        result["endpoints"]
            .as_array_mut()
            .ok_or_else(unavailable)?
            .retain(|row| row["branch_id"] == branch);
        Ok(result)
    }
    async fn view(&self, token: &str) -> Result<Value> {
        let current = self
            .request(token, "GET", "/_spitzli/databases/current", None, None)
            .await?;
        let name = current["resource"]["name"]
            .as_str()
            .ok_or_else(unavailable)?;
        text(name, 80).map_err(|_| unavailable())?;
        let status = current["status"].as_str().ok_or_else(unavailable)?;
        text(status, 64).map_err(|_| unavailable())?;
        let (state, status_text) = match status {
            "active" => ("healthy", "Neon compute is active"),
            "idle" => (
                "unknown",
                "Neon compute is idle; a database connection can wake it",
            ),
            "init" => ("pending", "Neon compute is starting"),
            _ => ("unknown", "Neon compute state is not recognized"),
        };
        let mut details = vec![json!({"field":"Compute state","value":status})];
        for (key, label) in [
            ("neon_project_id", "Neon project"),
            ("branch_id", "Branch"),
            ("database_name", "Database"),
            ("role_name", "Role"),
            ("endpoint_id", "Endpoint"),
        ] {
            let value = current["binding"][key].as_str().ok_or_else(unavailable)?;
            text(value, 128).map_err(|_| unavailable())?;
            details.push(json!({"field":label,"value":value}));
        }
        details.push(json!({"field":"SSL mode","value":"require"}));
        Ok(json!({"version":2,"title":name,"blocks":[
            {"kind":"status","label":"Compute","state":state,"text":status_text},
            {"kind":"table","title":"Connection settings","columns":[{"key":"field","label":"Setting"},{"key":"value","label":"Value"}],"rows":details},
            {"kind":"reveal","label":"Connection URI","action":"databases.credentials"},
            {"kind":"text","text":"Manage the Neon API key in workspace Connections. Detaching keeps the database and all provider data."},
            {"kind":"actions","actions":[{"id":"databases.detach","label":"Detach database","destructive":true}]}
        ]}))
    }
}
#[tonic::async_trait]
impl ActionHandler for Neon {
    async fn invoke(
        &self,
        claims: &InvocationClaims,
        token: &str,
        input: Value,
        configuration: Value,
    ) -> Result<Value> {
        if configuration != json!({}) || claims.plugin_id != PLUGIN_ID || claims.job.is_some() {
            return Err(invalid());
        }
        match claims.action.as_str() {
            "databases.discover" => self.discover(token, &input).await,
            "databases.attach" => {
                let tuple: Attach = parse(&input)?;
                text(&tuple.name, 80)?;
                identifier(&tuple.neon_project_id)?;
                identifier(&tuple.branch_id)?;
                text(&tuple.database_name, 128)?;
                text(&tuple.role_name, 128)?;
                identifier(&tuple.endpoint_id)?;
                let body = self
                    .request(
                        token,
                        "POST",
                        "/_spitzli/databases/attach",
                        None,
                        Some(input),
                    )
                    .await?;
                let resource = &body["resource"];
                let id = resource["id"].as_str().ok_or_else(unavailable)?;
                text(id, 128).map_err(|_| unavailable())?;
                let name = resource["name"].as_str().ok_or_else(unavailable)?;
                text(name, 80).map_err(|_| unavailable())?;
                Ok(json!({"resource":{"id":id,"name":name}}))
            }
            "databases.credentials" => {
                let resource: Credentials = parse(&input)?;
                text(&resource.resource_id, 128)?;
                let _pooled = resource.pooled.unwrap_or(true);
                let body = self
                    .request(
                        token,
                        "POST",
                        "/_spitzli/databases/credentials",
                        None,
                        Some(input),
                    )
                    .await?;
                let uri = body["value"].as_str().ok_or_else(unavailable)?;
                if uri.len() > 8000
                    || uri.chars().any(char::is_whitespace)
                    || !uri.contains('@')
                    || !(uri.starts_with("postgresql://") || uri.starts_with("postgres://"))
                {
                    return Err(unavailable());
                }
                Ok(json!({"value":uri}))
            }
            "databases.view" | "databases.branches" | "databases.detach" => {
                let resource: Resource = parse(&input)?;
                text(&resource.resource_id, 128)?;
                match claims.action.as_str() {
                    "databases.view" => self.view(token).await,
                    "databases.branches" => {
                        let body = self
                            .request(token, "GET", "/_spitzli/databases/branches", None, None)
                            .await?;
                        let rows = branches(&body)?;
                        Ok(json!({"version":2,"title":"Neon branches","blocks":[
                            {"kind":"text","text":"Read-only inventory of the attached Neon project. Branch changes remain in Neon."},
                            {"kind":"table","title":"Branches","columns":[{"key":"name","label":"Branch"},{"key":"id","label":"ID"},{"key":"current_state","label":"State"}],"rows":rows}
                        ]}))
                    }
                    _ => {
                        let body = self
                            .request(
                                token,
                                "POST",
                                "/_spitzli/databases/detach",
                                None,
                                Some(input),
                            )
                            .await?;
                        if body["detached"] != true || body["resource_id"] != resource.resource_id {
                            return Err(unavailable());
                        }
                        Ok(json!({"detached":true,"resource_id":resource.resource_id}))
                    }
                }
            }
            _ => Err(PluginError::new(
                ErrorCode::NotSupported,
                "Neon action unavailable",
            )),
        }
    }
}
/// Every permission, input schema and service declaration is pinned to this reviewed release.
pub fn validate_manifest(manifest: &Manifest) -> Result<()> {
    let mut reviewed: Value =
        serde_json::from_str(include_str!("../manifest.json")).map_err(|_| invalid())?;
    reviewed["image"] = manifest.as_json()["image"].clone();
    if manifest.as_json() != &reviewed {
        return Err(invalid());
    }
    Ok(())
}
