//! Cedar authorization-policy support.
//!
//! This is an *additional* restriction layer that sits alongside the TOML
//! configuration. When no policy is loaded, behavior is unchanged
//! (default-allow). When a policy is loaded, every gated action must be
//! permitted by the policy or it is denied (Cedar's normal default-deny) —
//! and this layers *on top of* the built-in SSRF and VFS-permission checks,
//! which it can never weaken.
//!
//! Only the generic, public `cedar-policy` API is used: parse the baked
//! schema, parse and schema-validate the policy text, then evaluate requests
//! with [`cedar_policy::Authorizer::is_authorized`]. The action vocabulary and
//! singleton-resource model are described in `schemas/agent.cedarschema`.
//!
//! Not compiled for `wasm32` — `cedar-policy` is not wasm-safe.

use std::io;
use std::str::FromStr;

use cedar_policy::{
    Authorizer, Context, Decision, Entities, Entity, EntityId, EntityTypeName, EntityUid,
    PolicySet, Request, RestrictedExpression, Schema, ValidationMode, Validator,
};

/// The baked Cedar schema, embedded at compile time. The engine validates
/// every loaded policy against this, so a policy that references an unknown
/// action or a malformed context is rejected at load time.
const SCHEMA_SRC: &str = include_str!("../schemas/agent.cedarschema");

/// The fixed principal UID used for every authorization request:
/// `Agent::Shell::"default"`.
const PRINCIPAL_ID: &str = "default";

/// The singleton resource UID shared by every resource type: `..::"global"`.
const SINGLETON_RESOURCE_ID: &str = "global";

/// A loaded, schema-validated Cedar policy set plus an authorizer.
///
/// Cheap to share: [`check`](Self::check) takes `&self`, so a single engine is
/// wrapped in an `Arc` and held by the kernel.
pub struct PolicyEngine {
    policy_set: PolicySet,
    authorizer: Authorizer,
}

impl PolicyEngine {
    /// Parse and schema-validate `policy_text`, returning a ready engine.
    ///
    /// # Errors
    ///
    /// Returns an error if the baked schema fails to parse (a build-time bug),
    /// if `policy_text` is not valid Cedar, or if the policy fails validation
    /// against the schema (e.g. it references an unknown action).
    // Not `std::str::FromStr`: this is fallible-with-`io::Error` and reads
    // naturally as `PolicyEngine::from_str(text)`.
    #[allow(clippy::should_implement_trait)]
    pub fn from_str(policy_text: &str) -> io::Result<Self> {
        let schema = Schema::from_cedarschema_str(SCHEMA_SRC)
            .map_err(|e| {
                io::Error::new(
                    io::ErrorKind::InvalidData,
                    format!("invalid baked Cedar schema: {e}"),
                )
            })?
            .0;

        let policy_set = PolicySet::from_str(policy_text).map_err(|e| {
            io::Error::new(
                io::ErrorKind::InvalidData,
                format!("invalid Cedar policy: {e}"),
            )
        })?;

        let validator = Validator::new(schema);
        let result = validator.validate(&policy_set, ValidationMode::default());
        if !result.validation_passed() {
            let errors: Vec<String> = result.validation_errors().map(|e| e.to_string()).collect();
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!(
                    "Cedar policy failed schema validation: {}",
                    errors.join("; ")
                ),
            ));
        }

        Ok(Self {
            policy_set,
            authorizer: Authorizer::new(),
        })
    }

    /// Evaluate `action` with the given `context.input` `fields`.
    ///
    /// Returns `Ok(())` if the policy permits the action, or a
    /// `PermissionDenied` error otherwise. `fields` are the per-call inputs the
    /// schema declares for the action (e.g. `[("path", "/home/lash/x")]`).
    pub fn check(&self, action: &str, fields: &[(&str, &str)]) -> io::Result<()> {
        let principal = entity_uid("Shell", PRINCIPAL_ID)?;
        let resource = entity_uid(resource_type_for(action), SINGLETON_RESOURCE_ID)?;
        let action_uid = EntityUid::from_str(&format!("Agent::Action::\"{action}\""))
            .map_err(|e| io::Error::other(format!("invalid action UID for {action}: {e}")))?;

        let input_record = RestrictedExpression::new_record(fields.iter().map(|(k, v)| {
            (
                (*k).to_owned(),
                RestrictedExpression::new_string((*v).to_owned()),
            )
        }))
        .map_err(|e| io::Error::other(format!("invalid context.input record: {e}")))?;
        let context = Context::from_pairs([("input".to_owned(), input_record)])
            .map_err(|e| io::Error::other(format!("invalid Cedar context: {e}")))?;

        let request = Request::new(
            principal.clone(),
            action_uid,
            resource.clone(),
            context,
            None,
        )
        .map_err(|e| io::Error::other(format!("invalid Cedar request: {e}")))?;

        let entities = Entities::from_entities(
            [
                Entity::new_no_attrs(principal, Default::default()),
                Entity::new_no_attrs(resource, Default::default()),
            ],
            None,
        )
        .map_err(|e| io::Error::other(format!("invalid Cedar entities: {e}")))?;

        let response = self
            .authorizer
            .is_authorized(&request, &self.policy_set, &entities);

        if response.decision() == Decision::Allow {
            Ok(())
        } else {
            Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                format!("policy denied: {action} {}", describe(fields)),
            ))
        }
    }
}

/// Build an `Agent::<type_name>::"<id>"` entity UID.
fn entity_uid(type_name: &str, id: &str) -> io::Result<EntityUid> {
    let type_name = EntityTypeName::from_str(&format!("Agent::{type_name}"))
        .map_err(|e| io::Error::other(format!("invalid entity type Agent::{type_name}: {e}")))?;
    Ok(EntityUid::from_type_name_and_id(
        type_name,
        EntityId::new(id),
    ))
}

/// Map an action name to its singleton resource type.
fn resource_type_for(action: &str) -> &'static str {
    match action {
        "net:request" => "Network",
        "env:read" => "Environment",
        "mcp:call" => "McpService",
        // All fs:* actions, and any unknown action, target Filesystem; an
        // unknown action would already have failed validation at load time.
        _ => "Filesystem",
    }
}

/// Render the input fields for an error message, e.g. `path=/x`.
fn describe(fields: &[(&str, &str)]) -> String {
    fields
        .iter()
        .map(|(k, v)| format!("{k}={v}"))
        .collect::<Vec<_>>()
        .join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn baked_schema_parses() {
        // A trivial valid policy proves the schema parses and validation runs.
        let engine = PolicyEngine::from_str("permit(principal, action, resource);").unwrap();
        // Blanket permit allows any action.
        assert!(engine.check("fs:read", &[("path", "/x")]).is_ok());
        assert!(
            engine
                .check("net:request", &[("url", "http://x/"), ("method", "GET")])
                .is_ok()
        );
    }

    #[test]
    fn empty_policy_denies_everything() {
        let engine = PolicyEngine::from_str("").unwrap();
        assert!(engine.check("fs:read", &[("path", "/x")]).is_err());
    }

    #[test]
    fn permit_matches_specific_path() {
        let policy = r#"
            permit(principal, action == Agent::Action::"fs:read", resource)
            when { context.input.path == "/home/lash/ok.txt" };
        "#;
        let engine = PolicyEngine::from_str(policy).unwrap();
        assert!(
            engine
                .check("fs:read", &[("path", "/home/lash/ok.txt")])
                .is_ok()
        );
        assert!(
            engine
                .check("fs:read", &[("path", "/home/lash/secret.txt")])
                .is_err()
        );
        // A different action is not permitted by this policy.
        assert!(
            engine
                .check("fs:write", &[("path", "/home/lash/ok.txt")])
                .is_err()
        );
    }

    #[test]
    fn malformed_policy_is_rejected() {
        assert!(PolicyEngine::from_str("permit(garbage").is_err());
    }

    #[test]
    fn unknown_action_fails_validation() {
        // Syntactically valid, but references an action absent from the schema.
        let policy = r#"permit(principal, action == Agent::Action::"fs:bogus", resource);"#;
        assert!(PolicyEngine::from_str(policy).is_err());
    }
}
