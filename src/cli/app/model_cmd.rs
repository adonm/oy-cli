use anyhow::{Result, bail};
use clap::Args;

use crate::config;
use crate::model;

const MODEL_LIST_LIMIT: usize = 30;

#[derive(Debug, Args, Clone)]
pub(super) struct ModelArgs {
    #[arg(
        value_name = "MODEL",
        help = "Model id or routing shim selection from `oy model`, e.g. copilot::<model-id>"
    )]
    model: Option<String>,
}

pub(super) async fn model_command(args: ModelArgs) -> Result<i32> {
    if let Some(model_spec) = args
        .model
        .as_deref()
        .filter(|value| is_exact_model_spec(value))
    {
        let normalized = model::canonical_model_spec(model_spec);
        config::save_model_config(&normalized)?;
        if crate::ui::is_json() {
            print_saved_model_json(&normalized)?;
        } else {
            print_saved_model(&normalized);
        }
        return Ok(0);
    }

    if args.model.is_none() && !crate::ui::is_json() {
        let current = model::resolve_model(None).ok();
        match crate::chat::choose_recent_model(current.as_deref(), &config::recent_models()?)? {
            crate::chat::RecentModelChoice::Selected(model_spec) => {
                config::save_model_config(&model_spec)?;
                print_saved_model(&model_spec);
                return Ok(0);
            }
            crate::chat::RecentModelChoice::Clear => {
                config::clear_recent_models()?;
                crate::ui::success("cleared recent model history");
                return Ok(0);
            }
            crate::chat::RecentModelChoice::Cancelled => return Ok(0),
            crate::chat::RecentModelChoice::Inspect => {}
        }
    }

    let listing = model::inspect_models().await?;
    if let Some(model_spec) = args.model {
        let normalized = resolve_model_choice(&listing, &model_spec)?;
        config::save_model_config(&normalized)?;
        if crate::ui::is_json() {
            print_model_json(&listing, Some(&normalized))?;
        } else {
            print_saved_model(&normalized);
        }
        return Ok(0);
    }
    if crate::ui::is_json() {
        print_model_json(&listing, None)?;
        return Ok(0);
    }
    print_model_listing(&listing);
    if config::can_prompt()
        && !listing.all_models.is_empty()
        && let Some(chosen) = crate::chat::choose_model_with_initial_list(
            listing.current.as_deref(),
            &listing.all_models,
            false,
        )?
    {
        config::save_model_config(&chosen)?;
        print_saved_model(&chosen);
    }
    Ok(0)
}

pub(super) fn is_exact_model_spec(value: &str) -> bool {
    let value = value.trim();
    value.contains("::") || value.contains('/') || value.contains(':') || value.contains('.')
}

fn print_saved_model_json(saved: &str) -> Result<()> {
    let payload = serde_json::json!({ "saved": saved });
    crate::ui::line(serde_json::to_string_pretty(&payload)?);
    Ok(())
}

fn print_model_json(listing: &model::ModelListing, saved: Option<&str>) -> Result<()> {
    let payload = serde_json::json!({
        "current": listing.current,
        "current_shim": listing.current_shim,
        "saved": saved,
        "recent_models": config::recent_models()?,
        "auth": listing.auth,
        "dynamic": listing.dynamic,
        "all_models": listing.all_models,
    });
    crate::ui::line(serde_json::to_string_pretty(&payload)?);
    Ok(())
}

fn print_model_listing(listing: &model::ModelListing) {
    crate::ui::section("Models");
    crate::ui::kv(
        "current",
        current_model_text(
            listing.current.as_deref().unwrap_or("<unset>"),
            listing.current_shim.as_deref(),
        ),
    );
    crate::ui::kv("selectable", listing.all_models.len());
    if let Ok(recent) = config::recent_models()
        && !recent.is_empty()
    {
        crate::ui::line("");
        crate::ui::section("Recent models");
        for model in recent {
            let marker = if listing.current.as_deref() == Some(model.as_str()) {
                "*"
            } else {
                " "
            };
            crate::ui::line(format_args!("    {marker} {model}"));
        }
    }
    if !listing.auth.is_empty() {
        crate::ui::line("");
        crate::ui::section("Auth / shims");
        for item in &listing.auth {
            let env_var = item.env_var.as_deref().unwrap_or("-");
            let active = if listing.current_shim.as_deref() == Some(item.adapter.as_str()) {
                " *"
            } else {
                ""
            };
            crate::ui::line(format_args!(
                "  {}{}  {} ({})",
                item.adapter, active, env_var, item.source
            ));
            crate::ui::line(format_args!("    {}", item.detail));
        }
    }

    crate::ui::line("");
    crate::ui::section("Introspected endpoint models");
    if listing.dynamic.is_empty() {
        crate::ui::line("  none found from configured OpenAI-compatible endpoints");
    } else {
        for item in &listing.dynamic {
            match item {
                model::AdapterModels::Available {
                    adapter,
                    source,
                    count,
                    models,
                } => {
                    crate::ui::line(format_args!("  {adapter}  {count} models via {source}"));
                    for model_name in models.iter().take(MODEL_LIST_LIMIT) {
                        let marker = if listing.current.as_deref() == Some(model_name.as_str()) {
                            "*"
                        } else {
                            " "
                        };
                        crate::ui::line(format_args!("    {marker} {model_name}"));
                    }
                    if models.len() > MODEL_LIST_LIMIT {
                        crate::ui::line(format_args!(
                            "    … {} more; use `oy model <filter>` or interactive selection",
                            models.len() - MODEL_LIST_LIMIT
                        ));
                    }
                }
                model::AdapterModels::Failed {
                    adapter,
                    source,
                    error,
                } => {
                    crate::ui::line(format_args!("  {adapter}  failed via {source}"));
                    crate::ui::line(format_args!(
                        "    {}",
                        crate::ui::truncate_chars(error, 140)
                    ));
                }
            }
        }
    }
}

fn current_model_text(model_spec: &str, shim: Option<&str>) -> String {
    match shim.filter(|value| !value.is_empty()) {
        Some(shim) => format!("{model_spec} (shim: {shim})"),
        None => model_spec.to_string(),
    }
}

fn print_saved_model(selection: &str) {
    let saved = config::saved_model_config_from_selection(selection);
    crate::ui::success(format_args!(
        "saved model {}",
        saved.model.as_deref().unwrap_or(selection)
    ));
    if let Some(shim) = saved.shim {
        crate::ui::kv("shim", shim);
    }
}

fn resolve_model_choice(listing: &model::ModelListing, query: &str) -> Result<String> {
    let normalized = model::canonical_model_spec(query);
    if listing.all_models.iter().any(|item| item == &normalized) {
        return Ok(normalized);
    }
    if !config::can_prompt() {
        bail!(
            "No exact model match for `{}`. Re-run in a TTY to choose interactively.",
            query
        );
    }
    let matches = listing
        .all_models
        .iter()
        .filter(|item| {
            item.to_ascii_lowercase()
                .contains(&query.to_ascii_lowercase())
        })
        .cloned()
        .collect::<Vec<_>>();
    if matches.is_empty() {
        bail!("No matching model for `{}`", query);
    }
    crate::chat::choose_model(listing.current.as_deref(), &matches)
        .map(|value| value.unwrap_or(normalized))
}
