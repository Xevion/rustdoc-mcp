use crate::context::ServerContext;
use anyhow::{Result, anyhow};
use rmcp::schemars;
use serde::Deserialize;

/// Parameters for list_crates tool
#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct ListCratesRequest {
    /// Optional workspace member to filter dependencies for
    #[serde(skip_serializing_if = "Option::is_none")]
    pub workspace_member: Option<String>,
}

/// List all crates available in the configured workspace.
///
/// Shows workspace members and dependencies with their resolved versions
/// in a simple, flat format.
pub fn execute_list_crates(context: &ServerContext, _request: ListCratesRequest) -> Result<String> {
    // Verify workspace is configured
    let workspace_metadata = context
        .workspace_metadata()
        .ok_or_else(|| anyhow!("Workspace not configured. Use set_workspace tool first."))?;

    let mut output = String::new();

    // Show workspace members
    if !workspace_metadata.members.is_empty() {
        output.push_str(&format!(
            "Workspace Members ({}):\n",
            workspace_metadata.members.len()
        ));

        for member in &workspace_metadata.members {
            output.push_str(&format!("  • {}\n", member));
        }
        output.push('\n');
    }

    // Show dependencies
    if !workspace_metadata.dependencies.is_empty() {
        output.push_str(&format!(
            "Dependencies ({}):\n",
            workspace_metadata.dependencies.len()
        ));

        let mut sorted_deps = workspace_metadata.dependencies.clone();
        sorted_deps.sort_by(|a, b| a.0.cmp(&b.0));

        // Show first 20 dependencies in detail
        for (name, version) in sorted_deps.iter().take(20) {
            output.push_str(&format!("  • {} v{}\n", name, version));
        }

        // Show abbreviated list for remaining dependencies
        if workspace_metadata.dependencies.len() > 20 {
            output.push_str(&format!(
                "  ... and {} more dependencies\n",
                workspace_metadata.dependencies.len() - 20
            ));
        }
    } else {
        output.push_str("No external dependencies found.\n");
    }

    Ok(output)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::context::{ServerContext, WorkspaceMetadata};
    use std::path::PathBuf;

    #[test]
    fn test_list_crates_no_workspace() {
        let context = ServerContext::new();
        let request = ListCratesRequest {
            workspace_member: None,
        };

        let result = execute_list_crates(&context, request);
        assert!(result.is_err());
        assert!(
            result
                .unwrap_err()
                .to_string()
                .contains("Workspace not configured")
        );
    }

    #[test]
    fn test_list_crates_with_workspace() {
        let mut context = ServerContext::new();
        let workspace_metadata = WorkspaceMetadata {
            root: PathBuf::from("/test/project"),
            members: vec!["my-crate".to_string()],
            dependencies: vec![
                ("serde".to_string(), "1.0.0".to_string()),
                ("tokio".to_string(), "1.0.0".to_string()),
            ],
        };

        context.set_workspace_metadata(workspace_metadata);

        let request = ListCratesRequest {
            workspace_member: None,
        };

        let result = execute_list_crates(&context, request).unwrap();

        assert!(result.contains("Workspace Members (1)"));
        assert!(result.contains("my-crate"));
        assert!(result.contains("Dependencies (2)"));
        assert!(result.contains("serde v1.0.0"));
        assert!(result.contains("tokio v1.0.0"));
    }
}
