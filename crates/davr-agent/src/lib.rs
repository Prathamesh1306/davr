use davr_config::AgentConfig;
use davr_types::{DavrError, Result};
#[cfg(unix)]
use nix::sys::signal::{kill, Signal};
#[cfg(unix)]
use nix::unistd::Pid;
use std::collections::HashMap;
use std::env;
use std::path::Path;
use std::process::Stdio;
use std::time::Duration;
use tokio::process::Command;
use tokio::time::sleep;
use tracing::{debug, info, warn};

// =====================================================================
// Agent Adapters (Tier 1 universal + Tier 2 placeholders)
// =====================================================================

pub trait AgentAdapter: Send + Sync {
    fn id(&self) -> &'static str;
    fn detects(&self, command: &str) -> bool;
    fn build_env(&self, config: &AgentConfig) -> HashMap<String, String> {
        let mut filtered = HashMap::new();
        if config.sanitize_env {
            for key in &config.env_allowlist {
                if let Ok(val) = env::var(key) {
                    filtered.insert(key.clone(), val);
                }
            }
        } else {
            for (k, v) in env::vars() {
                filtered.insert(k, v);
            }
        }
        filtered
    }
}

pub struct GenericAgentAdapter;
impl AgentAdapter for GenericAgentAdapter {
    fn id(&self) -> &'static str {
        "generic"
    }

    fn detects(&self, _command: &str) -> bool {
        true
    }
}

pub struct ClaudeAgentAdapter;
impl AgentAdapter for ClaudeAgentAdapter {
    fn id(&self) -> &'static str {
        "claude"
    }

    fn detects(&self, command: &str) -> bool {
        command.contains("claude")
    }
}

pub struct AiderAgentAdapter;
impl AgentAdapter for AiderAgentAdapter {
    fn id(&self) -> &'static str {
        "aider"
    }

    fn detects(&self, command: &str) -> bool {
        command.contains("aider")
    }
}

pub struct OpenCodeAgentAdapter;
impl AgentAdapter for OpenCodeAgentAdapter {
    fn id(&self) -> &'static str {
        "opencode"
    }

    fn detects(&self, command: &str) -> bool {
        command.contains("opencode")
    }
}

pub fn select_adapter(agent_name: &str) -> Box<dyn AgentAdapter> {
    match agent_name.to_lowercase().as_str() {
        "claude" => Box::new(ClaudeAgentAdapter),
        "aider" => Box::new(AiderAgentAdapter),
        "opencode" => Box::new(OpenCodeAgentAdapter),
        _ => Box::new(GenericAgentAdapter),
    }
}

// =====================================================================
// Process Supervisor (Process Group Supervision & Signals)
// =====================================================================

pub struct ProcessSupervisor {
    project_root: std::path::PathBuf,
    config: AgentConfig,
}

impl ProcessSupervisor {
    pub fn new(project_root: impl AsRef<Path>, config: AgentConfig) -> Self {
        Self {
            project_root: project_root.as_ref().to_path_buf(),
            config,
        }
    }

    /// Spawns the agent process inside a process group and supervises it to completion
    pub async fn run_supervised(&self, command_line: &str, args: &[String]) -> Result<i32> {
        let adapter = select_adapter(&self.config.default_agent);
        let env_map = adapter.build_env(&self.config);

        let mut cmd = Command::new(command_line);
        cmd.args(args);
        cmd.current_dir(&self.project_root);
        cmd.stdin(Stdio::inherit());
        cmd.stdout(Stdio::inherit());
        cmd.stderr(Stdio::inherit());

        // Set sanitized environment
        if self.config.sanitize_env {
            cmd.env_clear();
            for (k, v) in env_map {
                cmd.env(k, v);
            }
        }

        // On Unix, spawn in its own process group so we can signal child trees
        #[cfg(unix)]
        {
            cmd.process_group(0);
        }

        info!(cmd = %command_line, "Spawning supervised agent process");
        let mut child = cmd
            .spawn()
            .map_err(|e| DavrError::Agent(format!("Failed to spawn agent process: {}", e)))?;

        let child_id = child.id().unwrap_or(0);

        let timeout_secs = self.config.timeout_seconds;
        let timeout_fut = async move {
            if timeout_secs > 0 {
                sleep(Duration::from_secs(timeout_secs)).await;
                true
            } else {
                std::future::pending::<bool>().await
            }
        };

        tokio::select! {
            res = child.wait() => {
                match res {
                    Ok(status) => {
                        let code = status.code().unwrap_or(1);
                        debug!(exit_code = code, "Agent process completed");
                        Ok(code)
                    }
                    Err(e) => Err(DavrError::Agent(format!("Error waiting for agent: {}", e))),
                }
            }
            _ = timeout_fut => {
                warn!(pid = child_id, "Agent session exceeded timeout; killing process group");
                terminate_process_tree(child_id, false);
                sleep(Duration::from_millis(500)).await;
                terminate_process_tree(child_id, true);
                Err(DavrError::Agent("Agent session timed out".into()))
            }
            _ = tokio::signal::ctrl_c() => {
                info!(pid = child_id, "Received Ctrl+C; forwarding termination signal to agent process tree");
                terminate_process_tree(child_id, false);
                sleep(Duration::from_millis(500)).await;
                terminate_process_tree(child_id, true);
                Err(DavrError::Agent("Session aborted by user via Ctrl+C".into()))
            }
        }
    }
}

fn terminate_process_tree(pid: u32, force: bool) {
    if pid == 0 {
        return;
    }
    #[cfg(unix)]
    {
        // Negative PID sends signal to the entire process group
        let pgid = Pid::from_raw(-(pid as i32));
        let sig = if force {
            Signal::SIGKILL
        } else {
            Signal::SIGTERM
        };
        let _ = kill(pgid, sig);
    }
    #[cfg(windows)]
    {
        // On Windows, taskkill /F /T kills the specified process and any child processes started by it
        let mut cmd = std::process::Command::new("taskkill");
        if force {
            cmd.arg("/F");
        }
        cmd.args(["/T", "/PID", &pid.to_string()]);
        let _ = cmd.output();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[tokio::test]
    async fn test_process_supervisor_echo() {
        let temp = TempDir::new().unwrap();
        let config = AgentConfig {
            default_agent: "generic".into(),
            allowed_agents: vec![],
            timeout_seconds: 5,
            sanitize_env: false,
            env_allowlist: vec![],
        };

        let supervisor = ProcessSupervisor::new(temp.path(), config);
        let exit_code = supervisor
            .run_supervised("echo", &["davr_test_agent".into()])
            .await
            .unwrap();

        assert_eq!(exit_code, 0);
    }
}
