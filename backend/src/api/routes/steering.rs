//! Traffic steering API route handlers.
//!
//! Handlers for /api/wan/steering/* endpoints for Level 2 traffic steering
//! rule management. Rules control which WAN interface handles specific
//! traffic based on source/destination IP, protocol, and port criteria.

use std::sync::Arc;

use axum::extract::{Path, State};
use axum::{Extension, Json};
use serde::{Deserialize, Serialize};
use tracing::{info, warn};
use uuid::Uuid;

use crate::api::auth_middleware::{require_admin, SessionUser};
use crate::api::error::{ApiError, ApiResult};
use crate::api::steering::{
    assign_priorities, apply_rule, create_steering_chain, flush_steering,
    save_rules, validate_rule, FailoverMode, PortMatch, Protocol, RuleStatus,
    SteeringRule, STEERING_CONFIG_PATH, STEERING_MAX_RULES,
};
use crate::hardware::FirewallBackend;
use crate::state::AppState;

// ── Request / Response Types ─────────────────────────────────────────

/// Request body for creating a new steering rule.
#[derive(Debug, Deserialize)]
pub struct CreateSteeringRuleRequest {
    pub name: String,
    #[serde(default = "default_true")]
    pub enabled: bool,
    pub source_ip: Option<Vec<String>>,
    pub destination_ip: Option<Vec<String>>,
    pub protocol: Option<Protocol>,
    pub destination_port: Option<PortMatch>,
    pub source_port: Option<PortMatch>,
    pub target_wan: String,
    #[serde(default)]
    pub failover_mode: FailoverMode,
    pub fallback_wan: Option<String>,
}

fn default_true() -> bool {
    true
}

/// Request body for updating an existing steering rule.
/// All fields are optional for partial updates.
/// `Option<Option<T>>` fields: outer None = not provided, Some(None) = set to null.
#[derive(Debug, Deserialize)]
pub struct UpdateSteeringRuleRequest {
    pub name: Option<String>,
    pub enabled: Option<bool>,
    #[serde(default, deserialize_with = "deserialize_option_option")]
    pub source_ip: Option<Option<Vec<String>>>,
    #[serde(default, deserialize_with = "deserialize_option_option")]
    pub destination_ip: Option<Option<Vec<String>>>,
    #[serde(default, deserialize_with = "deserialize_option_option")]
    pub protocol: Option<Option<Protocol>>,
    #[serde(default, deserialize_with = "deserialize_option_option")]
    pub destination_port: Option<Option<PortMatch>>,
    #[serde(default, deserialize_with = "deserialize_option_option")]
    pub source_port: Option<Option<PortMatch>>,
    pub target_wan: Option<String>,
    pub failover_mode: Option<FailoverMode>,
    #[serde(default, deserialize_with = "deserialize_option_option")]
    pub fallback_wan: Option<Option<String>>,
}

/// Deserialize `Option<Option<T>>`: absent = `None`, `null` = `Some(None)`, value = `Some(Some(v))`.
fn deserialize_option_option<'de, T, D>(deserializer: D) -> Result<Option<Option<T>>, D::Error>
where
    T: Deserialize<'de>,
    D: serde::Deserializer<'de>,
{
    Ok(Some(Option::deserialize(deserializer)?))
}

/// Request body for reordering steering rules.
#[derive(Debug, Deserialize)]
pub struct ReorderRequest {
    pub order: Vec<String>,
}

/// Response for a single steering rule (includes resolved WAN label).
#[derive(Debug, Serialize)]
pub struct SteeringRuleResponse {
    pub id: String,
    pub name: String,
    pub enabled: bool,
    pub priority: u32,
    pub source_ip: Option<Vec<String>>,
    pub destination_ip: Option<Vec<String>>,
    pub protocol: Option<Protocol>,
    pub destination_port: Option<PortMatch>,
    pub source_port: Option<PortMatch>,
    pub target_wan: String,
    pub target_wan_label: Option<String>,
    pub failover_mode: FailoverMode,
    pub fallback_wan: Option<String>,
    pub status: RuleStatus,
    pub fwmark: u32,
}

/// Response for listing all steering rules.
#[derive(Debug, Serialize)]
pub struct SteeringListResponse {
    pub rules: Vec<SteeringRuleResponse>,
    pub firewall_backend: String,
}

// ── Helper Functions ─────────────────────────────────────────────────

/// Convert a SteeringRule to a SteeringRuleResponse with resolved WAN label.
fn rule_to_response(rule: &SteeringRule, label: Option<String>) -> SteeringRuleResponse {
    SteeringRuleResponse {
        id: rule.id.clone(),
        name: rule.name.clone(),
        enabled: rule.enabled,
        priority: rule.priority,
        source_ip: rule.source_ip.clone(),
        destination_ip: rule.destination_ip.clone(),
        protocol: rule.protocol.clone(),
        destination_port: rule.destination_port.clone(),
        source_port: rule.source_port.clone(),
        target_wan: rule.target_wan.clone(),
        target_wan_label: label,
        failover_mode: rule.failover_mode.clone(),
        fallback_wan: rule.fallback_wan.clone(),
        status: rule.status.clone(),
        fwmark: rule.fwmark,
    }
}

/// Look up the human-readable label for a WAN modem_id from wan_config.
async fn get_wan_label(state: &AppState, modem_id: &str) -> Option<String> {
    let wan_config = state.wan_config.read().await;
    wan_config
        .modem_priority
        .iter()
        .find(|entry| entry.modem_id == modem_id)
        .map(|entry| entry.label.clone())
}

/// Get all WAN modem_ids from the current wan_config.
async fn get_wan_ids(state: &AppState) -> Vec<String> {
    let wan_config = state.wan_config.read().await;
    wan_config
        .modem_priority
        .iter()
        .map(|entry| entry.modem_id.clone())
        .collect()
}

/// Flush and rebuild all firewall rules from the current rule set.
///
/// Acquires platform_capabilities and routing_state locks in consistent order,
/// then flushes all existing firewall rules and re-applies enabled rules.
async fn rebuild_firewall_rules(
    state: &AppState,
    rules: &mut [SteeringRule],
) -> Result<(), ApiError> {
    // Acquire locks in consistent order: platform_capabilities first, then routing_state
    let capabilities = state.platform_capabilities.read().await;
    let fw_backend = capabilities.firewall_backend.clone();
    drop(capabilities);

    if fw_backend == FirewallBackend::Unknown {
        warn!("Unknown firewall backend — steering rules will not be applied");
        for rule in rules.iter_mut() {
            if rule.enabled {
                rule.status = RuleStatus::Dormant;
            }
        }
        return Ok(());
    }

    let routing_state = state.routing_state.read().await;
    let routing_snapshot = routing_state.clone();
    drop(routing_state);

    // Flush existing rules
    flush_steering(&fw_backend);

    // Recreate the chain
    create_steering_chain(&fw_backend).map_err(|e| {
        ApiError::internal(format!("Failed to create steering chain: {e}"))
    })?;

    // Apply each enabled rule
    for rule in rules.iter_mut() {
        if !rule.enabled {
            rule.status = RuleStatus::Blocked;
            continue;
        }

        match routing_snapshot.get(&rule.target_wan) {
            Some(entry) => {
                match apply_rule(rule, entry.table_number, &fw_backend) {
                    Ok(()) => {
                        rule.status = RuleStatus::Active;
                    }
                    Err(e) => {
                        warn!(
                            "Failed to apply steering rule '{}': {e}",
                            rule.name
                        );
                        rule.status = RuleStatus::Dormant;
                    }
                }
            }
            None => {
                rule.status = RuleStatus::Dormant;
            }
        }
    }

    Ok(())
}

// ── Route Handlers ───────────────────────────────────────────────────

/// GET /wan/steering — List all steering rules with runtime status.
pub async fn list_rules(
    State(state): State<Arc<AppState>>,
) -> ApiResult<Json<SteeringListResponse>> {
    let rules = state.steering_rules.read().await;
    let wan_config = state.wan_config.read().await;
    let capabilities = state.platform_capabilities.read().await;

    let firewall_backend = format!("{:?}", capabilities.firewall_backend).to_lowercase();
    drop(capabilities);

    let mut response_rules = Vec::with_capacity(rules.len());
    for rule in rules.iter() {
        let label = wan_config
            .modem_priority
            .iter()
            .find(|entry| entry.modem_id == rule.target_wan)
            .map(|entry| entry.label.clone());
        response_rules.push(rule_to_response(rule, label));
    }

    drop(wan_config);
    drop(rules);

    Ok(Json(SteeringListResponse {
        rules: response_rules,
        firewall_backend,
    }))
}

/// POST /wan/steering — Create a new steering rule.
pub async fn create_rule(
    State(state): State<Arc<AppState>>,
    Extension(session_user): Extension<SessionUser>,
    Json(req): Json<CreateSteeringRuleRequest>,
) -> ApiResult<Json<SteeringRuleResponse>> {
    require_admin(&session_user)?;
    let wan_ids = get_wan_ids(&state).await;

    // Build the rule from the request
    let new_rule = SteeringRule {
        id: Uuid::new_v4().to_string(),
        name: req.name,
        enabled: req.enabled,
        priority: 0,
        source_ip: req.source_ip,
        destination_ip: req.destination_ip,
        protocol: req.protocol,
        destination_port: req.destination_port,
        source_port: req.source_port,
        target_wan: req.target_wan,
        failover_mode: req.failover_mode,
        fallback_wan: req.fallback_wan,
        status: RuleStatus::Active,
        fwmark: 0,
    };

    // Validate
    validate_rule(&new_rule, &wan_ids)
        .map_err(ApiError::bad_request)?;

    // Acquire write lock and mutate
    let mut rules = state.steering_rules.write().await;

    // Check max rule count
    if rules.len() >= STEERING_MAX_RULES as usize {
        return Err(ApiError::bad_request(format!(
            "Maximum number of steering rules ({STEERING_MAX_RULES}) reached"
        )));
    }

    // Append and reassign priorities
    rules.push(new_rule);
    assign_priorities(&mut rules);

    // Save to disk
    save_rules(STEERING_CONFIG_PATH, &rules)
        .map_err(|e| ApiError::internal(format!("Failed to save steering rules: {e}")))?;

    // Rebuild firewall rules
    rebuild_firewall_rules(&state, &mut rules).await?;

    // Return the created rule
    let created = rules.last().unwrap();
    let label = get_wan_label(&state, &created.target_wan).await;
    let response = rule_to_response(created, label);

    drop(rules);

    info!("Created steering rule '{}' ({})", response.name, response.id);
    Ok(Json(response))
}

/// PUT /wan/steering/:id — Update an existing steering rule.
pub async fn update_rule(
    State(state): State<Arc<AppState>>,
    Extension(session_user): Extension<SessionUser>,
    Path(id): Path<String>,
    Json(req): Json<UpdateSteeringRuleRequest>,
) -> ApiResult<Json<SteeringRuleResponse>> {
    require_admin(&session_user)?;
    let wan_ids = get_wan_ids(&state).await;

    let mut rules = state.steering_rules.write().await;

    // Find the rule by ID
    let rule = rules
        .iter_mut()
        .find(|r| r.id == id)
        .ok_or_else(|| ApiError::not_found(format!("Steering rule not found: {id}")))?;

    // Apply partial updates
    if let Some(name) = req.name {
        rule.name = name;
    }
    if let Some(enabled) = req.enabled {
        rule.enabled = enabled;
    }
    if let Some(source_ip) = req.source_ip {
        rule.source_ip = source_ip;
    }
    if let Some(destination_ip) = req.destination_ip {
        rule.destination_ip = destination_ip;
    }
    if let Some(protocol) = req.protocol {
        rule.protocol = protocol;
    }
    if let Some(destination_port) = req.destination_port {
        rule.destination_port = destination_port;
    }
    if let Some(source_port) = req.source_port {
        rule.source_port = source_port;
    }
    if let Some(target_wan) = req.target_wan {
        rule.target_wan = target_wan;
    }
    if let Some(failover_mode) = req.failover_mode {
        rule.failover_mode = failover_mode;
    }
    if let Some(fallback_wan) = req.fallback_wan {
        rule.fallback_wan = fallback_wan;
    }

    // Validate the updated rule
    validate_rule(rule, &wan_ids)
        .map_err(ApiError::bad_request)?;

    // Save to disk
    save_rules(STEERING_CONFIG_PATH, &rules)
        .map_err(|e| ApiError::internal(format!("Failed to save steering rules: {e}")))?;

    // Rebuild firewall rules
    rebuild_firewall_rules(&state, &mut rules).await?;

    // Find the rule again to get updated status
    let updated = rules
        .iter()
        .find(|r| r.id == id)
        .unwrap();
    let label = get_wan_label(&state, &updated.target_wan).await;
    let response = rule_to_response(updated, label);

    drop(rules);

    info!("Updated steering rule '{}' ({})", response.name, response.id);
    Ok(Json(response))
}

/// DELETE /wan/steering/:id — Delete a steering rule.
pub async fn delete_rule(
    State(state): State<Arc<AppState>>,
    Extension(session_user): Extension<SessionUser>,
    Path(id): Path<String>,
) -> ApiResult<axum::http::StatusCode> {
    require_admin(&session_user)?;
    let mut rules = state.steering_rules.write().await;

    let original_len = rules.len();
    rules.retain(|r| r.id != id);

    if rules.len() == original_len {
        return Err(ApiError::not_found(format!("Steering rule not found: {id}")));
    }

    // Reassign priorities after removal
    assign_priorities(&mut rules);

    // Save to disk
    save_rules(STEERING_CONFIG_PATH, &rules)
        .map_err(|e| ApiError::internal(format!("Failed to save steering rules: {e}")))?;

    // Rebuild firewall rules
    rebuild_firewall_rules(&state, &mut rules).await?;

    drop(rules);

    info!("Deleted steering rule {id}");
    Ok(axum::http::StatusCode::NO_CONTENT)
}

/// PUT /wan/steering/reorder — Reorder steering rules by providing ordered ID array.
pub async fn reorder_rules(
    State(state): State<Arc<AppState>>,
    Extension(session_user): Extension<SessionUser>,
    Json(req): Json<ReorderRequest>,
) -> ApiResult<Json<SteeringListResponse>> {
    require_admin(&session_user)?;
    let mut rules = state.steering_rules.write().await;

    // Validate: all IDs must exist and count must match
    if req.order.len() != rules.len() {
        return Err(ApiError::bad_request(format!(
            "Order array length ({}) does not match rule count ({})",
            req.order.len(),
            rules.len()
        )));
    }

    for id in &req.order {
        if !rules.iter().any(|r| &r.id == id) {
            return Err(ApiError::bad_request(format!(
                "Rule ID not found: {id}"
            )));
        }
    }

    // Reorder rules to match the provided order
    let mut reordered = Vec::with_capacity(rules.len());
    for id in &req.order {
        let rule = rules.iter().find(|r| &r.id == id).unwrap().clone();
        reordered.push(rule);
    }
    *rules = reordered;

    // Reassign priorities based on new order
    assign_priorities(&mut rules);

    // Save to disk
    save_rules(STEERING_CONFIG_PATH, &rules)
        .map_err(|e| ApiError::internal(format!("Failed to save steering rules: {e}")))?;

    // Rebuild firewall rules
    rebuild_firewall_rules(&state, &mut rules).await?;

    // Build response
    let wan_config = state.wan_config.read().await;
    let capabilities = state.platform_capabilities.read().await;
    let firewall_backend = format!("{:?}", capabilities.firewall_backend).to_lowercase();
    drop(capabilities);

    let mut response_rules = Vec::with_capacity(rules.len());
    for rule in rules.iter() {
        let label = wan_config
            .modem_priority
            .iter()
            .find(|entry| entry.modem_id == rule.target_wan)
            .map(|entry| entry.label.clone());
        response_rules.push(rule_to_response(rule, label));
    }

    drop(wan_config);
    drop(rules);

    info!("Reordered {} steering rules", response_rules.len());
    Ok(Json(SteeringListResponse {
        rules: response_rules,
        firewall_backend,
    }))
}
