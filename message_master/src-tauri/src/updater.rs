use semver::Version;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

const RELEASE_URL: &str = "https://api.github.com/repos/TOXiC-G/message_master/releases/latest";

#[derive(Debug, Clone, Serialize)]
pub struct UpdateInfo {
    pub current_version: String,
    pub latest_version: String,
    pub release_url: String,
    pub installer_path: Option<String>,
}

#[derive(Deserialize)]
struct ReleaseAsset {
    name: String,
    browser_download_url: String,
}

#[derive(Deserialize)]
struct ManifestOrRelease {
    tag_name: Option<String>,
    version: Option<String>,
    html_url: Option<String>,
    url: Option<String>,
    assets: Option<Vec<ReleaseAsset>>,
}

/// Checks GitHub Releases for a newer version. If a Windows setup `.exe` asset is found,
/// downloads it silently in the background to `%TEMP%` so the user can 1-click restart & install.
pub async fn check_and_download_update() -> Option<UpdateInfo> {
    let current_str = env!("CARGO_PKG_VERSION");
    let current_ver = Version::parse(current_str).ok()?;

    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(6))
        .user_agent("MessageMaster-Updater/1.0")
        .build()
        .ok()?;

    let res = client.get(RELEASE_URL).send().await.ok()?;
    if !res.status().is_success() {
        return None;
    }

    let data: ManifestOrRelease = res.json().await.ok()?;
    let raw_ver = data.version.or(data.tag_name)?;
    let clean_ver = raw_ver.trim().trim_start_matches(['v', 'V']);
    let latest_ver = Version::parse(clean_ver).ok()?;

    if latest_ver <= current_ver {
        return None;
    }

    let release_url = data
        .html_url
        .or(data.url)
        .unwrap_or_else(|| RELEASE_URL.to_string());

    // Look for NSIS setup .exe asset
    let setup_url = data
        .assets
        .as_ref()
        .and_then(|assets| {
            assets
                .iter()
                .find(|a| a.name.to_lowercase().ends_with("-setup.exe") || a.name.to_lowercase().ends_with(".exe"))
        })
        .map(|a| a.browser_download_url.clone());

    let mut installer_path: Option<String> = None;
    if let Some(dl_url) = setup_url {
        let dest: PathBuf = std::env::temp_dir().join(format!("MessageMaster_{latest_ver}_setup.exe"));
        if dest.exists() {
            installer_path = Some(dest.to_string_lossy().to_string());
        } else if let Ok(dl_client) = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(120))
            .user_agent("MessageMaster-Updater/1.0")
            .build()
        {
            if let Ok(resp) = dl_client.get(&dl_url).send().await {
                if resp.status().is_success() {
                    if let Ok(bytes) = resp.bytes().await {
                        if tokio::fs::write(&dest, &bytes).await.is_ok() {
                            installer_path = Some(dest.to_string_lossy().to_string());
                        }
                    }
                }
            }
        }
    }

    Some(UpdateInfo {
        current_version: current_str.to_string(),
        latest_version: latest_ver.to_string(),
        release_url,
        installer_path,
    })
}

/// Launches the downloaded NSIS installer silently (`/S`), waits for it to finish, and relaunches MessageMaster.
pub fn run_installer_and_restart(installer_path: &str) -> Result<(), String> {
    let current_exe = std::env::current_exe()
        .map(|p| p.to_string_lossy().to_string())
        .unwrap_or_default();

    let esc_installer = installer_path.replace('\'', "''");
    let esc_exe = current_exe.replace('\'', "''");

    let script = if esc_exe.is_empty() {
        format!("Start-Sleep -Milliseconds 800; Start-Process -FilePath '{esc_installer}' -ArgumentList '/S' -Wait")
    } else {
        format!(
            "Start-Sleep -Milliseconds 800; Start-Process -FilePath '{esc_installer}' -ArgumentList '/S' -Wait; Start-Process -FilePath '{esc_exe}'"
        )
    };

    std::process::Command::new("powershell")
        .args(["-NoProfile", "-WindowStyle", "Hidden", "-Command", &script])
        .spawn()
        .map_err(|e| format!("Failed to launch updater: {e}"))?;

    Ok(())
}
