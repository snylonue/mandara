//! Static frontend files snapshotted with content-derived ETags at construction.

#[cfg(test)]
mod tests;

use std::collections::{HashMap, HashSet};
use std::fs;
use std::io;
use std::path::{Component, Path, PathBuf};
use std::sync::Arc;

use axum::Router;
use axum::body::{Body, Bytes};
use axum::extract::State;
use axum::http::{HeaderMap, HeaderValue, Method, StatusCode, Uri, header};
use axum::response::Response;
use axum::routing::get;
use headers::{ETag, HeaderMapExt, IfMatch, IfNoneMatch};
use percent_encoding::percent_decode_str;
use sha2::{Digest, Sha256};

struct Asset {
    bytes: Bytes,
    etag: ETag,
    content_type: HeaderValue,
}

#[derive(Default)]
struct Frontend {
    files: HashMap<PathBuf, Asset>,
    blocked: HashSet<PathBuf>,
}

impl Frontend {
    fn load(
        &mut self,
        root: &Path,
        relative: &Path,
        parents: &mut HashSet<PathBuf>,
    ) -> io::Result<()> {
        let path = match fs::canonicalize(root.join(relative)) {
            Ok(path) => path,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(()),
            Err(error) => return Err(error),
        };
        if !path.starts_with(root) {
            self.blocked.insert(relative.to_path_buf());
            return Ok(());
        }
        let metadata = fs::metadata(&path)?;
        if metadata.is_dir() {
            // Internal directory links are allowed, but must not create cycles.
            if !parents.insert(path.clone()) {
                self.blocked.insert(relative.to_path_buf());
                return Ok(());
            }
            for entry in fs::read_dir(&path)? {
                self.load(root, &relative.join(entry?.file_name()), parents)?;
            }
            parents.remove(&path);
        } else if metadata.is_file() {
            let bytes = Bytes::from(fs::read(&path)?);
            let digest: String = Sha256::digest(&bytes)
                .iter()
                .map(|byte| format!("{byte:02x}"))
                .collect();
            self.files.insert(
                relative.to_path_buf(),
                Asset {
                    bytes,
                    etag: format!("\"sha256-{digest}\"").parse().unwrap(),
                    content_type: mime_guess::from_path(&path)
                        .first_or_octet_stream()
                        .as_ref()
                        .parse()
                        .unwrap(),
                },
            );
        }
        Ok(())
    }
}

pub(super) fn router(dist: &Path) -> io::Result<Router> {
    let root = fs::canonicalize(dist)?;
    if !root.is_dir() {
        return Err(io::ErrorKind::NotADirectory.into());
    }
    // Keep bytes and validators together: later filesystem changes must not
    // produce a new response body with an old ETag. Rebuild to refresh files.
    let mut frontend = Frontend::default();
    frontend.load(&root, Path::new(""), &mut HashSet::new())?;
    Ok(Router::new().fallback_service(get(serve).with_state(Arc::new(frontend))))
}

async fn serve(
    State(frontend): State<Arc<Frontend>>,
    uri: Uri,
    method: Method,
    headers: HeaderMap,
) -> Result<Response, StatusCode> {
    let decoded = percent_decode_str(uri.path())
        .decode_utf8()
        .map_err(|_| StatusCode::BAD_REQUEST)?;
    let relative = Path::new(decoded.trim_start_matches('/'));
    if decoded.contains(['\\', '\0'])
        || relative
            .components()
            .any(|part| !matches!(part, Component::Normal(_) | Component::CurDir))
    {
        return Err(StatusCode::NOT_FOUND);
    }
    let relative: PathBuf = relative
        .components()
        .filter(|part| !matches!(part, Component::CurDir))
        .collect();
    let index = relative.join("index.html");
    if relative
        .ancestors()
        .chain(index.ancestors())
        .any(|path| frontend.blocked.contains(path))
    {
        return Err(StatusCode::NOT_FOUND);
    }
    let file = if decoded.ends_with('/') {
        None
    } else {
        frontend.files.get(&relative)
    };
    let (file, status) = match file.or_else(|| frontend.files.get(&index)) {
        Some(file) => (file, StatusCode::OK),
        None => {
            // Preserve the existing index.html fallback's 404 status.
            let index = frontend
                .files
                .get(Path::new("index.html"))
                .ok_or(StatusCode::NOT_FOUND)?;
            (index, StatusCode::NOT_FOUND)
        }
    };
    let cache_control = if status == StatusCode::OK && uri.path().starts_with("/assets/") {
        "public, max-age=31536000, immutable"
    } else {
        "no-cache"
    };
    let mut response = Response::builder()
        .status(status)
        .header(header::CACHE_CONTROL, cache_control);

    if status == StatusCode::OK {
        let etag = &file.etag;
        response.headers_mut().unwrap().typed_insert(etag.clone());
        let if_match = headers
            .typed_try_get::<IfMatch>()
            .map_err(|_| StatusCode::BAD_REQUEST)?;
        if if_match.is_some_and(|condition| !condition.precondition_passes(etag)) {
            response
                .headers_mut()
                .unwrap()
                .insert(header::CACHE_CONTROL, "no-cache".parse().unwrap());
            return Ok(response
                .status(StatusCode::PRECONDITION_FAILED)
                .body(Body::empty())
                .unwrap());
        }
        let if_none_match = headers
            .typed_try_get::<IfNoneMatch>()
            .map_err(|_| StatusCode::BAD_REQUEST)?;
        if if_none_match.is_some_and(|condition| !condition.precondition_passes(etag)) {
            // A 304 Content-Length describes the full representation, not its empty body.
            return Ok(response
                .status(StatusCode::NOT_MODIFIED)
                .header(header::CONTENT_LENGTH, file.bytes.len())
                .body(Body::empty())
                .unwrap());
        }
    }

    Ok(response
        .header(header::CONTENT_TYPE, file.content_type.clone())
        .header(header::CONTENT_LENGTH, file.bytes.len())
        .header(header::ACCEPT_RANGES, "none")
        .body(if method == Method::HEAD {
            Body::empty()
        } else {
            Body::from(file.bytes.clone())
        })
        .unwrap())
}
