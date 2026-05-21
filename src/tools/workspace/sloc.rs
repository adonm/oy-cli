use anyhow::Result;
use serde_json::Value;
use tokei::{Config as TokeiConfig, Languages as TokeiLanguages, Sort as TokeiSort};

use super::super::ToolContext;
use super::super::args::{ExcludeArg, SlocArgs};
use super::output::SlocOutput;
use super::paths::resolve_existing_paths;

pub(crate) fn tool_sloc(ctx: &ToolContext, args: SlocArgs) -> Result<Value> {
    let targets = resolve_existing_paths(ctx, &args.path)?;
    let exclude = args
        .exclude
        .as_ref()
        .map(ExcludeArg::patterns)
        .unwrap_or_default();
    let targets = targets
        .iter()
        .map(|path| path.to_string_lossy().to_string())
        .collect::<Vec<_>>();
    let target_refs = targets.iter().map(String::as_str).collect::<Vec<_>>();
    let excluded = exclude.iter().map(String::as_str).collect::<Vec<_>>();

    let config = TokeiConfig {
        hidden: Some(false),
        no_ignore: Some(false),
        no_ignore_parent: Some(false),
        no_ignore_dot: Some(false),
        no_ignore_vcs: Some(false),
        ..TokeiConfig::default()
    };
    let mut languages = TokeiLanguages::new();
    languages.get_statistics(&target_refs, &excluded, &config);
    sort_tokei_reports(&mut languages);

    let mut output = serde_json::to_value(&languages)?;
    if let Value::Object(ref mut map) = output {
        map.insert(
            "Total".to_string(),
            serde_json::to_value(languages.total())?,
        );
    }

    Ok(serde_json::to_value(SlocOutput {
        path: args.path,
        format: "tokei-json",
        output,
        exclude: (!exclude.is_empty()).then_some(exclude),
    })?)
}

fn sort_tokei_reports(languages: &mut TokeiLanguages) {
    for language in languages.values_mut() {
        language.sort_by(TokeiSort::Code);
    }
}
