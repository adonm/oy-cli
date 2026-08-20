//! Standard agent-skills setup and the canonical oy skill/persona assets.

mod opencode_api;
mod opencode_host;
mod setup;

pub(crate) use setup::{
    global_skills_dir, plugin_cache_paths, setup_command, skills_complete, workspace_skills_dir,
};

pub(crate) const OY_AUDIT_SKILL: &str = include_str!("../assets/skills/oy-audit/SKILL.md");
pub(crate) const OY_REVIEW_SKILL: &str = include_str!("../assets/skills/oy-review/SKILL.md");
pub(crate) const OY_ENHANCE_SKILL: &str = include_str!("../assets/skills/oy-enhance/SKILL.md");
pub(crate) const OY_SETUP_SKILL: &str = include_str!("../assets/skills/oy-setup/SKILL.md");
pub(crate) const OY_PERSONA: &str = include_str!("../assets/skills/oy-setup/oy-persona.md");

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn audit_skill_is_canonical() {
        assert!(OY_AUDIT_SKILL.contains("OWASP ASVS 5.0"));
        assert!(OY_AUDIT_SKILL.contains("oy audit prepare"));
        assert!(OY_AUDIT_SKILL.contains("oy audit finalize"));
        assert!(OY_AUDIT_SKILL.contains("opencode/autoinvoke: true"));
        assert!(OY_AUDIT_SKILL.contains("your active permissions"));
    }

    #[test]
    fn review_skill_is_canonical() {
        assert!(OY_REVIEW_SKILL.contains("complexity is the apex predator"));
        assert!(OY_REVIEW_SKILL.contains("oy review prepare"));
        assert!(OY_REVIEW_SKILL.contains("oy review finalize"));
        assert!(OY_REVIEW_SKILL.contains("your active permissions"));
    }

    #[test]
    fn generated_skills_require_deterministic_protocol() {
        for skill in [OY_AUDIT_SKILL, OY_REVIEW_SKILL] {
            assert!(skill.contains("Protocol"));
            assert!(skill.contains("`[]`"));
            assert!(skill.contains("untrusted"));
            assert!(skill.contains("continue paging with your native read offset"));
        }
        assert!(OY_ENHANCE_SKILL.contains("Fix one finding per pass"));
    }

    #[test]
    fn generated_skills_are_host_neutral() {
        for skill in [
            OY_AUDIT_SKILL,
            OY_REVIEW_SKILL,
            OY_ENHANCE_SKILL,
            OY_SETUP_SKILL,
        ] {
            assert!(!skill.contains("current OpenCode permissions"));
            assert!(!skill.contains("OpenCode's native"));
            assert!(!skill.contains("OpenCode denies"));
        }
    }

    #[test]
    fn setup_skill_is_canonical() {
        assert!(OY_SETUP_SKILL.contains("oy setup"));
        assert!(OY_SETUP_SKILL.contains("oy doctor --check"));
        assert!(OY_SETUP_SKILL.contains("oy-persona.md"));
        assert!(OY_SETUP_SKILL.contains(".agents/skills"));
        assert!(OY_SETUP_SKILL.contains("untrusted"));
    }

    #[test]
    fn persona_is_autonomous_without_permission_overrides() {
        assert!(OY_PERSONA.contains("mode: primary"));
        assert!(!OY_PERSONA.contains("permissions:"));
        assert!(OY_PERSONA.contains("carry the task through"));
        assert!(OY_PERSONA.contains("focused, verified commits at natural checkpoints"));
        assert!(OY_PERSONA.contains("Never discard or commit unrelated changes"));
        assert!(OY_PERSONA.contains("push, force-push, or create tags unless explicitly asked"));
        assert!(OY_PERSONA.contains("tokei --compact --sort code"));
        assert!(OY_PERSONA.contains("ctags --options=NONE --output-format=json"));
        assert!(OY_PERSONA.contains("oy doctor --install-missing"));
        assert!(OY_PERSONA.contains("The host agent and the user own permissions"));
    }
}
