use crate::cargo::*;
use crate::cli::{Cli, Commands};
use crate::doc::DocIndex;
use crate::types::{ItemKind, SearchResult};
use std::collections::HashMap;
use tracing::{error, info, info_span, warn};

pub async fn run(cli: Cli) -> Result<(), Box<dyn std::error::Error>> {
    match cli.command {
        Commands::Search {
            query,
            crate_override,
            kind,
            limit,
        } => {
            let crate_list = resolve_crates(crate_override.clone())?;
            let (loaded_crates, failed_crates) = load_multiple_crates(&crate_list).await;

            if loaded_crates.is_empty() {
                error!("No crates could be loaded");
                return Err("All crates failed to load".into());
            }

            if crate_override.is_some() {
                info!("Searching specified crates: {}", loaded_crates
                    .iter()
                    .map(|(name, _)| name.as_str())
                    .collect::<Vec<_>>()
                    .join(", "));
            } else {
                info!("Auto-detected {} crate(s) from Cargo.toml: {}",
                    loaded_crates.len(),
                    loaded_crates
                        .iter()
                        .map(|(name, _)| name.as_str())
                        .collect::<Vec<_>>()
                        .join(", "));
            }
            if !failed_crates.is_empty() {
                info!("{} crate(s) failed to load", failed_crates.len());
            }

            let kind_filter = kind.as_ref().and_then(|k| parse_item_kind(k));

            let results = search_multiple_crates(&loaded_crates, &query, kind_filter);

            println!("Found {} items matching '{}':", results.len(), query);

            let is_multi_crate = loaded_crates.len() > 1;
            for result in results.iter().take(limit) {
                if is_multi_crate {
                    if let Some(source_crate) = &result.source_crate {
                        println!(
                            "{} {} ({}) [crate: {}]",
                            result.kind, result.name, result.path, source_crate
                        );
                    } else {
                        println!("{} {} ({})", result.kind, result.name, result.path);
                    }
                } else {
                    println!("{} {} ({})", result.kind, result.name, result.path);
                }
            }

            if results.len() > limit {
                println!("... and {} more results", results.len() - limit);
            }
        }

        Commands::Paths {
            type_name,
            crate_override,
        } => {
            let crate_list = resolve_crates(crate_override.clone())?;
            let (loaded_crates, failed_crates) = load_multiple_crates(&crate_list).await;

            if loaded_crates.is_empty() {
                error!("No crates could be loaded");
                return Err("All crates failed to load".into());
            }

            if crate_override.is_some() {
                info!("Searching specified crates: {}", loaded_crates
                    .iter()
                    .map(|(name, _)| name.as_str())
                    .collect::<Vec<_>>()
                    .join(", "));
            } else {
                info!("Auto-detected {} crate(s) from Cargo.toml: {}",
                    loaded_crates.len(),
                    loaded_crates
                        .iter()
                        .map(|(name, _)| name.as_str())
                        .collect::<Vec<_>>()
                        .join(", "));
            }
            if !failed_crates.is_empty() {
                info!("{} crate(s) failed to load", failed_crates.len());
            }

            let mut found_any = false;
            let is_multi_crate = loaded_crates.len() > 1;

            for (crate_name, doc) in &loaded_crates {
                let paths = doc.find_public_path(&type_name);

                if !paths.is_empty() {
                    found_any = true;

                    if is_multi_crate {
                        println!("In crate '{}':", crate_name);
                    }

                    for (idx, path) in paths.iter().enumerate() {
                        if idx == 0 {
                            println!("  ✓ {} (recommended)", path);
                        } else {
                            println!("    {}", path);
                        }
                    }
                    println!();
                }
            }

            if !found_any {
                println!("No public paths found for '{}'", type_name);
                println!();
                println!("This could mean:");
                println!("  • The type doesn't exist in these crates");
                println!("  • The type is not publicly exported");
                println!("  • You need to check the exact name (case-sensitive)");
            } else if is_multi_crate {
                println!("Tip: The first path in each crate is usually the most canonical/preferred.");
            }
        }

        Commands::Signature {
            function_name,
            crate_override,
            limit,
        } => {
            let crate_list = resolve_crates(crate_override.clone())?;
            let (loaded_crates, failed_crates) = load_multiple_crates(&crate_list).await;

            if loaded_crates.is_empty() {
                error!("No crates could be loaded");
                return Err("All crates failed to load".into());
            }

            if crate_override.is_some() {
                info!("Searching specified crates: {}", loaded_crates
                    .iter()
                    .map(|(name, _)| name.as_str())
                    .collect::<Vec<_>>()
                    .join(", "));
            } else {
                info!("Auto-detected {} crate(s) from Cargo.toml: {}",
                    loaded_crates.len(),
                    loaded_crates
                        .iter()
                        .map(|(name, _)| name.as_str())
                        .collect::<Vec<_>>()
                        .join(", "));
            }
            if !failed_crates.is_empty() {
                info!("{} crate(s) failed to load", failed_crates.len());
            }

            let results = search_multiple_crates(&loaded_crates, &function_name, Some(ItemKind::Function));

            if results.is_empty() {
                println!("No functions found matching '{}'", function_name);
            } else {
                println!("Found {} function(s) matching '{}':", results.len(), function_name);
                println!();

                let is_multi_crate = loaded_crates.len() > 1;
                let mut count = 0;

                for result in results.iter().take(limit) {
                    count += 1;
                    println!("{}. {}", count, result.name);
                    println!("   Path: {}", result.path);

                    if is_multi_crate
                        && let Some(source_crate) = &result.source_crate {
                            println!("   Crate: {}", source_crate);
                        }

                    let doc = if let Some(source_crate) = &result.source_crate {
                        loaded_crates.iter()
                            .find(|(name, _)| name == source_crate)
                            .map(|(_, doc)| doc)
                    } else {
                        None
                    };

                    if let Some(doc) = doc
                        && let Some(id) = &result.id {
                            if let Some(item) = doc.get_item(id) {
                                if let Some(sig) = doc.format_function_signature(item) {
                                    println!("   Signature: {}", sig);
                                }

                                if let Some(docs) = &item.docs {
                                    let preview: Vec<_> = docs.lines().take(2).collect();
                                    if !preview.is_empty() {
                                        println!("   Docs:");
                                        for line in preview {
                                            println!("     {}", line);
                                        }
                                    }
                                }
                            } else if let Some(crate_name) = &result.crate_name {
                                println!("   From: {} (external - signature details not available)", crate_name);
                            } else {
                                println!("   (external - signature details not available)");
                            }
                        }
                    println!();
                }

                if results.len() > limit {
                    println!("... and {} more results", results.len() - limit);
                }
            }
        }
    }

    Ok(())
}

pub fn resolve_crates(override_crates: Option<String>) -> Result<Vec<String>, Box<dyn std::error::Error>> {
    if let Some(crates_str) = override_crates {
        Ok(parse_crate_list(&crates_str))
    } else {
        let cargo_toml = find_cargo_toml().ok_or("Could not find Cargo.toml in current directory or any parent directory")?;

        let crates = extract_dependencies(&cargo_toml)?;

        if crates.is_empty() {
            warn!("No dependencies found in Cargo.toml. You can specify crates manually with: --crate <crate1>,<crate2>");
        }

        Ok(crates)
    }
}

pub fn parse_crate_list(input: &str) -> Vec<String> {
    input
        .split(',')
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .collect()
}

pub async fn load_multiple_crates(crate_names: &[String]) -> (Vec<(String, DocIndex)>, Vec<String>) {
    let version_map = match get_resolved_versions() {
        Ok(map) => map,
        Err(e) => {
            warn!("Failed to get cargo metadata: {}, continuing without version resolution", e);
            HashMap::new()
        }
    };

    let mut tasks = Vec::new();

    for crate_name in crate_names {
        let crate_name = crate_name.clone();
        let version = version_map.get(&crate_name).cloned();

        let task = tokio::task::spawn_blocking(move || {
            let target = if let Some(ref v) = version {
                format!("{}@{}", crate_name, v)
            } else {
                crate_name.clone()
            };

            let span = info_span!("get_docs", target = %target);
            let _enter = span.enter();

            match get_docs(&crate_name, version.as_deref()) {
                Ok(doc_index) => Ok((crate_name, doc_index)),
                Err(e) => {
                    warn!("Failed to load crate '{}': {}", crate_name, e);
                    Err(crate_name)
                }
            }
        });

        tasks.push(task);
    }

    let mut successful = Vec::new();
    let mut failed = Vec::new();

    for task in tasks {
        match task.await {
            Ok(Ok((name, doc))) => successful.push((name, doc)),
            Ok(Err(name)) => failed.push(name),
            Err(e) => error!("Task join error: {}", e),
        }
    }

    (successful, failed)
}

pub fn search_multiple_crates(
    crates: &[(String, DocIndex)],
    query: &str,
    kind_filter: Option<ItemKind>,
) -> Vec<SearchResult> {
    let mut all_results = Vec::new();

    for (crate_name, doc_index) in crates {
        let mut results = doc_index.search_with_filter(query, kind_filter);

        for result in &mut results {
            result.source_crate = Some(crate_name.clone());
        }

        all_results.extend(results);
    }

    all_results.sort_by(|a, b| {
        b.relevance
            .cmp(&a.relevance)
            .then_with(|| a.name.cmp(&b.name))
    });

    all_results
}

pub fn parse_item_kind(s: &str) -> Option<ItemKind> {
    match s.to_lowercase().as_str() {
        "module" | "mod" => Some(ItemKind::Module),
        "struct" => Some(ItemKind::Struct),
        "enum" => Some(ItemKind::Enum),
        "function" | "fn" => Some(ItemKind::Function),
        "trait" => Some(ItemKind::Trait),
        "type" | "typealias" => Some(ItemKind::TypeAlias),
        "const" | "constant" => Some(ItemKind::Constant),
        "static" => Some(ItemKind::Static),
        _ => None,
    }
}
