//! Google Drive connector (Drive REST API v3) on the same Google sign-in as the calendar: one OAuth consent with the
//! calendar and Drive scopes (`gcal::scopes_for`), one refresh token in Sentinel's vault, [`GCal`]'s token refresh.
//! The agent gets file metadata and file contents (into the workspace), never tokens.
//!
//! Operations: `about` (account + storage quota), `list` (Drive query language, ordered, paged), `get`, `download`
//! (Google Docs / Sheets / Slides exported as PDF / XLSX / PDF), `upload` (multipart), `create_folder`, `update`
//! (rename / move), `trash` (never a permanent delete: the user empties the bin themselves).

use std::time::Duration;

use omuse_common::util::{get_str, py_str, truthy};
use serde_json::{json, Map, Value};

use crate::gcal::{GCal, GoogleSettings};
use crate::py::httpx_name;

/// HTTP timeout (seconds) of a metadata call; downloads and uploads get [`MEDIA_TIMEOUT`].
pub const TIMEOUT: f64 = 30.0;
pub const MEDIA_TIMEOUT: f64 = 600.0;
/// The metadata fields asked for on every file.
pub const FILE_FIELDS: &str =
    "id,name,mimeType,size,quotaBytesUsed,modifiedTime,viewedByMeTime,createdTime,\
owners(emailAddress,displayName,me),parents,webViewLink,trashed,shared,\
imageMediaMetadata(width,height),videoMediaMetadata(width,height,durationMillis)";
pub const FOLDER_MIME: &str = "application/vnd.google-apps.folder";

/// Python-style error with the message the agent sees.
#[derive(Debug, Clone, thiserror::Error)]
#[error("{0}")]
pub struct GDriveError(pub String);

/// Google-native documents cannot be downloaded as they are: `(export mime, extension)`.
pub fn export_of(mime: &str) -> Option<(&'static str, &'static str)> {
    match mime {
        "application/vnd.google-apps.document" => Some(("application/pdf", ".pdf")),
        "application/vnd.google-apps.spreadsheet" => Some((
            "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet",
            ".xlsx",
        )),
        "application/vnd.google-apps.presentation" => Some(("application/pdf", ".pdf")),
        "application/vnd.google-apps.drawing" => Some(("image/png", ".png")),
        _ => None,
    }
}

/// A coarse kind for the agent: folder / image / video / audio / pdf / doc / sheet / slides / other.
pub fn kind_of(mime: &str) -> &'static str {
    if mime == FOLDER_MIME {
        "folder"
    } else if mime.starts_with("image/") {
        "image"
    } else if mime.starts_with("video/") {
        "video"
    } else if mime.starts_with("audio/") {
        "audio"
    } else if mime == "application/pdf" {
        "pdf"
    } else if mime.ends_with(".document") || mime.contains("wordprocessingml") {
        "doc"
    } else if mime.ends_with(".spreadsheet") || mime.contains("spreadsheetml") {
        "sheet"
    } else if mime.ends_with(".presentation") || mime.contains("presentationml") {
        "slides"
    } else {
        "other"
    }
}

fn round1(x: f64) -> f64 {
    (x * 10.0).round() / 10.0
}

/// Megabytes with two decimals, so a small photo or clip is not "0.0".
fn mb(bytes: u64) -> f64 {
    (bytes as f64 / 1_048_576.0 * 100.0).round() / 100.0
}

/// Bytes as a number from Drive's string fields (`size`, `quotaBytesUsed`).
pub fn bytes_of(v: &Value) -> u64 {
    match v {
        Value::Number(n) => n.as_u64().unwrap_or(0),
        Value::String(s) => s.trim().parse().unwrap_or(0),
        _ => 0,
    }
}

/// `YYYY-MM-DD HH:MM` from an RFC 3339 time, or "" (also for a missing / null time: never opened).
pub fn short_time(v: &Value) -> String {
    let Some(s) = v.as_str().map(str::to_string) else {
        return String::new();
    };
    if s.len() >= 16 && s.as_bytes()[10] == b'T' {
        format!("{} {}", &s[..10], &s[11..16])
    } else {
        s
    }
}

/// The record the agent sees for a file.
pub fn compact(f: &Value) -> Value {
    let mime = get_str(f, "mimeType");
    let size = bytes_of(&f["size"]);
    let quota = bytes_of(&f["quotaBytesUsed"]);
    let owner = f["owners"]
        .as_array()
        .and_then(|a| a.first())
        .cloned()
        .unwrap_or(Value::Null);
    let mut m = Map::new();
    m.insert("id".into(), f["id"].clone());
    m.insert("name".into(), f["name"].clone());
    m.insert("kind".into(), json!(kind_of(&mime)));
    m.insert("mime".into(), json!(mime));
    m.insert("size_mb".into(), json!(mb(size.max(quota))));
    m.insert("modified".into(), json!(short_time(&f["modifiedTime"])));
    m.insert(
        "last_viewed".into(),
        json!(short_time(&f["viewedByMeTime"])),
    );
    m.insert("owner".into(), json!(get_str(&owner, "emailAddress")));
    m.insert("mine".into(), json!(owner.get("me").is_some_and(truthy)));
    m.insert("shared".into(), json!(f.get("shared").is_some_and(truthy)));
    m.insert(
        "trashed".into(),
        json!(f.get("trashed").is_some_and(truthy)),
    );
    m.insert(
        "parents".into(),
        f.get("parents").cloned().unwrap_or(json!([])),
    );
    m.insert("link".into(), f["webViewLink"].clone());
    if let Some(v) = f.get("videoMediaMetadata").filter(|v| v.is_object()) {
        let ms = bytes_of(&v["durationMillis"]);
        m.insert("duration_s".into(), json!(round1(ms as f64 / 1000.0)));
        m.insert("width".into(), v["width"].clone());
        m.insert("height".into(), v["height"].clone());
    } else if let Some(v) = f.get("imageMediaMetadata").filter(|v| v.is_object()) {
        m.insert("width".into(), v["width"].clone());
        m.insert("height".into(), v["height"].clone());
    }
    Value::Object(m)
}

/// Quote a value for the Drive query language.
pub fn q_str(s: &str) -> String {
    format!("'{}'", s.replace('\\', "\\\\").replace('\'', "\\'"))
}

fn fmt_err(status: u16, body: &[u8]) -> String {
    let text = String::from_utf8_lossy(body);
    let msg = match serde_json::from_slice::<Value>(body) {
        Ok(j) if j.is_object() => {
            let m = get_str(&j["error"], "message");
            if m.is_empty() {
                get_str(&j, "error")
            } else {
                m
            }
        }
        _ => String::new(),
    };
    let msg = if msg.is_empty() {
        omuse_common::util::take_chars(&text, 200).to_string()
    } else {
        msg
    };
    match status {
        401 => "Google Drive 授权已失效，请在「连接」页重新用 Google 登录 (Drive sign-in expired: reconnect)".to_string(),
        403 if msg.to_lowercase().contains("insufficient") || msg.contains("scope") => {
            "这次 Google 登录没有授予 Drive 权限：请在「连接」页重新用 Google 登录并勾选 Drive (Drive scope not granted)".to_string()
        }
        404 => format!("Google Drive 里没有这个文件 (file not found): {msg}"),
        _ => format!("Google Drive HTTP {status}: {msg}"),
    }
}

/// A Drive client for one connected account.
pub struct GDrive {
    g: GCal,
    api: String,
    upload: String,
}

impl GDrive {
    /// Built on the Google client (same refresh token as the calendar connection, or its own copy).
    pub fn new(g: GCal, settings: &GoogleSettings) -> Self {
        Self {
            g,
            api: settings.gdrive_api.trim_end_matches('/').to_string(),
            upload: settings.gdrive_upload_api.trim_end_matches('/').to_string(),
        }
    }

    async fn tok(&self) -> Result<String, GDriveError> {
        self.g.access_token().await.map_err(|e| GDriveError(e.0))
    }

    fn http(&self) -> &reqwest::Client {
        self.g.parts().0
    }

    async fn send(&self, rb: reqwest::RequestBuilder) -> Result<(u16, Vec<u8>), GDriveError> {
        let r = rb
            .send()
            .await
            .map_err(|e| GDriveError(format!("{}: {}", httpx_name(&e), e)))?;
        let status = r.status().as_u16();
        let bytes = r.bytes().await.unwrap_or_default().to_vec();
        Ok((status, bytes))
    }

    async fn json(
        &self,
        method: reqwest::Method,
        url: String,
        params: &[(&str, String)],
        body: Option<&Value>,
    ) -> Result<Value, GDriveError> {
        let tok = self.tok().await?;
        let mut rb = self
            .http()
            .request(method, url)
            .timeout(Duration::from_secs_f64(TIMEOUT))
            .header("Authorization", format!("Bearer {}", tok));
        if !params.is_empty() {
            rb = rb.query(params);
        }
        if let Some(b) = body {
            rb = rb.json(b);
        }
        let (status, bytes) = self.send(rb).await?;
        if status >= 400 {
            return Err(GDriveError(fmt_err(status, &bytes)));
        }
        if bytes.is_empty() {
            return Ok(json!({}));
        }
        serde_json::from_slice(&bytes).map_err(|e| GDriveError(format!("JSONDecodeError: {}", e)))
    }

    /// The account and its storage quota: `{email, name, used_gb, limit_gb, trash_gb, drive_gb}`.
    pub async fn about(&self) -> Result<Value, GDriveError> {
        let a = self
            .json(
                reqwest::Method::GET,
                format!("{}/about", self.api),
                &[(
                    "fields",
                    "user(emailAddress,displayName),storageQuota".to_string(),
                )],
                None,
            )
            .await?;
        let q = &a["storageQuota"];
        let gb = |v: &Value| round1(bytes_of(v) as f64 / 1_073_741_824.0);
        Ok(json!({
            "email": get_str(&a["user"], "emailAddress"), "name": get_str(&a["user"], "displayName"),
            "used_gb": gb(&q["usage"]), "limit_gb": if truthy(&q["limit"]) { json!(gb(&q["limit"])) } else { Value::Null },
            "drive_gb": gb(&q["usageInDrive"]), "trash_gb": gb(&q["usageInDriveTrash"]),
        }))
    }

    /// `GET files?q=…`: `{files: [compact…], next_page_token}`.
    pub async fn list(
        &self,
        q: &str,
        order_by: &str,
        page_size: i64,
        page_token: &str,
    ) -> Result<Value, GDriveError> {
        let mut params: Vec<(&str, String)> = vec![
            ("fields", format!("nextPageToken,files({})", FILE_FIELDS)),
            ("pageSize", page_size.clamp(1, 1000).to_string()),
            ("spaces", "drive".into()),
        ];
        if !q.trim().is_empty() {
            params.push(("q", q.trim().to_string()));
        }
        if !order_by.trim().is_empty() {
            params.push(("orderBy", order_by.trim().to_string()));
        }
        if !page_token.is_empty() {
            params.push(("pageToken", page_token.to_string()));
        }
        let r = self
            .json(
                reqwest::Method::GET,
                format!("{}/files", self.api),
                &params,
                None,
            )
            .await?;
        let files: Vec<Value> = r["files"]
            .as_array()
            .map(|a| a.iter().map(compact).collect())
            .unwrap_or_default();
        Ok(json!({"files": files, "next_page_token": get_str(&r, "nextPageToken")}))
    }

    /// One file's metadata (raw Drive record).
    pub async fn get_raw(&self, id: &str) -> Result<Value, GDriveError> {
        self.json(
            reqwest::Method::GET,
            format!("{}/files/{}", self.api, id),
            &[("fields", FILE_FIELDS.to_string())],
            None,
        )
        .await
    }

    /// One file's metadata as the agent sees it.
    pub async fn get(&self, id: &str) -> Result<Value, GDriveError> {
        Ok(compact(&self.get_raw(id).await?))
    }

    /// The file's bytes: `(name, mime, data)`. Google-native documents are exported ([`export_of`]); anything over
    /// `max` bytes is refused before the transfer.
    pub async fn download(
        &self,
        id: &str,
        max: usize,
    ) -> Result<(String, String, Vec<u8>), GDriveError> {
        let meta = self.get_raw(id).await?;
        let mime = get_str(&meta, "mimeType");
        let mut name = get_str(&meta, "name");
        if mime == FOLDER_MIME {
            return Err(GDriveError(format!(
                "「{name}」是文件夹，不能下载 (a folder cannot be downloaded; list it instead)"
            )));
        }
        let size = bytes_of(&meta["size"]) as usize;
        if size > max {
            return Err(GDriveError(format!(
                "文件太大 (file too large: {} MB, limit {} MB): {name}",
                size / 1_048_576,
                max / 1_048_576
            )));
        }
        let tok = self.tok().await?;
        let (url, out_mime) = match export_of(&mime) {
            Some((em, ext)) => {
                if !name.to_lowercase().ends_with(ext) {
                    name.push_str(ext);
                }
                (
                    format!("{}/files/{}/export?mimeType={}", self.api, id, em),
                    em.to_string(),
                )
            }
            None => (format!("{}/files/{}?alt=media", self.api, id), mime),
        };
        let rb = self
            .http()
            .get(url)
            .timeout(Duration::from_secs_f64(MEDIA_TIMEOUT))
            .header("Authorization", format!("Bearer {}", tok));
        let (status, bytes) = self.send(rb).await?;
        if status >= 400 {
            return Err(GDriveError(fmt_err(status, &bytes)));
        }
        if bytes.len() > max {
            return Err(GDriveError(format!(
                "文件太大 (file too large: over {} MB): {name}",
                max / 1_048_576
            )));
        }
        Ok((name, out_mime, bytes))
    }

    /// Upload `data` as a new file (multipart): the compact record of the file made.
    pub async fn upload(
        &self,
        name: &str,
        mime: &str,
        parent: Option<&str>,
        data: Vec<u8>,
    ) -> Result<Value, GDriveError> {
        let mut meta = json!({"name": name});
        if let Some(p) = parent.filter(|p| !p.trim().is_empty()) {
            meta["parents"] = json!([p.trim()]);
        }
        let boundary = format!("omuse{}", omuse_common::util::now_ts() as u64);
        let mut body: Vec<u8> = Vec::with_capacity(data.len() + 512);
        body.extend_from_slice(
            format!(
                "--{b}\r\nContent-Type: application/json; charset=UTF-8\r\n\r\n{m}\r\n--{b}\r\nContent-Type: {t}\r\n\r\n",
                b = boundary,
                m = meta,
                t = if mime.is_empty() { "application/octet-stream" } else { mime }
            )
            .as_bytes(),
        );
        body.extend_from_slice(&data);
        body.extend_from_slice(format!("\r\n--{}--", boundary).as_bytes());
        let tok = self.tok().await?;
        let rb = self
            .http()
            .post(format!(
                "{}/files?uploadType=multipart&fields={}",
                self.upload, FILE_FIELDS
            ))
            .timeout(Duration::from_secs_f64(MEDIA_TIMEOUT))
            .header("Authorization", format!("Bearer {}", tok))
            .header(
                "Content-Type",
                format!("multipart/related; boundary={}", boundary),
            )
            .body(body);
        let (status, bytes) = self.send(rb).await?;
        if status >= 400 {
            return Err(GDriveError(fmt_err(status, &bytes)));
        }
        let v: Value = serde_json::from_slice(&bytes)
            .map_err(|e| GDriveError(format!("JSONDecodeError: {}", e)))?;
        Ok(compact(&v))
    }

    /// A new folder (compact record).
    pub async fn create_folder(
        &self,
        name: &str,
        parent: Option<&str>,
    ) -> Result<Value, GDriveError> {
        let mut meta = json!({"name": name, "mimeType": FOLDER_MIME});
        if let Some(p) = parent.filter(|p| !p.trim().is_empty()) {
            meta["parents"] = json!([p.trim()]);
        }
        let v = self
            .json(
                reqwest::Method::POST,
                format!("{}/files", self.api),
                &[("fields", FILE_FIELDS.to_string())],
                Some(&meta),
            )
            .await?;
        Ok(compact(&v))
    }

    /// Rename and/or move a file (compact record).
    pub async fn update(
        &self,
        id: &str,
        new_name: Option<&str>,
        move_to: Option<&str>,
    ) -> Result<Value, GDriveError> {
        let mut params: Vec<(&str, String)> = vec![("fields", FILE_FIELDS.to_string())];
        let mut body = json!({});
        if let Some(n) = new_name.filter(|n| !n.trim().is_empty()) {
            body["name"] = json!(n.trim());
        }
        if let Some(folder) = move_to.filter(|f| !f.trim().is_empty()) {
            let cur = self.get_raw(id).await?;
            let old: Vec<String> = cur["parents"]
                .as_array()
                .map(|a| a.iter().map(py_str).collect())
                .unwrap_or_default();
            params.push(("addParents", folder.trim().to_string()));
            if !old.is_empty() {
                params.push(("removeParents", old.join(",")));
            }
        }
        let v = self
            .json(
                reqwest::Method::PATCH,
                format!("{}/files/{}", self.api, id),
                &params,
                Some(&body),
            )
            .await?;
        Ok(compact(&v))
    }

    /// Move a file to the bin (`trashed: true`); the user empties the bin themselves.
    pub async fn trash(&self, id: &str) -> Result<Value, GDriveError> {
        let v = self
            .json(
                reqwest::Method::PATCH,
                format!("{}/files/{}", self.api, id),
                &[("fields", FILE_FIELDS.to_string())],
                Some(&json!({"trashed": true})),
            )
            .await?;
        Ok(compact(&v))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn kinds_exports_and_compact_records() {
        assert_eq!(kind_of(FOLDER_MIME), "folder");
        assert_eq!(kind_of("video/mp4"), "video");
        assert_eq!(kind_of("application/vnd.google-apps.spreadsheet"), "sheet");
        assert_eq!(
            export_of("application/vnd.google-apps.document"),
            Some(("application/pdf", ".pdf"))
        );
        assert_eq!(export_of("image/jpeg"), None);
        assert_eq!(q_str("it's"), "'it\\'s'");
        let f = json!({"id": "f1", "name": "clip.mp4", "mimeType": "video/mp4", "size": "52428800",
                       "modifiedTime": "2026-03-01T10:20:30.000Z", "viewedByMeTime": "2025-01-02T03:04:05Z",
                       "owners": [{"emailAddress": "me@example.com", "me": true}], "parents": ["root"],
                       "webViewLink": "https://drive.google.com/file/d/f1", "trashed": false,
                       "videoMediaMetadata": {"width": 1280, "height": 720, "durationMillis": "4500"}});
        let c = compact(&f);
        assert_eq!(c["kind"], "video");
        assert_eq!(c["size_mb"], 50.0);
        assert_eq!(c["modified"], "2026-03-01 10:20");
        assert_eq!(c["last_viewed"], "2025-01-02 03:04");
        assert_eq!(c["owner"], "me@example.com");
        assert_eq!(c["mine"], true);
        assert_eq!(c["duration_s"], 4.5);
        assert_eq!(c["width"], 1280);
        // never opened: no viewedByMeTime at all; a small file keeps two decimals
        let c = compact(
            &json!({"id": "f2", "name": "tiny.jpg", "mimeType": "image/jpeg", "size": "20480",
                                "modifiedTime": "2026-03-01T10:20:30.000Z", "viewedByMeTime": null}),
        );
        assert_eq!(c["last_viewed"], "");
        assert_eq!(c["size_mb"], 0.02);
    }
}
