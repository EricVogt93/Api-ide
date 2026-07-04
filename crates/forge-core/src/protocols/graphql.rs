//! GraphQL schema introspection and query helpers.
//!
//! This module does not implement a GraphQL client for executing queries
//! (that goes through the regular HTTP engine, since a GraphQL request is
//! just a POST with a JSON body) — it covers the GraphQL-specific pieces:
//! running the introspection query and rendering the resulting schema,
//! validating query documents, and building the `{query, variables,
//! operationName}` request body.

use serde::{Deserialize, Serialize};

use super::ProtocolError;

/// The standard GraphQL introspection query, as used by most GraphQL
/// tooling (GraphiQL, Apollo, etc). Nests `ofType` seven levels deep, which
/// is enough to describe any real-world type (e.g. `[[Pet!]!]!`).
pub const INTROSPECTION_QUERY: &str = r#"
query IntrospectionQuery {
  __schema {
    queryType { name }
    mutationType { name }
    subscriptionType { name }
    types {
      ...FullType
    }
    directives {
      name
      description
      locations
      args {
        ...InputValue
      }
    }
  }
}

fragment FullType on __Type {
  kind
  name
  description
  fields(includeDeprecated: true) {
    name
    description
    args {
      ...InputValue
    }
    type {
      ...TypeRef
    }
    isDeprecated
    deprecationReason
  }
  inputFields {
    ...InputValue
  }
  interfaces {
    ...TypeRef
  }
  enumValues(includeDeprecated: true) {
    name
    description
    isDeprecated
    deprecationReason
  }
  possibleTypes {
    ...TypeRef
  }
}

fragment InputValue on __InputValue {
  name
  description
  type { ...TypeRef }
  defaultValue
}

fragment TypeRef on __Type {
  kind
  name
  ofType {
    kind
    name
    ofType {
      kind
      name
      ofType {
        kind
        name
        ofType {
          kind
          name
          ofType {
            kind
            name
            ofType {
              kind
              name
              ofType {
                kind
                name
              }
            }
          }
        }
      }
    }
  }
}
"#;

/// A GraphQL schema as recovered from an introspection query response.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct GraphQlSchema {
    pub query_type: Option<String>,
    pub mutation_type: Option<String>,
    pub subscription_type: Option<String>,
    pub types: Vec<GqlType>,
}

/// One named type from the schema (object, interface, enum, scalar, ...).
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct GqlType {
    pub name: String,
    /// Introspection `__TypeKind`, e.g. `"OBJECT"`, `"SCALAR"`, `"ENUM"`.
    pub kind: String,
    pub description: Option<String>,
    pub fields: Vec<GqlField>,
}

/// A field on an object or interface type.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct GqlField {
    pub name: String,
    /// The field's type rendered as GraphQL SDL, e.g. `[Pet!]!`.
    pub type_display: String,
    /// Argument name -> rendered type.
    pub args: Vec<(String, String)>,
    pub description: Option<String>,
}

/// Runs [`INTROSPECTION_QUERY`] against `url` and parses the result into a
/// [`GraphQlSchema`].
pub async fn introspect(
    url: &str,
    headers: &[(String, String)],
) -> Result<GraphQlSchema, ProtocolError> {
    let client = reqwest::Client::new();
    let mut request = client
        .post(url)
        .json(&serde_json::json!({ "query": INTROSPECTION_QUERY }));
    for (name, value) in headers {
        request = request.header(name, value);
    }

    let response = request
        .send()
        .await
        .map_err(|e| ProtocolError::Connect(e.to_string()))?;

    let status = response.status();
    if !status.is_success() {
        let body = response.text().await.unwrap_or_default();
        return Err(ProtocolError::Http(format!("HTTP {status}: {body}")));
    }

    let raw: RawIntrospectionResponse = response
        .json()
        .await
        .map_err(|e| ProtocolError::Parse(e.to_string()))?;

    if let Some(errors) = raw.errors {
        if !errors.is_empty() {
            let message = errors
                .into_iter()
                .map(|e| e.message)
                .collect::<Vec<_>>()
                .join("; ");
            return Err(ProtocolError::Parse(message));
        }
    }

    let data = raw
        .data
        .ok_or_else(|| ProtocolError::Parse("introspection response had no data".to_string()))?;

    Ok(convert_schema(data.schema))
}

/// Validates that `query` is syntactically valid GraphQL, returning an
/// error message (including the failing position) if not.
pub fn validate_query(query: &str) -> Result<(), String> {
    graphql_parser::parse_query::<&str>(query)
        .map(|_| ())
        .map_err(|e| e.to_string())
}

/// Builds the `{query, variables, operationName}` JSON body sent to a
/// GraphQL endpoint. An empty/blank `variables_json` becomes `{}`; invalid
/// JSON in `variables_json` is an error.
pub fn build_request_body(
    query: &str,
    variables_json: &str,
    operation_name: Option<&str>,
) -> Result<serde_json::Value, String> {
    let variables: serde_json::Value = if variables_json.trim().is_empty() {
        serde_json::Value::Object(serde_json::Map::new())
    } else {
        serde_json::from_str(variables_json)
            .map_err(|e| format!("invalid variables JSON: {e}"))?
    };

    let mut body = serde_json::json!({
        "query": query,
        "variables": variables,
    });
    if let Some(name) = operation_name.filter(|n| !n.is_empty()) {
        body["operationName"] = serde_json::Value::String(name.to_string());
    }
    Ok(body)
}

// --- Raw introspection JSON shapes -----------------------------------

#[derive(Debug, Deserialize)]
struct RawIntrospectionResponse {
    data: Option<RawIntrospectionData>,
    errors: Option<Vec<RawGraphQlError>>,
}

#[derive(Debug, Deserialize)]
struct RawGraphQlError {
    message: String,
}

#[derive(Debug, Deserialize)]
struct RawIntrospectionData {
    #[serde(rename = "__schema")]
    schema: RawSchema,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct RawSchema {
    query_type: Option<RawNamedRef>,
    mutation_type: Option<RawNamedRef>,
    subscription_type: Option<RawNamedRef>,
    #[serde(default)]
    types: Vec<RawType>,
}

#[derive(Debug, Deserialize)]
struct RawNamedRef {
    name: Option<String>,
}

#[derive(Debug, Deserialize)]
struct RawType {
    kind: String,
    name: Option<String>,
    description: Option<String>,
    #[serde(default)]
    fields: Option<Vec<RawField>>,
}

#[derive(Debug, Deserialize)]
struct RawField {
    name: String,
    description: Option<String>,
    #[serde(default)]
    args: Vec<RawInputValue>,
    #[serde(rename = "type")]
    type_ref: RawTypeRef,
}

#[derive(Debug, Deserialize)]
struct RawInputValue {
    name: String,
    #[serde(rename = "type")]
    type_ref: RawTypeRef,
}

#[derive(Debug, Deserialize)]
struct RawTypeRef {
    kind: String,
    name: Option<String>,
    #[serde(rename = "ofType", default)]
    of_type: Option<Box<RawTypeRef>>,
}

fn convert_schema(schema: RawSchema) -> GraphQlSchema {
    GraphQlSchema {
        query_type: schema.query_type.and_then(|t| t.name),
        mutation_type: schema.mutation_type.and_then(|t| t.name),
        subscription_type: schema.subscription_type.and_then(|t| t.name),
        types: schema
            .types
            .into_iter()
            .filter(|t| !t.name.as_deref().unwrap_or_default().starts_with("__"))
            .map(convert_type)
            .collect(),
    }
}

fn convert_type(raw: RawType) -> GqlType {
    GqlType {
        name: raw.name.unwrap_or_default(),
        kind: raw.kind,
        description: raw.description,
        fields: raw
            .fields
            .unwrap_or_default()
            .into_iter()
            .map(convert_field)
            .collect(),
    }
}

fn convert_field(raw: RawField) -> GqlField {
    GqlField {
        name: raw.name,
        type_display: render_type_ref(&raw.type_ref),
        args: raw
            .args
            .into_iter()
            .map(|a| (a.name, render_type_ref(&a.type_ref)))
            .collect(),
        description: raw.description,
    }
}

/// Renders a `__Type`/`ofType` reference chain as GraphQL SDL, e.g.
/// `NON_NULL(LIST(NON_NULL(Pet)))` -> `[Pet!]!`.
fn render_type_ref(type_ref: &RawTypeRef) -> String {
    match type_ref.kind.as_str() {
        "NON_NULL" => format!(
            "{}!",
            type_ref
                .of_type
                .as_deref()
                .map(render_type_ref)
                .unwrap_or_default()
        ),
        "LIST" => format!(
            "[{}]",
            type_ref
                .of_type
                .as_deref()
                .map(render_type_ref)
                .unwrap_or_default()
        ),
        _ => type_ref.name.clone().unwrap_or_default(),
    }
}
