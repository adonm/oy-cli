//! OpenCode package setup and workflow integration.

mod api;
mod host;
mod runner;
mod setup;

pub(crate) use api::RuntimeHealth;
pub(crate) use host::OpenCodeHost;
pub(crate) use runner::{
    audit_workflow_command, enhance_workflow_command, launch_command, recover_workflow_command,
    review_workflow_command, run_task_command, runtime_health,
};
pub(crate) use setup::{global_config_path, setup_command, workspace_config_path};

const OY_AGENT: &str = include_str!("../packages/opencode/assets/agents/oy.md");
const OY_AUDIT_SKILL: &str = include_str!("../packages/opencode/assets/skills/oy-audit/SKILL.md");
const OY_REVIEW_SKILL: &str = include_str!("../packages/opencode/assets/skills/oy-review/SKILL.md");
const OY_ENHANCE_SKILL: &str =
    include_str!("../packages/opencode/assets/skills/oy-enhance/SKILL.md");

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn audit_skill_is_canonical() {
        assert!(OY_AUDIT_SKILL.contains("OWASP ASVS 5.0"));
        assert!(OY_AUDIT_SKILL.contains("oy audit prepare"));
        assert!(OY_AUDIT_SKILL.contains("oy audit finalize"));
        assert!(OY_AUDIT_SKILL.contains("opencode/autoinvoke: true"));
        assert!(OY_AUDIT_SKILL.contains("current OpenCode permissions"));
    }

    #[test]
    fn review_skill_is_canonical() {
        assert!(OY_REVIEW_SKILL.contains("complexity is the apex predator"));
        assert!(OY_REVIEW_SKILL.contains("oy review prepare"));
        assert!(OY_REVIEW_SKILL.contains("oy review finalize"));
        assert!(OY_REVIEW_SKILL.contains("current OpenCode permissions"));
    }

    #[test]
    fn generated_skills_require_deterministic_protocol() {
        for skill in [OY_AUDIT_SKILL, OY_REVIEW_SKILL] {
            assert!(skill.contains("Protocol"));
            assert!(skill.contains("`[]`"));
            assert!(skill.contains("untrusted"));
            assert!(skill.contains("continue paging with the native read offset"));
        }
        assert!(OY_ENHANCE_SKILL.contains("Fix one finding per pass"));
    }

    #[test]
    fn generated_oy_agent_is_autonomous_without_permission_overrides() {
        assert!(OY_AGENT.contains("mode: primary"));
        assert!(!OY_AGENT.contains("permissions:"));
        assert!(OY_AGENT.contains("carry the task through"));
        assert!(OY_AGENT.contains("focused, verified commits at natural checkpoints"));
        assert!(OY_AGENT.contains("Never discard or commit unrelated changes"));
        assert!(OY_AGENT.contains("push, force-push, or create tags unless explicitly asked"));
        assert!(OY_AGENT.contains("OpenCode and the user own permissions"));
    }
}
