use std::env;
use std::io::IsTerminal as _;

#[derive(Debug, Clone, Copy)]
pub struct ContextConfig {
    pub limit_tokens: usize,
    pub output_reserve_tokens: usize,
    pub safety_reserve_tokens: usize,
    pub trigger_ratio: f64,
    pub recent_messages: usize,
    pub tool_output_tokens: usize,
    pub summary_tokens: usize,
}

impl ContextConfig {
    pub fn input_budget_tokens(self) -> usize {
        self.limit_tokens
            .saturating_sub(self.output_reserve_tokens)
            .saturating_sub(self.safety_reserve_tokens)
            .max(1)
    }

    pub fn trigger_tokens(self) -> usize {
        ((self.input_budget_tokens() as f64) * self.trigger_ratio) as usize
    }
}

pub fn non_interactive() -> bool {
    env_flag("OY_NON_INTERACTIVE", false)
}

pub fn can_prompt() -> bool {
    std::io::stdin().is_terminal() && !non_interactive()
}

pub fn context_config() -> ContextConfig {
    let limit_tokens = parse_usize_env("OY_CONTEXT_LIMIT", 128_000).max(1_000);
    let output_reserve_tokens = parse_usize_env("OY_CONTEXT_OUTPUT_RESERVE", 12_000);
    let safety_reserve_tokens = parse_usize_env("OY_CONTEXT_SAFETY_RESERVE", 4_000);
    ContextConfig {
        limit_tokens,
        output_reserve_tokens,
        safety_reserve_tokens,
        trigger_ratio: parse_f64_env("OY_COMPACT_TRIGGER", 0.80).clamp(0.10, 1.0),
        recent_messages: parse_usize_env("OY_COMPACT_RECENT_MESSAGES", 16).max(1),
        tool_output_tokens: parse_usize_env("OY_COMPACT_TOOL_OUTPUT_TOKENS", 4_000).max(256),
        summary_tokens: parse_usize_env("OY_COMPACT_SUMMARY_TOKENS", 8_000).max(512),
    }
}

pub fn max_bash_cmd_bytes() -> usize {
    env::var("OY_MAX_BASH_CMD_BYTES")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(16 * 1024)
}

pub fn max_tool_rounds(default: usize) -> usize {
    parse_tool_round_limit(env::var("OY_MAX_TOOL_ROUNDS").ok().as_deref(), default)
}

pub(super) fn parse_tool_round_limit(value: Option<&str>, default: usize) -> usize {
    let Some(value) = value.map(str::trim).filter(|value| !value.is_empty()) else {
        return default.max(1);
    };
    if matches!(
        value.to_ascii_lowercase().as_str(),
        "unlimited" | "none" | "off"
    ) {
        return usize::MAX / 4;
    }
    match value.parse::<usize>() {
        Ok(0) => usize::MAX / 4,
        Ok(max) => max,
        Err(_) => default.max(1),
    }
}

fn parse_usize_env(name: &str, default: usize) -> usize {
    env::var(name)
        .ok()
        .and_then(|v| v.trim().parse::<usize>().ok())
        .unwrap_or(default)
}

fn parse_f64_env(name: &str, default: f64) -> f64 {
    env::var(name)
        .ok()
        .and_then(|v| v.trim().parse::<f64>().ok())
        .filter(|v| v.is_finite())
        .unwrap_or(default)
}

fn env_flag(name: &str, default: bool) -> bool {
    match env::var(name) {
        Ok(value) => match value.trim().to_ascii_lowercase().as_str() {
            "1" | "true" | "yes" | "on" => true,
            "0" | "false" | "no" | "off" => false,
            _ => default,
        },
        Err(_) => default,
    }
}
