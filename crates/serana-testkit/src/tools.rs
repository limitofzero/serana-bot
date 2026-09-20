//! Tools that behave predictably, including badly.

use async_trait::async_trait;
use schemars::JsonSchema;
use serana_domain::ToolError;
use serana_domain::tool::{Tool, ToolSpec};
use serde::Deserialize;

#[derive(Debug, Deserialize, JsonSchema)]
pub struct EchoArgs {
    /// Text to echo back.
    pub text: String,
}

/// Returns its argument. The simplest possible successful tool.
pub struct EchoTool(ToolSpec);

impl Default for EchoTool {
    fn default() -> Self {
        Self::new("echo")
    }
}

impl EchoTool {
    pub fn new(name: &str) -> Self {
        Self(ToolSpec::typed::<EchoArgs>(
            name,
            "Echo the input text back to the caller.",
        ))
    }
}

#[async_trait]
impl Tool for EchoTool {
    fn spec(&self) -> &ToolSpec {
        &self.0
    }

    async fn invoke(&self, arguments: serde_json::Value) -> Result<String, ToolError> {
        let args: EchoArgs = serde_json::from_value(arguments)
            .map_err(|e| ToolError::InvalidArguments(e.to_string()))?;
        Ok(args.text)
    }
}

/// Always fails, with a chosen error. For exercising the loop's tool-failure path.
pub struct FailingTool {
    spec: ToolSpec,
    error: ToolError,
}

impl FailingTool {
    pub fn new(name: &str, error: ToolError) -> Self {
        Self {
            spec: ToolSpec::typed::<EchoArgs>(name, "Always fails."),
            error,
        }
    }

    /// A tool that blows up during execution.
    pub fn exploding(name: &str) -> Self {
        Self::new(name, ToolError::Execution("boom".into()))
    }

    /// A tool that is not available in this session.
    pub fn unavailable(name: &str) -> Self {
        Self::new(name, ToolError::Unavailable("no credentials".into()))
    }
}

#[async_trait]
impl Tool for FailingTool {
    fn spec(&self) -> &ToolSpec {
        &self.spec
    }

    async fn invoke(&self, _arguments: serde_json::Value) -> Result<String, ToolError> {
        Err(self.error.clone())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn echo_returns_its_argument() {
        let tool = EchoTool::default();
        assert_eq!(tool.spec().name, "echo");
        let out = tool
            .invoke(serde_json::json!({"text": "hi"}))
            .await
            .unwrap();
        assert_eq!(out, "hi");
    }

    #[tokio::test]
    async fn echo_rejects_arguments_that_do_not_match_its_schema() {
        let tool = EchoTool::default();
        let err = tool
            .invoke(serde_json::json!({"wrong": 1}))
            .await
            .unwrap_err();
        assert!(matches!(err, ToolError::InvalidArguments(_)), "{err:?}");
    }

    #[tokio::test]
    async fn a_failing_tool_returns_the_error_it_was_built_with() {
        let exploding = FailingTool::exploding("boom");
        assert_eq!(
            exploding
                .invoke(serde_json::json!({"text": "x"}))
                .await
                .unwrap_err(),
            ToolError::Execution("boom".into())
        );
        let unavailable = FailingTool::unavailable("nope");
        assert!(matches!(
            unavailable.invoke(serde_json::json!({})).await.unwrap_err(),
            ToolError::Unavailable(_)
        ));
    }
}
