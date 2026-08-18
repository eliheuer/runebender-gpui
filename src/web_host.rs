// Copyright 2026 the Runebender Authors
// SPDX-License-Identifier: Apache-2.0

//! Web host layer: load and save fonts through the runebender
//! workspace server (`runebender-web/server/serve.mjs`) instead of a
//! filesystem. The server address comes from the page URL's
//! `?server=` query parameter, e.g.
//! `http://127.0.0.1:8321/?server=http://127.0.0.1:8765`.
//!
//! Protocol (all CORS-enabled):
//! - `GET  {base}/runebender/api/info`   → `{ entry, ... }`
//! - `GET  {base}/runebender/api/files`  → `{ files: [{path, ...}] }`
//! - `GET  {base}/runebender/api/file/{rel}` → bytes + `etag` header
//! - `PUT  {base}/runebender/api/file/{rel}` + `If-Match` → `{ etag }`

use std::collections::HashMap;
use std::sync::Arc;

use futures::AsyncReadExt;
use gpui::http_client::{AsyncBody, HttpClient, Request};
use serde::Deserialize;

/// Connection state kept on the workspace: server base URL and the
/// ETag of every file we have read (If-Match tokens for saves).
pub struct WebHost {
    pub base: String,
    pub etags: HashMap<String, String>,
    /// Per-master path prefix from the workspace root to the UFO
    /// ("VirtuaGrotesk-Regular.ufo/").
    pub ufo_prefixes: Vec<String>,
}

#[derive(Deserialize)]
struct Info {
    entry: Option<String>,
}

#[derive(Deserialize)]
struct FileList {
    files: Vec<FileEntry>,
}

#[derive(Deserialize)]
struct FileEntry {
    path: String,
}

#[derive(Deserialize)]
struct PutResponse {
    etag: Option<String>,
    error: Option<String>,
}

/// The `?server=` query parameter of the current page, if any.
pub fn server_from_location() -> Option<String> {
    let search = web_sys::window()?.location().search().ok()?;
    let search = search.strip_prefix('?')?;
    for pair in search.split('&') {
        if let Some(value) = pair.strip_prefix("server=") {
            let decoded = js_sys::decode_uri_component(value)
                .ok()
                .map(|v| String::from(v))?;
            return Some(decoded.trim_end_matches('/').to_string());
        }
    }
    None
}

async fn get_bytes(
    client: &Arc<dyn HttpClient>,
    url: &str,
) -> Result<(Vec<u8>, Option<String>), String> {
    let mut response = client
        .get(url, AsyncBody::empty(), true)
        .await
        .map_err(|e| format!("GET {url}: {e}"))?;
    if !response.status().is_success() {
        return Err(format!("GET {url}: HTTP {}", response.status()));
    }
    let etag = response
        .headers()
        .get("etag")
        .and_then(|v| v.to_str().ok())
        .map(|s| s.trim_matches('"').to_string());
    let mut body = Vec::new();
    response
        .body_mut()
        .read_to_end(&mut body)
        .await
        .map_err(|e| format!("read {url}: {e}"))?;
    Ok((body, etag))
}

fn file_url(base: &str, rel: &str) -> String {
    let encoded: String = rel
        .split('/')
        .map(|seg| js_sys::encode_uri_component(seg).as_string().unwrap_or_default())
        .collect::<Vec<_>>()
        .join("/");
    format!("{base}/runebender/api/file/{encoded}")
}

/// Everything fetched from the server needed to assemble a project.
pub struct FetchedWorkspace {
    pub entry: String,
    pub designspace_text: Option<String>,
    /// All fetched files by root-relative path.
    pub files: HashMap<String, Vec<u8>>,
    pub etags: HashMap<String, String>,
}

/// Fetch the workspace: entry file plus every UFO file the entry
/// references (or the whole tree for a bare UFO entry).
pub async fn fetch_workspace(
    client: Arc<dyn HttpClient>,
    base: String,
) -> Result<FetchedWorkspace, String> {
    let (info_bytes, _) = get_bytes(&client, &format!("{base}/runebender/api/info")).await?;
    let info: Info =
        serde_json::from_slice(&info_bytes).map_err(|e| format!("info: {e}"))?;
    let entry = info.entry.ok_or("server has no entry font")?;

    let (list_bytes, _) =
        get_bytes(&client, &format!("{base}/runebender/api/files")).await?;
    let list: FileList =
        serde_json::from_slice(&list_bytes).map_err(|e| format!("files: {e}"))?;

    let mut files = HashMap::new();
    let mut etags = HashMap::new();
    let mut designspace_text = None;

    if entry.ends_with(".designspace") {
        let (bytes, etag) = get_bytes(&client, &file_url(&base, &entry)).await?;
        if let Some(etag) = etag {
            etags.insert(entry.clone(), etag);
        }
        designspace_text =
            Some(String::from_utf8(bytes).map_err(|e| format!("designspace utf8: {e}"))?);
    }

    // Fetch every file under any .ufo directory (the designspace
    // loader picks the ones its sources name).
    for f in &list.files {
        if !f.path.contains(".ufo/") {
            continue;
        }
        let (bytes, etag) = get_bytes(&client, &file_url(&base, &f.path)).await?;
        if let Some(etag) = etag {
            etags.insert(f.path.clone(), etag);
        }
        files.insert(f.path.clone(), bytes);
    }
    if files.is_empty() {
        return Err("server workspace has no UFO files".into());
    }
    Ok(FetchedWorkspace {
        entry,
        designspace_text,
        files,
        etags,
    })
}

/// One file to save: root-relative path and its bytes.
pub struct SaveFile {
    pub path: String,
    pub bytes: Vec<u8>,
}

/// PUT one file with its known ETag (or create). Returns the new
/// ETag on success.
pub async fn put_file(
    client: &Arc<dyn HttpClient>,
    base: &str,
    file: &SaveFile,
    etag: Option<&str>,
) -> Result<String, String> {
    let if_match = match etag {
        Some(t) => format!("\"{t}\""),
        None => "*".to_string(),
    };
    let request = Request::builder()
        .method("PUT")
        .uri(file_url(base, &file.path))
        .header("If-Match", if_match)
        .header("content-type", "application/octet-stream")
        .body(AsyncBody::from(file.bytes.clone()))
        .map_err(|e| format!("build PUT: {e}"))?;
    let mut response = client
        .send(request)
        .await
        .map_err(|e| format!("PUT {}: {e}", file.path))?;
    let mut body = Vec::new();
    response
        .body_mut()
        .read_to_end(&mut body)
        .await
        .map_err(|e| format!("read PUT response: {e}"))?;
    let parsed: PutResponse = serde_json::from_slice(&body).unwrap_or(PutResponse {
        etag: None,
        error: Some(format!("HTTP {}", response.status())),
    });
    if !response.status().is_success() {
        return Err(format!(
            "save {}: {}",
            file.path,
            parsed.error.unwrap_or_else(|| format!("HTTP {}", response.status()))
        ));
    }
    parsed
        .etag
        .ok_or_else(|| format!("save {}: no etag in response", file.path))
}
