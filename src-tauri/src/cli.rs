//! Headless CLI / diagnostic modes, parsed from argv before the tray is built.
//! Useful for support and CI.

use anyhow::Result;
use chrono::Utc;
use std::collections::HashSet;

use crate::format;
use crate::profile_store::ProfileStore;
use crate::source::{self, secret_key};
use crate::{engine, secrets};

/// Which mode (if any) was requested on the command line.
pub enum CliMode {
    Check,
    PrintPaths,
    Screenshot,
    /// No recognized flag — run the normal tray app.
    Tray,
}

pub fn parse(args: &[String]) -> CliMode {
    for a in args {
        match a.as_str() {
            "--check" => return CliMode::Check,
            "--print-paths" => return CliMode::PrintPaths,
            "--screenshot" => return CliMode::Screenshot,
            _ => {}
        }
    }
    CliMode::Tray
}

/// Build a `ProfileStore` for headless use.
fn store() -> Result<ProfileStore> {
    Ok(ProfileStore::new(secrets::build()?, source::build_all()?))
}

/// `--print-paths`: resolved dirs + each provider's detected credential location.
pub fn print_paths() -> Result<()> {
    println!("PitStopX paths");
    println!("  config dir : {}", crate::paths::config_dir()?.display());
    println!("  data dir   : {}", crate::paths::data_dir()?.display());
    println!("  log dir    : {}", crate::paths::log_dir()?.display());
    println!(
        "  profiles   : {}",
        crate::paths::profiles_file()?.display()
    );

    let store = store()?;
    for source in store.sources() {
        println!(
            "  {:<9} : {}",
            source.provider().display_name(),
            source.describe()
        );
    }
    Ok(())
}

/// `--check`: capture, refresh stale tokens, fetch usage, print a summary.
pub async fn check() -> Result<()> {
    let store = store()?;
    let now_ms = Utc::now().timestamp_millis();

    // Snapshot the current account(s) so the saved copies are current.
    if let Err(e) = store.capture_current().await {
        eprintln!("warning: capture_current failed: {e}");
    }

    let profiles = store.load()?;
    let active = store.active_accounts().await;
    let active_keys: HashSet<String> = active.iter().map(|(p, e)| secret_key(*p, e)).collect();

    if profiles.is_empty() {
        println!("No saved accounts. Log in with `claude` or `codex` first.");
        return Ok(());
    }

    for (provider, email) in &active {
        println!("Active {}: {email}", provider.display_name());
    }
    println!();

    for p in &profiles {
        let is_active = active_keys.contains(&p.key());
        let marker = if is_active { "*" } else { " " };
        print!(
            "{marker} {} <{}> [{}]",
            p.email,
            p.provider.display_name(),
            p.plan_label()
        );

        let blob = match store.blob_for(p.provider, &p.email, is_active).await {
            Ok(Some(b)) => b,
            Ok(None) => {
                println!("  — no credentials");
                continue;
            }
            Err(e) => {
                println!("  — credential error: {e}");
                continue;
            }
        };

        match engine::fetch(p.provider, &blob, is_active, now_ms).await {
            Ok(fetched) => {
                let five = format::percent(fetched.report.five_hour.utilization);
                let week = format::percent(fetched.report.seven_day.utilization);
                println!("  5-hour {five} · weekly {week}");
                // Persist a refreshed inactive blob so the next run is cheap. The
                // old token is already revoked server-side, so a failed write
                // leaves the saved login broken — say so loudly.
                if let Some(new_blob) = fetched.refreshed_blob {
                    if let Err(e) = store
                        .store_refreshed_blob(p.provider, &p.email, &new_blob)
                        .await
                    {
                        println!("  ⚠ could not save refreshed login ({e}) — re-add this account");
                    }
                }
            }
            Err(e) => println!("  — usage error: {e}"),
        }
    }
    Ok(())
}
