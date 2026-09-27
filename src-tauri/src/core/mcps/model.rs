use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Definition {
    pub id: String,
    pub name: String,
    pub transport: Transport,
    pub server: ServerConfig,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub auth: Option<AuthRefs>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source: Option<CatalogSource>,
    pub revision: String,
    pub updated_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct DefinitionInput {
    #[serde(default)]
    pub id: Option<String>,
    pub name: String,
    pub transport: Transport,
    pub server: ServerConfig,
    #[serde(default)]
    pub auth: Option<AuthRefs>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source: Option<CatalogSource>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CatalogSource {
    pub registry: String,
    pub server_id: String,
    pub version: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum Transport {
    Stdio,
    Http,
    Sse,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ServerConfig {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub command: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub args: Vec<String>,
    #[serde(default, skip_serializing_if = "std::collections::BTreeMap::is_empty")]
    pub env: std::collections::BTreeMap<String, String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
    #[serde(default, skip_serializing_if = "std::collections::BTreeMap::is_empty")]
    pub headers: std::collections::BTreeMap<String, String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub env_file: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cwd: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AuthRefs {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bearer_token_env_var: Option<String>,
    #[serde(default, skip_serializing_if = "std::collections::BTreeMap::is_empty")]
    pub credential_refs: std::collections::BTreeMap<String, String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(
    tag = "action",
    rename_all = "snake_case",
    rename_all_fields = "camelCase",
    deny_unknown_fields
)]
pub enum McpsRequest {
    List {},
    Get {
        id: String,
    },
    Save {
        definition: Box<DefinitionInput>,
        #[serde(default)]
        expected_revision: Option<String>,
    },
    Remove {
        id: String,
        #[serde(default)]
        detach: bool,
    },
    Capabilities {},
    Inspect {
        target: Target,
    },
    Import {
        target: Target,
        name: String,
        #[serde(default)]
        conflict: Option<String>,
        #[serde(default)]
        new_name: Option<String>,
    },
    Catalog {
        #[serde(default)]
        query: Option<String>,
        #[serde(default)]
        cursor: Option<String>,
        #[serde(default)]
        server_id: Option<String>,
        #[serde(default)]
        dry_run: bool,
    },
    Preview {
        target: Target,
        operations: Vec<Operation>,
        #[serde(default)]
        dry_run: bool,
    },
    Apply {
        preview_id: String,
    },
    Recover {
        recovery_id: String,
    },
    Deployments {
        #[serde(default)]
        target: Option<Target>,
    },
    Undeploy {
        target: Target,
        definition_id: String,
        #[serde(default)]
        server_name: Option<String>,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Target {
    pub agent_key: String,
    #[serde(default)]
    pub project_id: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Operation {
    pub kind: String,
    pub definition_id: String,
    #[serde(default)]
    pub server_name: Option<String>,
    #[serde(default)]
    pub conflict: Option<String>,
    #[serde(default)]
    pub new_name: Option<String>,
    #[serde(default)]
    pub override_config: Option<Value>,
}
