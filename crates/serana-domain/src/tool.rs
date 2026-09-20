//! The tool port and the registry the turn loop dispatches through.

use std::collections::BTreeMap;
use std::sync::Arc;

use async_trait::async_trait;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::error::ToolError;

/// What the model is told about a tool.
///
/// `parameters` is a JSON Schema object. It is never hand-written: build it from a Rust
/// type with [`ToolSpec::typed`], so the schema and the type the handler deserialises
/// cannot drift apart.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ToolSpec {
    pub name: String,
    pub description: String,
    pub parameters: serde_json::Value,
}

impl ToolSpec {
    /// Derive the parameter schema from `A`, the type the handler will deserialise.
    ///
    /// Subschemas are inlined because providers do not reliably resolve `$ref`/`$defs` in
    /// tool parameters.
    pub fn typed<A: JsonSchema>(name: impl Into<String>, description: impl Into<String>) -> Self {
        let settings =
            schemars::generate::SchemaSettings::draft07().with(|s| s.inline_subschemas = true);
        let schema = schemars::SchemaGenerator::new(settings).into_root_schema_for::<A>();
        let mut parameters =
            serde_json::to_value(schema).expect("a JsonSchema type always serialises");

        // Provider-facing noise: `$schema` is rejected by some gateways and `title` only
        // burns context.
        if let Some(obj) = parameters.as_object_mut() {
            obj.remove("$schema");
            obj.remove("title");
        }
        Self {
            name: name.into(),
            description: description.into(),
            parameters,
        }
    }
}

/// Something the model can invoke.
///
/// Handlers return a `String` — by convention JSON — because that is what lands verbatim
/// in a tool-role message. Errors are values, not failures of the turn: the loop renders a
/// [`ToolError`] into a tool result and lets the model correct itself.
#[async_trait]
pub trait Tool: Send + Sync {
    fn spec(&self) -> &ToolSpec;

    /// `arguments` has already been parsed from the model's raw string and validated as an
    /// object. Deserialising it into the handler's own argument type may still fail, which
    /// is [`ToolError::InvalidArguments`].
    async fn invoke(&self, arguments: serde_json::Value) -> Result<String, ToolError>;
}

/// The set of tools available for one session.
///
/// Built per session at the composition root, not once per process: capability is a
/// property of the session, so the Telegram surface and the local CLI get different
/// registries from the same binary.
#[derive(Clone, Default)]
pub struct ToolRegistry {
    // BTreeMap so `specs()` is deterministically ordered — an unstable tool order would
    // change the prompt prefix between turns and break the prompt cache.
    tools: BTreeMap<String, Arc<dyn Tool>>,
}

impl ToolRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    /// Register `tool`, replacing any tool already registered under the same name.
    pub fn insert(&mut self, tool: Arc<dyn Tool>) -> &mut Self {
        self.tools.insert(tool.spec().name.clone(), tool);
        self
    }

    pub fn get(&self, name: &str) -> Option<&Arc<dyn Tool>> {
        self.tools.get(name)
    }

    pub fn contains(&self, name: &str) -> bool {
        self.tools.contains_key(name)
    }

    pub fn is_empty(&self) -> bool {
        self.tools.is_empty()
    }

    pub fn len(&self) -> usize {
        self.tools.len()
    }

    pub fn names(&self) -> impl Iterator<Item = &str> {
        self.tools.keys().map(String::as_str)
    }

    /// Specs to send to the provider, in a stable order.
    pub fn specs(&self) -> Vec<ToolSpec> {
        self.tools.values().map(|t| t.spec().clone()).collect()
    }
}

impl std::fmt::Debug for ToolRegistry {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ToolRegistry")
            .field("tools", &self.names().collect::<Vec<_>>())
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(JsonSchema)]
    #[allow(dead_code)]
    struct EchoArgs {
        /// Text to echo back.
        text: String,
        times: Option<u8>,
    }

    struct Echo(ToolSpec);

    #[async_trait]
    impl Tool for Echo {
        fn spec(&self) -> &ToolSpec {
            &self.0
        }

        async fn invoke(&self, arguments: serde_json::Value) -> Result<String, ToolError> {
            Ok(arguments.to_string())
        }
    }

    fn echo(name: &str) -> Arc<dyn Tool> {
        Arc::new(Echo(ToolSpec::typed::<EchoArgs>(
            name,
            "Echo the input back.",
        )))
    }

    #[test]
    fn a_typed_spec_describes_the_argument_type() {
        let spec = ToolSpec::typed::<EchoArgs>("echo", "Echo the input back.");
        assert_eq!(spec.parameters["type"], "object");
        assert!(spec.parameters["properties"].get("text").is_some());
        assert!(spec.parameters["properties"].get("times").is_some());
        assert_eq!(spec.parameters["required"], serde_json::json!(["text"]));
    }

    #[test]
    fn a_typed_spec_carries_no_provider_hostile_keys() {
        let spec = ToolSpec::typed::<EchoArgs>("echo", "Echo the input back.");
        let obj = spec.parameters.as_object().unwrap();
        assert!(!obj.contains_key("$schema"));
        assert!(!obj.contains_key("title"));
        assert!(
            !spec.parameters.to_string().contains("$ref"),
            "subschemas must be inlined"
        );
    }

    #[test]
    fn an_empty_registry_offers_nothing() {
        let registry = ToolRegistry::new();
        assert!(registry.is_empty());
        assert!(registry.specs().is_empty());
        assert!(registry.get("echo").is_none());
    }

    #[test]
    fn registered_tools_are_retrievable_by_name() {
        let mut registry = ToolRegistry::new();
        registry.insert(echo("echo"));
        assert_eq!(registry.len(), 1);
        assert!(registry.contains("echo"));
        assert!(registry.get("echo").is_some());
        assert!(registry.get("missing").is_none());
    }

    #[test]
    fn specs_are_ordered_deterministically_to_keep_the_prompt_prefix_stable() {
        let mut a = ToolRegistry::new();
        a.insert(echo("zeta"))
            .insert(echo("alpha"))
            .insert(echo("mid"));
        let mut b = ToolRegistry::new();
        b.insert(echo("mid"))
            .insert(echo("zeta"))
            .insert(echo("alpha"));

        let names = |r: &ToolRegistry| r.specs().into_iter().map(|s| s.name).collect::<Vec<_>>();
        assert_eq!(names(&a), vec!["alpha", "mid", "zeta"]);
        assert_eq!(names(&a), names(&b));
    }

    #[test]
    fn re_registering_a_name_replaces_it() {
        let mut registry = ToolRegistry::new();
        registry.insert(echo("echo"));
        registry.insert(Arc::new(Echo(ToolSpec::typed::<EchoArgs>(
            "echo",
            "A newer echo.",
        ))));
        assert_eq!(registry.len(), 1);
        assert_eq!(
            registry.get("echo").unwrap().spec().description,
            "A newer echo."
        );
    }

    #[tokio::test]
    async fn a_registered_tool_can_be_invoked_through_the_registry() {
        let mut registry = ToolRegistry::new();
        registry.insert(echo("echo"));
        let out = registry
            .get("echo")
            .unwrap()
            .invoke(serde_json::json!({"text": "hi"}))
            .await
            .unwrap();
        assert_eq!(out, r#"{"text":"hi"}"#);
    }
}
