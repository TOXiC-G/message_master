use semver::Version;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize)]
pub struct UpdateInfo {
    pub current_version: String,
    pub latest_version: String,
    pub release_url: String,
}

#[derive(Deserialize)]
struct ManifestOrRelease {
    tag_name: Option<String>,
    version: Option<String>,
    html_url: Option<String>,
    url: Option<String>,
}

const RELEASE_URL: &str = "https://api.github.com/repos/TOXiC-G/message_master/releases/latest";

/// Unobtrusively checks the GitHub releases endpoint with a short timeout.
/// Returns `Some(UpdateInfo)` only if a strictly newer semver release exists; silent otherwise.
pub async fn check_for_update() -> Option<UpdateInfo> {
    let url = RELEASE_URL;

    let current_str = env!("CARGO_PKG_VERSION");
    let current_ver = Version::parse(current_str).ok()?;

    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(4))
        .user_agent("MessageMaster-Updater/1.0")
        .build()
        .ok()?;

    let res = client.get(url).send().await.ok()?;
    if !res.status().is_success() {
        return None;
    }

    let data: ManifestOrRelease = res.json().await.ok()?;
    let raw_ver = data.version.or(data.tag_name)?;
    let clean_ver = raw_ver.trim().trim_start_matches(['v', 'V']);
    let latest_ver = Version::parse(clean_ver).ok()?;

    if latest_ver > current_ver {
        Some(UpdateInfo {
            current_version: current_str.to_string(),
            latest_version: latest_ver.to_string(),
            release_url: data.html_url.or(data.url).unwrap_or_else(|| url.to_string()),
        })
    } else {
        None
    }
}
