use crate::config::AppConfig;
use reqwest::multipart::{Form, Part};
use serde::{Deserialize, Serialize};
use std::path::Path;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DriveFolder {
    pub id: String,
    pub name: String,
}

#[derive(Deserialize)]
struct TokenResponse {
    access_token: String,
    refresh_token: Option<String>,
}

#[derive(Deserialize)]
struct DriveFileResponse {
    id: String,
    #[serde(rename = "webViewLink")]
    web_view_link: Option<String>,
}

#[derive(Deserialize)]
struct DriveListResponse {
    files: Vec<DriveFolder>,
}

/// Runs Google OAuth2 Desktop loopback flow, opens the user's browser, and stores the refresh token.
pub async fn connect_oauth(cfg: &mut AppConfig) -> Result<String, String> {
    if cfg.oauth_client_id.trim().is_empty() || cfg.oauth_client_secret.trim().is_empty() {
        return Err("Please enter your Google OAuth Client ID and Client Secret in Settings first.".into());
    }

    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .map_err(|e| format!("Failed to bind local OAuth callback port: {e}"))?;
    let port = listener.local_addr().map_err(|e| e.to_string())?.port();
    let redirect_uri = format!("http://127.0.0.1:{port}");

    let auth_url = format!(
        "https://accounts.google.com/o/oauth2/v2/auth?client_id={}&redirect_uri={}&response_type=code&scope={}&access_type=offline&prompt=consent",
        urlencoding::encode(cfg.oauth_client_id.trim()),
        urlencoding::encode(&redirect_uri),
        urlencoding::encode("https://www.googleapis.com/auth/drive")
    );

    open_browser(&auth_url)?;

    let (mut socket, _) = tokio::time::timeout(
        std::time::Duration::from_secs(180),
        listener.accept(),
    )
    .await
    .map_err(|_| "Timed out waiting for Google sign-in.".to_string())?
    .map_err(|e| format!("OAuth callback failed: {e}"))?;

    let mut buf = [0u8; 4096];
    let n = socket.read(&mut buf).await.unwrap_or(0);
    let req_str = String::from_utf8_lossy(&buf[..n]);
    let first_line = req_str.lines().next().unwrap_or("");
    let code = extract_query_param(first_line, "code")
        .ok_or_else(|| "Google sign-in was cancelled or did not return an auth code.".to_string())?;

    let html = "HTTP/1.1 200 OK\r\nContent-Type: text/html\r\n\r\n<html><body style='font-family:sans-serif;text-align:center;padding:48px;background:#0f172a;color:#f8fafc;'><h2>Connected to Google Drive</h2><p>You can close this tab and return to MessageMaster.</p></body></html>";
    let _ = socket.write_all(html.as_bytes()).await;

    let client = reqwest::Client::new();
    let res = client
        .post("https://oauth2.googleapis.com/token")
        .form(&[
            ("code", code.as_str()),
            ("client_id", cfg.oauth_client_id.trim()),
            ("client_secret", cfg.oauth_client_secret.trim()),
            ("redirect_uri", redirect_uri.as_str()),
            ("grant_type", "authorization_code"),
        ])
        .send()
        .await
        .map_err(|e| format!("Token exchange failed: {e}"))?;

    if !res.status().is_success() {
        let body = res.text().await.unwrap_or_default();
        return Err(format!("Google OAuth error: {body}"));
    }

    let tok: TokenResponse = res.json().await.map_err(|e| e.to_string())?;
    if let Some(rt) = tok.refresh_token {
        cfg.refresh_token = Some(rt);
        cfg.save()?;
        Ok("Connected to Google Drive successfully.".into())
    } else {
        Err("Google did not return a refresh token. Please try again.".into())
    }
}

async fn get_access_token(cfg: &AppConfig) -> Result<String, String> {
    let rt = cfg
        .refresh_token
        .as_deref()
        .ok_or_else(|| "Google Drive is not connected. Open Settings -> Connect Google Drive.".to_string())?;

    let client = reqwest::Client::new();
    let res = client
        .post("https://oauth2.googleapis.com/token")
        .form(&[
            ("client_id", cfg.oauth_client_id.trim()),
            ("client_secret", cfg.oauth_client_secret.trim()),
            ("refresh_token", rt),
            ("grant_type", "refresh_token"),
        ])
        .send()
        .await
        .map_err(|e| format!("Failed to refresh Google token: {e}"))?;

    if !res.status().is_success() {
        let body = res.text().await.unwrap_or_default();
        return Err(format!("Google token refresh failed: {body}"));
    }
    let tok: TokenResponse = res.json().await.map_err(|e| e.to_string())?;
    Ok(tok.access_token)
}

/// Lists subfolders inside `parent_id` (defaults to `"root"`).
pub async fn list_folders(cfg: &AppConfig, parent_id: Option<&str>) -> Result<Vec<DriveFolder>, String> {
    let token = get_access_token(cfg).await?;
    let parent = parent_id.filter(|s| !s.trim().is_empty()).unwrap_or("root");
    let query = format!(
        "'{parent}' in parents and mimeType='application/vnd.google-apps.folder' and trashed=false"
    );

    let url = format!(
        "https://www.googleapis.com/drive/v3/files?q={}&fields=files(id,name)&orderBy=name&pageSize=100",
        urlencoding::encode(&query)
    );

    let client = reqwest::Client::new();
    let res = client
        .get(&url)
        .bearer_auth(&token)
        .send()
        .await
        .map_err(|e| format!("Failed to list Drive folders: {e}"))?;

    if !res.status().is_success() {
        let body = res.text().await.unwrap_or_default();
        return Err(format!("Drive folder list error: {body}"));
    }

    let list: DriveListResponse = res.json().await.map_err(|e| e.to_string())?;
    Ok(list.files)
}

/// Creates a new folder inside `parent_id` (defaults to `"root"`) and returns it.
pub async fn create_folder(
    cfg: &AppConfig,
    parent_id: Option<&str>,
    name: &str,
) -> Result<DriveFolder, String> {
    let trimmed = name.trim();
    if trimmed.is_empty() {
        return Err("Folder name cannot be empty.".into());
    }
    let token = get_access_token(cfg).await?;
    let parent = parent_id.filter(|s| !s.trim().is_empty()).unwrap_or("root");

    let body = serde_json::json!({
        "name": trimmed,
        "mimeType": "application/vnd.google-apps.folder",
        "parents": [parent]
    });

    let client = reqwest::Client::new();
    let res = client
        .post("https://www.googleapis.com/drive/v3/files?fields=id,name")
        .bearer_auth(&token)
        .json(&body)
        .send()
        .await
        .map_err(|e| format!("Failed to create Drive folder: {e}"))?;

    if !res.status().is_success() {
        let err = res.text().await.unwrap_or_default();
        return Err(format!("Drive create folder error: {err}"));
    }

    res.json::<DriveFolder>().await.map_err(|e| e.to_string())
}

/// Renames a Google Drive folder and returns the updated folder metadata.
pub async fn rename_folder(
    cfg: &AppConfig,
    folder_id: &str,
    new_name: &str,
) -> Result<DriveFolder, String> {
    let trimmed = new_name.trim();
    if trimmed.is_empty() {
        return Err("Folder name cannot be empty.".into());
    }
    let token = get_access_token(cfg).await?;
    let url = format!(
        "https://www.googleapis.com/drive/v3/files/{}?fields=id,name",
        urlencoding::encode(folder_id)
    );
    let client = reqwest::Client::new();
    let res = client
        .patch(&url)
        .bearer_auth(&token)
        .json(&serde_json::json!({ "name": trimmed }))
        .send()
        .await
        .map_err(|e| format!("Failed to rename Drive folder: {e}"))?;

    if !res.status().is_success() {
        let err = res.text().await.unwrap_or_default();
        return Err(format!("Drive rename error: {err}"));
    }
    res.json::<DriveFolder>().await.map_err(|e| e.to_string())
}

/// Moves a Google Drive folder to Trash (`trashed = true`) so accidental folders can be removed safely.
pub async fn delete_folder(cfg: &AppConfig, folder_id: &str) -> Result<(), String> {
    let token = get_access_token(cfg).await?;
    let url = format!(
        "https://www.googleapis.com/drive/v3/files/{}",
        urlencoding::encode(folder_id)
    );
    let client = reqwest::Client::new();
    let res = client
        .patch(&url)
        .bearer_auth(&token)
        .json(&serde_json::json!({ "trashed": true }))
        .send()
        .await
        .map_err(|e| format!("Failed to delete Drive folder: {e}"))?;

    if !res.status().is_success() {
        let err = res.text().await.unwrap_or_default();
        return Err(format!("Drive delete error: {err}"));
    }
    Ok(())
}

/// Finds an existing subfolder by exact name inside `parent_id`, or creates it if missing.
pub async fn find_or_create_folder(
    cfg: &AppConfig,
    parent_id: Option<&str>,
    name: &str,
) -> Result<DriveFolder, String> {
    let trimmed = name.trim();
    let token = get_access_token(cfg).await?;
    let parent = parent_id.filter(|s| !s.trim().is_empty()).unwrap_or("root");
    let escaped_name = trimmed.replace('\'', "\\'");
    let query = format!(
        "'{parent}' in parents and name='{escaped_name}' and mimeType='application/vnd.google-apps.folder' and trashed=false"
    );
    let url = format!(
        "https://www.googleapis.com/drive/v3/files?q={}&fields=files(id,name)&pageSize=1",
        urlencoding::encode(&query)
    );
    let client = reqwest::Client::new();
    let res = client
        .get(&url)
        .bearer_auth(&token)
        .send()
        .await
        .map_err(|e| format!("Failed to search Drive folder: {e}"))?;

    if res.status().is_success() {
        if let Ok(list) = res.json::<DriveListResponse>().await {
            if let Some(found) = list.files.into_iter().next() {
                return Ok(found);
            }
        }
    }
    create_folder(cfg, Some(parent), trimmed).await
}

/// Uploads the mastered MP3 to the configured Google Drive folder (supporting Direct or Base Year/Month mode)
/// and returns the shareable Drive URL.
pub async fn upload_file(
    cfg: &AppConfig,
    file_path: &Path,
    upload_name: &str,
    drive_mode: Option<&str>,
    save_date: Option<&str>,
) -> Result<String, String> {
    let token = get_access_token(cfg).await?;
    let bytes = tokio::fs::read(file_path)
        .await
        .map_err(|e| format!("Failed to read MP3 for upload: {e}"))?;

    let mode = drive_mode
        .map(|s| s.trim().to_lowercase())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| cfg.drive_save_mode.to_lowercase());

    let mut target_parent = cfg.folder_id();
    if mode == "base" {
        let parsed_date = save_date
            .and_then(|s| chrono::NaiveDate::parse_from_str(s.trim(), "%Y-%m-%d").ok())
            .unwrap_or_else(|| chrono::Local::now().date_naive());
        let year_str = parsed_date.format("%Y").to_string();
        let month_str = parsed_date.format("%B").to_string();

        let year_folder = find_or_create_folder(cfg, target_parent.as_deref(), &year_str).await?;
        let month_folder = find_or_create_folder(cfg, Some(&year_folder.id), &month_str).await?;
        target_parent = Some(month_folder.id);
    }

    let metadata = if let Some(fid) = target_parent {
        serde_json::json!({
            "name": upload_name,
            "parents": [fid]
        })
    } else {
        serde_json::json!({
            "name": upload_name
        })
    };

    let meta_part = Part::text(metadata.to_string())
        .mime_str("application/json; charset=UTF-8")
        .map_err(|e| e.to_string())?;
    let file_part = Part::bytes(bytes)
        .file_name(upload_name.to_string())
        .mime_str("audio/mpeg")
        .map_err(|e| e.to_string())?;

    let form = Form::new()
        .part("metadata", meta_part)
        .part("file", file_part);

    let client = reqwest::Client::new();
    let res = client
        .post("https://www.googleapis.com/upload/drive/v3/files?uploadType=multipart&fields=id,webViewLink")
        .bearer_auth(&token)
        .multipart(form)
        .send()
        .await
        .map_err(|e| format!("Drive upload failed: {e}"))?;

    if !res.status().is_success() {
        let body = res.text().await.unwrap_or_default();
        return Err(format!("Drive upload error: {body}"));
    }

    let uploaded: DriveFileResponse = res.json().await.map_err(|e| e.to_string())?;

    if cfg.make_public_link {
        let perm_url = format!(
            "https://www.googleapis.com/drive/v3/files/{}/permissions",
            uploaded.id
        );
        let _ = client
            .post(&perm_url)
            .bearer_auth(&token)
            .json(&serde_json::json!({
                "role": "reader",
                "type": "anyone"
            }))
            .send()
            .await;
    }

    Ok(uploaded.web_view_link.unwrap_or_else(|| {
        format!("https://drive.google.com/file/d/{}/view?usp=sharing", uploaded.id)
    }))
}

fn extract_query_param(http_first_line: &str, key: &str) -> Option<String> {
    let path = http_first_line.split_whitespace().nth(1)?;
    let query = path.split('?').nth(1)?;
    for pair in query.split('&') {
        let mut kv = pair.splitn(2, '=');
        if kv.next() == Some(key) {
            return kv.next().map(|v| urlencoding::decode(v).unwrap_or_default().into_owned());
        }
    }
    None
}

fn open_browser(url: &str) -> Result<(), String> {
    #[cfg(target_os = "windows")]
    {
        // Use PowerShell Start-Process with single-quoted URL so `&` in query params is never split by cmd.exe
        let escaped = url.replace('\'', "''");
        std::process::Command::new("powershell")
            .args([
                "-NoProfile",
                "-WindowStyle",
                "Hidden",
                "-Command",
                &format!("Start-Process '{escaped}'"),
            ])
            .spawn()
            .map_err(|e| format!("Failed to open browser: {e}"))?;
    }
    #[cfg(not(target_os = "windows"))]
    {
        std::process::Command::new("xdg-open")
            .arg(url)
            .spawn()
            .map_err(|e| format!("Failed to open browser: {e}"))?;
    }
    Ok(())
}
