use crate::agent::pipeline;
use crate::agent::tool_loop::LoopDetector;
use hydeclaw_types::ToolCall;
use serde_json::Value;
use std::collections::HashMap;
use uuid::Uuid;

pub use pipeline::parallel::LoopBreak;

impl pipeline::parallel::ToolExecutor for super::AgentEngine {
    fn execute_tool_call<'a>(
        &'a self,
        name: &'a str,
        arguments: &'a Value,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = String> + Send + 'a>> {
        self.execute_tool_call(name, arguments)
    }

    fn needs_approval(&self, tool_name: &str) -> bool {
        self.needs_approval(tool_name)
    }
}

impl super::AgentEngine {
    #[allow(clippy::too_many_arguments)]
    pub(super) async fn execute_tool_calls_partitioned(
        &self,
        tool_calls: &[ToolCall],
        context: &Value,
        session_id: Uuid,
        channel: &str,
        current_context_chars: usize,
        detector: &mut LoopDetector,
        detect_loops: bool,
    ) -> Result<Vec<(String, String)>, LoopBreak> {
        // Load YAML tools (cached for 30s)
        let yaml_tools: std::sync::Arc<HashMap<String, crate::tools::yaml_tools::YamlToolDef>> = {
            let cache = self.tex().yaml_tools_cache.read().await;
            if cache.0.elapsed() < std::time::Duration::from_secs(30) && !cache.1.is_empty() {
                std::sync::Arc::clone(&cache.1)
            } else {
                drop(cache);
                let tools = std::sync::Arc::new(
                    crate::tools::yaml_tools::load_yaml_tools(&self.workspace_dir, false)
                        .await
                        .into_iter()
                        .map(|t| (t.name.clone(), t))
                        .collect::<HashMap<String, crate::tools::yaml_tools::YamlToolDef>>(),
                );
                *self.tex().yaml_tools_cache.write().await =
                    (std::time::Instant::now(), std::sync::Arc::clone(&tools));
                tools
            }
        };

        let subagent_timeout =
            super::subagent_impl::parse_subagent_timeout(&self.app_config.subagents.in_process_timeout)
                + std::time::Duration::from_secs(10);

        pipeline::parallel::execute_tool_calls_partitioned(
            tool_calls,
            context,
            session_id,
            channel,
            &self.agent.model,
            current_context_chars,
            detector,
            detect_loops,
            &self.db,
            &self.embedder,
            &yaml_tools,
            subagent_timeout,
            self,
        )
        .await
    }
}
