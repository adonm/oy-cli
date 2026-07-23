//! Cursor rule, subagent, and skill setup.

mod setup;

pub(crate) use setup::setup_command;

const OY_AGENT: &str = include_str!("../assets/cursor/agents/oy.md");
const OY_RULE: &str = include_str!("../assets/cursor/rules/oy.mdc");
const OY_AUDIT_SKILL: &str = include_str!("../assets/cursor/skills/oy-audit/SKILL.md");
const OY_REVIEW_SKILL: &str = include_str!("../assets/cursor/skills/oy-review/SKILL.md");
const OY_ENHANCE_SKILL: &str = include_str!("../assets/cursor/skills/oy-enhance/SKILL.md");

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cursor_assets_use_native_formats() {
        assert!(OY_AGENT.contains("name: oy"));
        assert!(OY_AGENT.contains("model: inherit"));
        assert!(!OY_AGENT.contains("mode: primary"));
        assert!(OY_RULE.contains("alwaysApply: true"));
        assert!(OY_RULE.contains("Cursor and the user own permissions"));
        assert_eq!(markdown_body(OY_RULE), markdown_body(OY_AGENT));

        for (name, skill) in [
            ("oy-audit", OY_AUDIT_SKILL),
            ("oy-review", OY_REVIEW_SKILL),
            ("oy-enhance", OY_ENHANCE_SKILL),
        ] {
            assert!(skill.contains(&format!("name: {name}")));
            assert!(!skill.contains("slash:"));
            assert!(!skill.contains("opencode/autoinvoke"));
            assert!(skill.contains("current Cursor permissions"));
        }
    }

    #[test]
    fn cursor_review_workflows_keep_the_deterministic_protocol() {
        for skill in [OY_AUDIT_SKILL, OY_REVIEW_SKILL] {
            assert!(skill.contains("Protocol:"));
            assert!(skill.contains("`[]`"));
            assert!(skill.contains("untrusted"));
            assert!(skill.contains("continue paging with the native read offset"));
        }
        assert!(OY_ENHANCE_SKILL.contains("Fix one finding per pass"));
    }

    fn markdown_body(source: &str) -> &str {
        source
            .splitn(3, "---")
            .nth(2)
            .expect("asset frontmatter")
            .trim()
    }
}
