//! Static frontend files with content-derived ETags.

use std::io;
use std::path::{Component, Path, PathBuf};

use axum::Router;
use axum::body::Body;
use axum::extract::State;
use axum::http::{HeaderMap, Method, StatusCode, Uri, header};
use axum::response::Response;
use axum::routing::get;
use headers::{ETag, HeaderMapExt, IfMatch, IfNoneMatch};
use percent_encoding::percent_decode_str;
use sha2::{Digest, Sha256};
use tokio::fs;

pub(super) fn router(dist: &Path) -> Router {
    Router::new().fallback_service(get(serve).with_state(dist.to_path_buf()))
}

async fn serve(
    State(dist): State<PathBuf>,
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

    let root = fs::canonicalize(dist).await.map_err(file_error)?;
    let (path, status) = match resolve_file(&root, relative).await {
        Ok(path) => (path, StatusCode::OK),
        Err(error)
            if matches!(
                error.kind(),
                io::ErrorKind::NotFound | io::ErrorKind::NotADirectory
            ) =>
        {
            // Preserve the existing index.html fallback's 404 status.
            let index = resolve_file(&root, Path::new("index.html"))
                .await
                .map_err(file_error)?;
            (index, StatusCode::NOT_FOUND)
        }
        Err(error) => return Err(file_error(error)),
    };

    // The same bytes supply the hash and body. Frontend files are small enough
    // to buffer; range requests intentionally receive the full representation.
    let bytes = fs::read(&path).await.map_err(file_error)?;
    let cache_control = if status == StatusCode::OK && uri.path().starts_with("/assets/") {
        "public, max-age=31536000, immutable"
    } else {
        "no-cache"
    };
    let mut response = Response::builder()
        .status(status)
        .header(header::CACHE_CONTROL, cache_control);

    if status == StatusCode::OK {
        let digest: String = Sha256::digest(&bytes)
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect();
        let etag: ETag = format!("\"sha256-{digest}\"").parse().unwrap();
        response.headers_mut().unwrap().typed_insert(etag.clone());

        let if_match = headers
            .typed_try_get::<IfMatch>()
            .map_err(|_| StatusCode::BAD_REQUEST)?;
        if if_match.is_some_and(|condition| !condition.precondition_passes(&etag)) {
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
        if if_none_match.is_some_and(|condition| !condition.precondition_passes(&etag)) {
            // A 304 Content-Length describes the full representation, not its empty body.
            return Ok(response
                .status(StatusCode::NOT_MODIFIED)
                .header(header::CONTENT_LENGTH, bytes.len())
                .body(Body::empty())
                .unwrap());
        }
    }

    Ok(response
        .header(
            header::CONTENT_TYPE,
            mime_guess::from_path(&path)
                .first_or_octet_stream()
                .as_ref(),
        )
        .header(header::CONTENT_LENGTH, bytes.len())
        .header(header::ACCEPT_RANGES, "none")
        .body(if method == Method::HEAD {
            Body::empty()
        } else {
            Body::from(bytes)
        })
        .unwrap())
}

async fn resolve_file(root: &Path, relative: &Path) -> io::Result<PathBuf> {
    let mut path = fs::canonicalize(root.join(relative)).await?;
    if !path.starts_with(root) {
        return Err(io::ErrorKind::PermissionDenied.into());
    }
    let mut metadata = fs::metadata(&path).await?;
    if metadata.is_dir() {
        path = fs::canonicalize(path.join("index.html")).await?;
        if !path.starts_with(root) {
            return Err(io::ErrorKind::PermissionDenied.into());
        }
        metadata = fs::metadata(&path).await?;
    }
    if !metadata.is_file() {
        return Err(io::ErrorKind::NotFound.into());
    }
    Ok(path)
}

fn file_error(error: io::Error) -> StatusCode {
    match error.kind() {
        io::ErrorKind::NotFound
        | io::ErrorKind::NotADirectory
        | io::ErrorKind::PermissionDenied => StatusCode::NOT_FOUND,
        _ => {
            tracing::error!(%error, "failed to read static frontend file");
            StatusCode::INTERNAL_SERVER_ERROR
        }
    }
}
