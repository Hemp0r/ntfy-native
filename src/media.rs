//! Capped download of notification images and icons into a local cache dir.

use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use url::Url;

use crate::notification::{MediaRef, Notification};
use crate::ntfy::client::request;
use crate::secrets::Auth;

pub const MAX_IMAGE_BYTES: u64 = 5 * 1024 * 1024;
pub const MAX_ICON_BYTES: u64 = 1024 * 1024;

const FETCH_TIMEOUT: Duration = Duration::from_secs(10);
const MAX_STEM_LEN: usize = 100;

pub struct MediaFetcher {
    http: reqwest::Client,
    dir: PathBuf,
    timeout: Duration,
}

impl MediaFetcher {
    pub fn new(http: reqwest::Client, dir: PathBuf) -> Self {
        Self {
            http,
            dir,
            timeout: FETCH_TIMEOUT,
        }
    }

    /// Downloads an image to `<dir>/<stem>.<ext>`; `None` on any failure or
    /// limit violation (no partial file is left behind).
    pub async fn fetch(
        &self,
        url: &str,
        file_stem: &str,
        max_bytes: u64,
        auth: Option<&Auth>,
    ) -> Option<PathBuf> {
        let parsed = Url::parse(url).ok()?;
        if !matches!(parsed.scheme(), "http" | "https") {
            tracing::debug!(url = %redacted(&parsed), "media: unsupported scheme");
            return None;
        }
        let stem = sanitize_stem(file_stem);
        let result =
            tokio::time::timeout(self.timeout, self.download(url, &stem, max_bytes, auth)).await;
        match result {
            Ok(Ok(path)) => Some(path),
            Ok(Err(reason)) => {
                tracing::debug!(url = %redacted(&parsed), reason, "media: download skipped");
                None
            }
            Err(_) => {
                tracing::debug!(url = %redacted(&parsed), "media: download timed out");
                None
            }
        }
    }

    async fn download(
        &self,
        url: &str,
        stem: &str,
        max_bytes: u64,
        auth: Option<&Auth>,
    ) -> Result<PathBuf, &'static str> {
        let mut resp = request(&self.http, url, auth)
            .send()
            .await
            .map_err(|_| "request failed")?;
        if !resp.status().is_success() {
            return Err("non-success status");
        }
        let ext = resp
            .headers()
            .get(reqwest::header::CONTENT_TYPE)
            .and_then(|v| v.to_str().ok())
            .and_then(extension_for)
            .ok_or("not a supported image type")?;
        if resp.content_length().is_some_and(|len| len > max_bytes) {
            return Err("content-length over limit");
        }

        fs::create_dir_all(&self.dir).map_err(|_| "cannot create media dir")?;
        let final_path = self.dir.join(format!("{stem}.{ext}"));
        let mut tmp = TempFile::create(self.dir.join(format!(".{stem}.{ext}.part")))
            .map_err(|_| "cannot create temp file")?;

        let mut total: u64 = 0;
        while let Some(chunk) = resp.chunk().await.map_err(|_| "stream failed")? {
            total += chunk.len() as u64;
            if total > max_bytes {
                return Err("stream over limit");
            }
            tmp.write(&chunk).map_err(|_| "write failed")?;
        }
        tmp.commit(&final_path).map_err(|_| "rename failed")?;
        Ok(final_path)
    }

    /// Replaces remote image/icon refs with local files; failures become `None`.
    pub async fn resolve(
        &self,
        mut n: Notification,
        base_url: &str,
        auth: Option<&Auth>,
    ) -> Notification {
        let id = sanitize_stem(&n.id);
        let (image_stem, icon_stem) = (format!("{id}-image"), format!("{id}-icon"));
        let (image, icon) = tokio::join!(
            self.resolve_ref(n.image.take(), &image_stem, MAX_IMAGE_BYTES, base_url, auth),
            self.resolve_ref(n.icon.take(), &icon_stem, MAX_ICON_BYTES, base_url, auth),
        );
        n.image = image;
        n.icon = icon;
        n
    }

    async fn resolve_ref(
        &self,
        media: Option<MediaRef>,
        stem: &str,
        max_bytes: u64,
        base_url: &str,
        auth: Option<&Auth>,
    ) -> Option<MediaRef> {
        match media? {
            MediaRef::Local(path) => Some(MediaRef::Local(path)),
            MediaRef::Remote(url) => {
                let auth = auth.filter(|_| url_within_base(&url, base_url));
                self.fetch(&url, stem, max_bytes, auth)
                    .await
                    .map(MediaRef::Local)
            }
        }
    }

    /// Removes regular files in the cache dir older than `older_than`.
    pub fn cleanup(&self, older_than: Duration) -> std::io::Result<usize> {
        let entries = match fs::read_dir(&self.dir) {
            Ok(e) => e,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(0),
            Err(e) => return Err(e),
        };
        let now = SystemTime::now();
        let mut removed = 0;
        for entry in entries {
            let entry = entry?;
            let Ok(meta) = entry.metadata() else { continue };
            if !meta.is_file() {
                continue;
            }
            let Ok(modified) = meta.modified() else {
                continue;
            };
            let age = now.duration_since(modified).unwrap_or_default();
            if age > older_than && fs::remove_file(entry.path()).is_ok() {
                removed += 1;
            }
        }
        Ok(removed)
    }
}

/// Partial download that is deleted on drop unless committed.
struct TempFile {
    path: PathBuf,
    file: Option<fs::File>,
}

impl TempFile {
    fn create(path: PathBuf) -> std::io::Result<Self> {
        let file = fs::File::create(&path)?;
        Ok(Self {
            path,
            file: Some(file),
        })
    }

    fn write(&mut self, data: &[u8]) -> std::io::Result<()> {
        self.file
            .as_mut()
            .expect("open until commit")
            .write_all(data)
    }

    fn commit(mut self, dest: &Path) -> std::io::Result<()> {
        let mut file = self.file.take().expect("open until commit");
        file.flush()?;
        drop(file);
        fs::rename(&self.path, dest)
    }
}

impl Drop for TempFile {
    fn drop(&mut self) {
        // After a successful commit the temp path no longer exists.
        self.file.take();
        let _ = fs::remove_file(&self.path);
    }
}

fn extension_for(content_type: &str) -> Option<&'static str> {
    let mime = content_type.split(';').next()?.trim().to_ascii_lowercase();
    match mime.as_str() {
        "image/png" => Some("png"),
        "image/jpeg" | "image/jpg" => Some("jpg"),
        "image/gif" => Some("gif"),
        "image/webp" => Some("webp"),
        _ => None,
    }
}

fn sanitize_stem(s: &str) -> String {
    let stem: String = s
        .chars()
        .take(MAX_STEM_LEN)
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '_' || c == '-' {
                c
            } else {
                '_'
            }
        })
        .collect();
    if stem.is_empty() { "_".into() } else { stem }
}

/// Same origin (scheme, host, port) and `base_url`'s path as a segment prefix.
fn url_within_base(url: &str, base_url: &str) -> bool {
    let (Ok(url), Ok(base)) = (Url::parse(url), Url::parse(base_url)) else {
        return false;
    };
    if url.origin() != base.origin() {
        return false;
    }
    let base_path = base.path().trim_end_matches('/');
    let path = url.path();
    path == base_path
        || path
            .strip_prefix(base_path)
            .is_some_and(|rest| rest.starts_with('/'))
}

fn redacted(url: &Url) -> String {
    let mut u = url.clone();
    u.set_query(None);
    u.set_fragment(None);
    let _ = u.set_username("");
    let _ = u.set_password(None);
    u.to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn base_matching_is_by_origin_and_path_segment() {
        let base = "https://ntfy.example.org";
        assert!(url_within_base("https://ntfy.example.org/file/x.png", base));
        assert!(!url_within_base(
            "https://ntfy.example.org.evil.com/x.png",
            base
        ));
        assert!(!url_within_base("http://ntfy.example.org/x.png", base));
        assert!(!url_within_base(
            "https://ntfy.example.org:8443/x.png",
            base
        ));
        let sub = "https://h.example/ntfy/";
        assert!(url_within_base("https://h.example/ntfy/a.png", sub));
        assert!(!url_within_base("https://h.example/ntfyevil/a.png", sub));
    }

    #[test]
    fn stem_is_sanitized_and_capped() {
        assert_eq!(sanitize_stem("a/b..c-d_e"), "a_b__c-d_e");
        assert_eq!(sanitize_stem(&"x".repeat(300)).len(), 100);
    }
}
