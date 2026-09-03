//! Standard agent-skills setup and the canonical oy skill assets.

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
