//! Headless CLI / diagnostic modes, parsed from argv before the tray is built.
//! Useful for support and CI.

use anyhow::Result;
use chrono::Utc;

use crate::claude_source;
use crate::credentials::CredentialBlob;
use crate::format;
use crate::profile_store::ProfileStore;
use crate::secrets;
use crate::usage_api;

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
    Ok(ProfileStore::new(
        secrets::build()?,
        claude_source::build()?,
    ))
}

/// `--print-paths`: resolved config/data/log dirs + detected Claude Code creds.
pub fn print_paths() -> Result<()> {
    println!("PitStopX paths");
    println!("  config dir : {}", crate::paths::config_dir()?.display());
    println!("  data dir   : {}", crate::paths::data_dir()?.display());
    println!("  log dir    : {}", crate::paths::log_dir()?.display());
    println!(
        "  profiles   : {}",
        crate::paths::profiles_file()?.display()
    );

    let source = claude_source::build()?;
    println!("  identity   : {}", source.identity_path().display());
    println!("  live creds : {}", source.describe());
    Ok(())
}

/// `--check`: capture, refresh stale tokens, fetch usage, print a summary.
pub async fn check() -> Result<()> {
    let store = store()?;
    let now_ms = Utc::now().timestamp_millis();

    // Snapshot the current account so the saved copy is current.
    if let Err(e) = store.capture_current().await {
        eprintln!("warning: capture_current failed: {e}");
    }

    let profiles = store.load()?;
    let active = store.active_email()?;
    if profiles.is_empty() {
        println!("No saved accounts. Log in with `claude` first.");
        return Ok(());
    }

    println!("Active account: {}", active.as_deref().unwrap_or("(none)"));
    println!();

    for p in &profiles {
        let is_active = active.as_deref() == Some(p.email.as_str());
        let marker = if is_active { "*" } else { " " };
        print!(
            "{marker} {} <{}> [{}]",
            p.email,
            p.provider.display_name(),
            p.plan_label()
        );

        let access = match credentials_for(&store, &p.email, is_active, now_ms).await {
            Ok(token) => token,
            Err(e) => {
                println!("  — credential error: {e}");
                continue;
            }
        };

        match usage_api::fetch_usage(&access).await {
            Ok(report) => {
                let five = format::percent(report.five_hour.utilization);
                let week = format::percent(report.seven_day.utilization);
                println!("  5-hour {five} · weekly {week}");
            }
            Err(e) => println!("  — usage error: {e}"),
        }
    }
    Ok(())
}

async fn credentials_for(
    store: &ProfileStore,
    email: &str,
    is_active: bool,
    now_ms: i64,
) -> Result<String> {
    let raw = store
        .blob_for(email, is_active)
        .await?
        .ok_or_else(|| anyhow::anyhow!("no blob"))?;
    let blob = CredentialBlob::parse(&raw)?;
    let creds = blob.credentials()?;
    if is_active || !creds.is_expired(now_ms) {
        return Ok(creds.access_token);
    }
    let refresh = creds
        .refresh_token
        .as_deref()
        .ok_or_else(|| anyhow::anyhow!("expired, no refresh token"))?;
    let fresh = usage_api::refresh_token(refresh, now_ms).await?;
    let patched = blob.patching(&fresh)?;
    store
        .store_refreshed_blob(email, is_active, &patched.to_bytes())
        .await?;
    Ok(fresh.access_token)
}
