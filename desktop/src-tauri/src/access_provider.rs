//! ZOUGCLOUD(ZC-011): GitHub-backed visibility manifest provider.
//!
//! Members read the manifest from `raw.githubusercontent.com` with **no
//! credential at all** — that is why the repository is public and contains
//! only opaque UUIDs. The admin writes it through the GitHub Contents API with
//! a token that exists solely in their own Windows Credential Manager and is
//! never shipped, logged or written to disk by this client.
//!
//! All the decision logic lives in the `access` crate; this file only moves
//! bytes. Swapping GitHub for something self-hosted later means replacing this
//! module, not the rules.

use std::time::Duration;

use access::{ACCESS, AccessManifest};
use base64::{Engine as _, engine::general_purpose::STANDARD as BASE64};
use log::{debug, info, warn};
use serde::Deserialize;

const OWNER_REPO: &str = "ZacharyTanguay/zougcloud-games-access";
const MANIFEST_FILE: &str = "visibility.json";
const BRANCH: &str = "main";

/// Windows Credential Manager target, created by the admin with:
/// `cmdkey /generic:ZougCloud/GitHubToken /user:ZacharyTanguay /pass`
///
/// On Windows the target name is the credential's sole identifier, so this
/// string must match exactly what was stored.
const CREDENTIAL_TARGET: &str = "ZougCloud/GitHubToken";

/// Poll cadence.
///
/// Chosen to match the `Cache-Control: max-age=300` that
/// raw.githubusercontent.com is currently observed to send — polling faster
/// would only re-read a cached copy. This is an observation, not a protocol
/// assumption: correctness rests on ETag/If-None-Match below, which stays
/// right whatever the CDN decides to do.
const POLL_INTERVAL: Duration = Duration::from_secs(300);

fn raw_url() -> String {
    format!("https://raw.githubusercontent.com/{OWNER_REPO}/{BRANCH}/{MANIFEST_FILE}")
}

fn contents_api_url() -> String {
    format!("https://api.github.com/repos/{OWNER_REPO}/contents/{MANIFEST_FILE}")
}

fn client() -> reqwest::Client {
    reqwest::Client::builder()
        // GitHub rejects requests with no User-Agent.
        .user_agent("Drop-ZougCloud")
        .build()
        .unwrap_or_default()
}

/// Fetch the manifest, sending `If-None-Match` when we have an ETag.
///
/// Every failure maps to [`ManifestResponse::Unavailable`], which the cache
/// treats as "keep what you have". Nothing in this function can widen access.
pub async fn fetch(etag: Option<&str>) -> access::store::ManifestResponse {
    use access::store::ManifestResponse;

    let mut request = client().get(raw_url());
    if let Some(etag) = etag {
        request = request.header(reqwest::header::IF_NONE_MATCH, etag);
    }

    let response = match request.send().await {
        Ok(r) => r,
        Err(e) => {
            debug!("access manifest fetch failed: {e}");
            return ManifestResponse::Unavailable;
        }
    };

    if response.status() == reqwest::StatusCode::NOT_MODIFIED {
        return ManifestResponse::NotModified;
    }

    if !response.status().is_success() {
        warn!("access manifest fetch returned {}", response.status());
        return ManifestResponse::Unavailable;
    }

    let new_etag = response
        .headers()
        .get(reqwest::header::ETAG)
        .and_then(|v| v.to_str().ok())
        .map(str::to_owned);

    match response.text().await {
        Ok(text) => ManifestResponse::Body {
            text,
            etag: new_etag,
        },
        Err(e) => {
            warn!("could not read the access manifest body: {e}");
            ManifestResponse::Unavailable
        }
    }
}

/// Poll the manifest for as long as the app runs.
///
/// Deliberately survives every error: an outage must not stop future polls,
/// or a member would stay on a stale policy until they restarted Drop.
pub async fn poll_task() -> ! {
    use access::store::ApplyOutcome;

    let mut interval = tokio::time::interval(POLL_INTERVAL);

    loop {
        // Ticks immediately the first time, so a launch picks up changes
        // without waiting five minutes.
        interval.tick().await;

        let etag = ACCESS.etag();
        let response = fetch(etag.as_deref()).await;
        let outcome = ACCESS.apply(response, chrono::Utc::now().timestamp());

        match outcome {
            ApplyOutcome::Updated { revision } => {
                info!("access manifest updated to revision {revision}");
            }
            ApplyOutcome::NoPolicy => {
                debug!("no access manifest available yet; members see nothing gated");
            }
            ApplyOutcome::Unchanged | ApplyOutcome::RetainedCache => {}
        }
    }
}

// --- admin write ---------------------------------------------------------

#[derive(Debug)]
pub enum WriteError {
    NoCredential,
    Http(String),
    Rejected { status: u16, body: String },
}

impl std::fmt::Display for WriteError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            WriteError::NoCredential => write!(
                f,
                "No GitHub token found. Store one with: \
                 cmdkey /generic:{CREDENTIAL_TARGET} /user:<you> /pass"
            ),
            WriteError::Http(e) => write!(f, "GitHub request failed: {e}"),
            // The body is GitHub's own error text; the token is never part of
            // a response and so cannot leak here.
            WriteError::Rejected { status, body } => {
                write!(f, "GitHub refused the update ({status}): {body}")
            }
        }
    }
}

/// Read the admin token.
///
/// Returns the token itself, which must never be logged, echoed into an error,
/// or written anywhere. Callers pass it straight to a request header.
fn admin_token() -> Option<String> {
    match keyring::Entry::new_with_target(CREDENTIAL_TARGET, "ZougCloud", "GitHubToken") {
        Ok(entry) => match entry.get_password() {
            Ok(token) if !token.trim().is_empty() => Some(token),
            Ok(_) => None,
            Err(e) => {
                // Deliberately logs the error kind only, never the secret.
                debug!("no usable GitHub token in the credential store: {e}");
                None
            }
        },
        Err(e) => {
            debug!("could not open the credential store: {e}");
            None
        }
    }
}

pub fn admin_token_configured() -> bool {
    admin_token().is_some()
}

#[derive(Deserialize)]
struct ContentsResponse {
    sha: String,
    #[serde(default)]
    content: String,
}

/// What is actually published right now, read from the API.
///
/// The admin **must** edit from this and not from the raw endpoint. raw sits
/// behind a CDN that serves a stale copy for up to five minutes, so an edit
/// based on it can reuse a revision number or silently clobber a newer change.
/// The round-trip test caught exactly that: it read revision 1 from the CDN
/// while the API already held revision 2.
pub async fn fetch_published() -> Result<(Option<String>, AccessManifest), WriteError> {
    let token = admin_token().ok_or(WriteError::NoCredential)?;

    let Some(raw) = current_contents(&token).await? else {
        return Ok((None, AccessManifest::default()));
    };

    // The API returns base64 with newlines in it.
    let decoded = BASE64
        .decode(raw.content.replace(['\n', '\r'], ""))
        .map_err(|e| WriteError::Http(format!("could not decode the manifest: {e}")))?;
    let manifest = serde_json::from_slice::<AccessManifest>(&decoded)
        .map_err(|e| WriteError::Http(format!("published manifest is malformed: {e}")))?;

    Ok((Some(raw.sha), manifest))
}

/// Current blob SHA, which the Contents API requires to update a file. Its
/// absence means the file does not exist yet, which is a valid create.
async fn current_sha(token: &str) -> Result<Option<String>, WriteError> {
    Ok(current_contents(token).await?.map(|c| c.sha))
}

async fn current_contents(token: &str) -> Result<Option<ContentsResponse>, WriteError> {
    let response = client()
        .get(contents_api_url())
        .bearer_auth(token)
        .header("Accept", "application/vnd.github+json")
        .send()
        .await
        .map_err(|e| WriteError::Http(e.to_string()))?;

    if response.status() == reqwest::StatusCode::NOT_FOUND {
        return Ok(None);
    }

    if !response.status().is_success() {
        return Err(WriteError::Rejected {
            status: response.status().as_u16(),
            body: response.text().await.unwrap_or_default(),
        });
    }

    let parsed: ContentsResponse = response
        .json()
        .await
        .map_err(|e| WriteError::Http(e.to_string()))?;
    Ok(Some(parsed))
}

/// Publish an edited manifest safely.
///
/// Re-reads the authoritative copy, refuses if it moved under us, bumps the
/// revision and writes. Going through here rather than calling
/// [`write_manifest`] directly is what stops two edits in quick succession from
/// reusing a revision number or clobbering each other — the CDN's five-minute
/// staleness makes that easy to do by accident.
///
/// The freshly published manifest is also applied locally straight away, so the
/// admin's own client reflects their change immediately instead of waiting out
/// the CDN and the poll.
pub async fn publish(
    mut edited: AccessManifest,
    base_revision: u64,
    message: &str,
) -> Result<u64, WriteError> {
    let (_, published) = fetch_published().await?;

    if published.revision != base_revision {
        return Err(WriteError::Http(format!(
            "the manifest changed while you were editing (you started from revision \
             {base_revision}, it is now {}). Reload and redo the change.",
            published.revision
        )));
    }

    edited.revision = published.revision + 1;
    edited.schema_version = access::SCHEMA_VERSION;

    write_manifest(&edited, message).await?;

    let revision = edited.revision;
    ACCESS.store(edited, None, chrono::Utc::now().timestamp());
    Ok(revision)
}

/// Publish a manifest.
///
/// The caller is responsible for having bumped `revision`; this only moves the
/// bytes. Members pick the change up on their next poll.
pub async fn write_manifest(
    manifest: &AccessManifest,
    message: &str,
) -> Result<String, WriteError> {
    let token = admin_token().ok_or(WriteError::NoCredential)?;

    let json = serde_json::to_string_pretty(manifest)
        .map_err(|e| WriteError::Http(e.to_string()))?;
    let encoded = BASE64.encode(json.as_bytes());

    let sha = current_sha(&token).await?;

    let mut body = serde_json::json!({
        "message": message,
        "content": encoded,
        "branch": BRANCH,
    });
    if let Some(sha) = sha {
        body["sha"] = serde_json::Value::String(sha);
    }

    let response = client()
        .put(contents_api_url())
        .bearer_auth(&token)
        .header("Accept", "application/vnd.github+json")
        .json(&body)
        .send()
        .await
        .map_err(|e| WriteError::Http(e.to_string()))?;

    if !response.status().is_success() {
        return Err(WriteError::Rejected {
            status: response.status().as_u16(),
            body: response.text().await.unwrap_or_default(),
        });
    }

    info!(
        "published access manifest revision {} to {OWNER_REPO}",
        manifest.revision
    );
    Ok(manifest.revision.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn urls_point_at_the_manifest_repository() {
        assert_eq!(
            raw_url(),
            "https://raw.githubusercontent.com/ZacharyTanguay/zougcloud-games-access/main/visibility.json"
        );
        assert!(contents_api_url().starts_with("https://api.github.com/repos/"));
        assert!(contents_api_url().ends_with("/contents/visibility.json"));
    }

    #[test]
    fn the_poll_interval_matches_the_observed_cdn_cache() {
        assert_eq!(POLL_INTERVAL.as_secs(), 300);
    }

    #[test]
    fn the_credential_target_matches_what_the_admin_stored() {
        // Must equal the target used with cmdkey; on Windows this string is
        // the credential's only identifier.
        assert_eq!(CREDENTIAL_TARGET, "ZougCloud/GitHubToken");
    }

    #[test]
    fn a_missing_credential_explains_itself_without_leaking_anything() {
        let message = WriteError::NoCredential.to_string();
        assert!(message.contains("cmdkey"), "{message}");
        assert!(message.contains(CREDENTIAL_TARGET), "{message}");
    }

    /// ZOUGCLOUD(ZC-011): end-to-end check against the real repository.
    ///
    /// `#[ignore]`d because it needs the network and the admin's credential,
    /// neither of which belongs in a normal test run. Run deliberately:
    ///
    /// ```text
    /// cargo test -p drop-app --lib real_github_round_trip -- --ignored --nocapture
    /// ```
    ///
    /// Non-destructive by construction: it only increments `revision`, and
    /// carries every game policy and member grant through untouched. Nothing
    /// a member can see changes.
    #[test]
    #[ignore = "hits the network and needs the admin credential"]
    fn real_github_round_trip() {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("runtime");

        runtime.block_on(async {
            assert!(
                admin_token_configured(),
                "no token at {CREDENTIAL_TARGET}; store one with cmdkey first"
            );
            println!("credential found (value never read into the log)");

            // Give the global state somewhere disposable to live, so the
            // "applies locally" assertion below has something to observe and
            // the developer's real cache is left alone.
            let scratch = tempfile::tempdir().expect("tempdir");
            access::state::AccessState::init(scratch.path().join("visibility.json"));

            // 1. Read the AUTHORITATIVE copy, not the CDN one.
            //
            //    An earlier version of this test read from the raw endpoint and
            //    got revision 1 while the API already held revision 2 -- a
            //    stale base that would have reused a revision number. That is
            //    the mistake `fetch_published` exists to prevent.
            let (sha, before) = fetch_published().await.expect("authoritative read");
            println!("published revision = {} (blob {sha:?})", before.revision);

            // 2. Publish through the guarded path: it re-reads, refuses if the
            //    manifest moved, bumps the revision and applies locally.
            let next_revision = publish(
                before.clone(),
                before.revision,
                "visibility: round-trip verification",
            )
            .await
            .expect("publish should succeed");
            println!("wrote revision {next_revision}");

            // The admin's own client must not wait for the CDN.
            assert_eq!(
                ACCESS.revision(),
                Some(next_revision),
                "the local state should reflect the write immediately"
            );

            // 3. A stale base must be refused rather than silently overwritten.
            let conflict = publish(before.clone(), before.revision, "should not happen").await;
            assert!(
                conflict.is_err(),
                "publishing from a stale base must be refused"
            );
            println!("stale-base publish correctly refused");

            let next = AccessManifest {
                revision: next_revision,
                ..before.clone()
            };

            // 3. Read it back on the member path.
            //
            //    raw.githubusercontent.com sits behind a CDN that answers with
            //    Cache-Control: max-age=300, so a fresh write is genuinely
            //    invisible there for up to five minutes. That is a property of
            //    the transport, not a fault: this loop has to outlast it, or it
            //    reports a failure that is really just an edge cache doing its
            //    job. Allow a margin beyond 300s.
            let mut seen = None;
            for attempt in 0..24u32 {
                tokio::time::sleep(Duration::from_secs(20)).await;
                if let access::store::ManifestResponse::Body { text, .. } = fetch(None).await
                    && let Ok(m) = serde_json::from_str::<AccessManifest>(&text)
                    && m.revision == next.revision
                {
                    seen = Some(m);
                    println!(
                        "new revision visible on the raw endpoint after ~{}s",
                        (attempt + 1) * 20
                    );
                    break;
                }
            }

            let seen = seen.expect(
                "the new revision never reached the raw endpoint within 8 minutes, \
                 which is longer than its advertised cache lifetime",
            );
            assert_eq!(seen.revision, next.revision);
            assert_eq!(
                seen.games.len(),
                before.games.len(),
                "game policies must be carried through untouched"
            );
            assert_eq!(
                seen.users.len(),
                before.users.len(),
                "member grants must be carried through untouched"
            );
        });
    }
}

/// Where the cached manifest lives, beside the other ZougCloud state.
pub fn access_manifest_path() -> std::path::PathBuf {
    crate::playtime::zougcloud_dir().join("visibility.json")
}
