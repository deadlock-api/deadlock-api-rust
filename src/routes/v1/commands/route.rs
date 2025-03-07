use crate::error::{APIError, APIResult};
use crate::routes::v1::commands::variables::Variable;
use crate::routes::v1::leaderboard::types::LeaderboardRegion;
use crate::state::AppState;
use crate::utils::parse_steam_id;
use axum::Json;
use axum::extract::{Query, State};
use axum::response::IntoResponse;
use itertools::Itertools;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use strum::VariantArray;
use tracing::warn;
use utoipa::{IntoParams, ToSchema};

#[derive(Debug, Clone, Serialize, ToSchema)]
pub struct VariableDescription {
    pub name: String,
    pub description: String,
    pub extra_args: Vec<String>,
}

impl From<Variable> for VariableDescription {
    fn from(v: Variable) -> Self {
        Self {
            name: v.get_name().to_string(),
            description: v.get_description().to_string(),
            extra_args: v.extra_args(),
        }
    }
}

#[utoipa::path(
    get,
    path = "/variables/available",
    responses(
        (status = OK, body = [VariableDescription]),
        (status = BAD_REQUEST, description = "Provided parameters are invalid."),
    ),
    tags = ["Commands"],
    summary = "Available Variables",
    description = "Returns a list of available variables that can be used in the command endpoint."
)]
pub async fn available_variables() -> APIResult<impl IntoResponse> {
    let variable_descriptions = Variable::VARIANTS
        .iter()
        .copied()
        .map_into::<VariableDescription>()
        .collect_vec();
    Ok(Json(variable_descriptions))
}

#[utoipa::path(
    get,
    path = "/widgets/versions",
    responses(
        (status = OK, body = HashMap<String, i32>),
        (status = BAD_REQUEST, description = "Provided parameters are invalid."),
    ),
    tags = ["Commands"],
    summary = "Widget Versions",
    description = "Returns a dict of str->int of widget versions."
)]
pub async fn widget_versions() -> APIResult<impl IntoResponse> {
    let widget_versions_file = std::fs::File::open("widget_versions.json").map_err(|e| {
        warn!("Failed to open widget_versions.json: {e}");
        APIError::InternalError {
            message: format!("Failed to open widget_versions.json: {e}"),
        }
    })?;
    serde_json::from_reader(widget_versions_file)
        .map(|r: HashMap<String, i32>| Json(r))
        .map_err(|e| {
            warn!("Failed to parse widget_versions.json: {e}");
            APIError::InternalError {
                message: format!("Failed to parse widget_versions.json: {e}"),
            }
        })
}

#[derive(Debug, Clone, Deserialize, IntoParams)]
pub struct CommandResolveQuery {
    #[serde(default)]
    #[param(inline)]
    pub region: LeaderboardRegion,
    #[serde(deserialize_with = "parse_steam_id")]
    pub account_id: u32,
    #[serde(default)]
    pub template: String,
    /// Hero name to check for hero specific stats
    #[serde(default)]
    pub hero_name: Option<String>,
}

#[utoipa::path(
    get,
    params(CommandResolveQuery),
    path = "/resolve",
    responses(
        (status = OK, body = String),
        (status = BAD_REQUEST, description = "Provided parameters are invalid."),
        (status = BAD_REQUEST, description = "Provided parameters are invalid."),
    ),
    tags = ["Commands"],
    summary = "Resolve Command",
    description = "Resolves a command and returns the resolved command."
)]
pub async fn command_resolve(
    State(state): State<AppState>,
    Query(query): Query<CommandResolveQuery>,
) -> String {
    if query.account_id == 0 {
        return "Invalid account ID".to_string();
    }
    let mut extra_args = HashMap::new();
    if let Some(hero_name) = query.hero_name {
        extra_args.insert("hero_name".to_string(), hero_name);
    }
    let mut resolved_template = query.template.clone();
    let results = futures::future::join_all(
        Variable::VARIANTS
            .iter()
            .filter(|v| query.template.contains(&format!("{{{}}}", v.get_name())))
            .map(|v| {
                let template_str = format!("{{{}}}", v.get_name());
                async {
                    match v
                        .resolve(&state, query.account_id, query.region, &extra_args)
                        .await
                    {
                        Ok(resolved) => Ok((template_str, resolved)),
                        Err(e) => Err(format!("Failed to resolve variable: {}, {e}", v.get_name())),
                    }
                }
            }),
    )
    .await;

    for result in results {
        match result {
            Ok((template_str, resolved_variable)) => {
                resolved_template = resolved_template.replace(&template_str, &resolved_variable);
            }
            Err(e) => return e,
        }
    }
    resolved_template
}

#[derive(Debug, Clone, Deserialize, IntoParams)]
pub struct VariablesResolveQuery {
    #[serde(default)]
    #[param(inline)]
    pub region: LeaderboardRegion,
    #[serde(deserialize_with = "parse_steam_id")]
    pub account_id: u32,
    /// Variables to resolve, separated by commas.
    #[serde(default)]
    pub variables: String,
    /// Hero name to check for hero specific stats
    #[serde(default)]
    pub hero_name: Option<String>,
}

#[utoipa::path(
    get,
    params(VariablesResolveQuery),
    path = "/variables/resolve",
    responses(
        (status = OK, body = HashMap<String, String>),
        (status = BAD_REQUEST, description = "Provided parameters are invalid."),
    ),
    tags = ["Commands"],
    summary = "Resolve Variables",
    description = "Resolves variables and returns a map of variable name to resolved value."
)]
pub async fn variables_resolve(
    State(state): State<AppState>,
    Query(query): Query<VariablesResolveQuery>,
) -> Json<HashMap<String, String>> {
    if query.account_id == 0 || query.variables.is_empty() {
        return Json(HashMap::new());
    }
    let mut extra_args = HashMap::new();
    if let Some(hero_name) = query.hero_name {
        extra_args.insert("hero_name".to_string(), hero_name);
    }
    let variables_to_resolve = query.variables.split(',').map(|v| v.trim()).collect_vec();
    let results = futures::future::join_all(
        Variable::VARIANTS
            .iter()
            .filter(|v| variables_to_resolve.contains(&v.get_name()))
            .map(|v| async {
                match v
                    .resolve(&state, query.account_id, query.region, &extra_args)
                    .await
                {
                    Ok(resolved) => Some((v.get_name().to_string(), resolved)),
                    Err(e) => {
                        warn!("Failed to resolve variable: {}, {e}", v.get_name());
                        None
                    }
                }
            }),
    )
    .await
    .into_iter()
    .flatten()
    .collect::<HashMap<_, _>>();

    Json(results)
}
