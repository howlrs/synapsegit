use axum::extract::Multipart;
use axum::http::StatusCode;
use std::io;
use std::path::PathBuf;
use synapse_local_service::BeginCreatorSessionRequest;
use tokio::io::AsyncWriteExt;

use crate::app::random_hex;
use crate::handlers::has_exact_part_content_type;
use crate::state::AppState;
use crate::views::HttpFailure;

pub(crate) const MAX_CREATOR_FILE_BYTES: usize = 64 * 1024 * 1024;
pub(crate) const MAX_CREATOR_FILE_AGGREGATE_BYTES: usize = 3 * MAX_CREATOR_FILE_BYTES;
const MAX_SESSION_BYTES: usize = 64;
const MAX_SUBJECT_LABEL_BYTES: usize = 500;
const MAX_CREATOR_NAME_BYTES: usize = 300;

pub(crate) struct StagingDirectory {
    pub(crate) path: PathBuf,
}

impl StagingDirectory {
    pub(crate) async fn create() -> io::Result<Self> {
        Self::create_with(Self::create_sync).await
    }

    pub(crate) async fn create_with<F>(operation: F) -> io::Result<Self>
    where
        F: FnOnce() -> io::Result<Self> + Send + 'static,
    {
        // Dropping this JoinHandle detaches the short task, but its returned
        // StagingDirectory is still dropped when the task completes. Thus a
        // cancellation cannot separate successful creation from its owner.
        tokio::task::spawn_blocking(operation)
            .await
            .map_err(io::Error::other)?
    }

    pub(crate) fn create_sync() -> io::Result<Self> {
        for _ in 0..8 {
            let suffix = random_hex(16).map_err(io::Error::other)?;
            let path = std::env::temp_dir().join(format!("synapse-local-upload-{suffix}"));
            let mut builder = std::fs::DirBuilder::new();
            #[cfg(unix)]
            std::os::unix::fs::DirBuilderExt::mode(&mut builder, 0o700);
            match builder.create(&path) {
                Ok(()) => {
                    let directory = Self { path };
                    #[cfg(unix)]
                    std::fs::set_permissions(
                        &directory.path,
                        <std::fs::Permissions as std::os::unix::fs::PermissionsExt>::from_mode(
                            0o700,
                        ),
                    )?;
                    return Ok(directory);
                }
                Err(error) if error.kind() == io::ErrorKind::AlreadyExists => continue,
                Err(error) => return Err(error),
            }
        }
        Err(io::Error::new(
            io::ErrorKind::AlreadyExists,
            "could not allocate a unique upload directory",
        ))
    }

    fn file(&self, name: &str) -> PathBuf {
        self.path.join(name)
    }
}

impl Drop for StagingDirectory {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.path);
    }
}

pub(crate) struct StagedCreatorUpload {
    pub(crate) _directory: StagingDirectory,
    pub(crate) request: BeginCreatorSessionRequest,
}

impl StagedCreatorUpload {
    pub(crate) async fn from_multipart(mut multipart: Multipart) -> Result<Self, UploadFailure> {
        let directory = StagingDirectory::create()
            .await
            .map_err(|_| UploadFailure::Storage)?;
        let mut session = None;
        let mut subject_label = None;
        let mut creator_name = None;
        let mut original_image = None;
        let mut current_image = None;
        let mut ai_output = None;
        let mut aggregate_file_bytes = 0_usize;
        let mut part_count = 0_usize;

        while let Some(field) = multipart
            .next_field()
            .await
            .map_err(UploadFailure::multipart)?
        {
            part_count += 1;
            if part_count > 6 {
                return Err(UploadFailure::Request(
                    "The multipart request must contain exactly six fields.",
                ));
            }
            let name = field.name().ok_or(UploadFailure::Request(
                "Every multipart field must have a name.",
            ))?;
            match name {
                "session" if session.is_none() => {
                    session = Some(read_text_part(field, MAX_SESSION_BYTES).await?);
                }
                "subject_label" if subject_label.is_none() => {
                    subject_label = Some(read_text_part(field, MAX_SUBJECT_LABEL_BYTES).await?);
                }
                "creator_name" if creator_name.is_none() => {
                    creator_name = Some(read_text_part(field, MAX_CREATOR_NAME_BYTES).await?);
                }
                "original_image" if original_image.is_none() => {
                    let path = directory.file("original-image.bin");
                    write_file_part(field, &path, &mut aggregate_file_bytes).await?;
                    original_image = Some(path);
                }
                "current_image" if current_image.is_none() => {
                    let path = directory.file("current-image.bin");
                    write_file_part(field, &path, &mut aggregate_file_bytes).await?;
                    current_image = Some(path);
                }
                "ai_output" if ai_output.is_none() => {
                    let path = directory.file("ai-output.bin");
                    write_file_part(field, &path, &mut aggregate_file_bytes).await?;
                    ai_output = Some(path);
                }
                "session" | "subject_label" | "creator_name" | "original_image"
                | "current_image" | "ai_output" => {
                    return Err(UploadFailure::Request(
                        "Multipart fields must not be duplicated.",
                    ));
                }
                _ => {
                    return Err(UploadFailure::Request(
                        "The multipart request contains an unknown field.",
                    ));
                }
            }
        }

        let session = session.ok_or(UploadFailure::Request(
            "The multipart request is missing a required field.",
        ))?;
        let subject_label = subject_label.ok_or(UploadFailure::Request(
            "The multipart request is missing a required field.",
        ))?;
        let creator_name = creator_name.ok_or(UploadFailure::Request(
            "The multipart request is missing a required field.",
        ))?;
        let original_image = original_image.ok_or(UploadFailure::Request(
            "The multipart request is missing a required field.",
        ))?;
        let current_image = current_image.ok_or(UploadFailure::Request(
            "The multipart request is missing a required field.",
        ))?;
        let ai_output = ai_output.ok_or(UploadFailure::Request(
            "The multipart request is missing a required field.",
        ))?;

        if !is_session_slug(&session) || subject_label.is_empty() || creator_name.is_empty() {
            return Err(UploadFailure::Request(
                "The multipart text fields do not satisfy the API schema.",
            ));
        }

        Ok(Self {
            _directory: directory,
            request: BeginCreatorSessionRequest {
                session,
                subject_label,
                creator_name,
                original_image,
                current_image,
                ai_output,
            },
        })
    }
}

async fn read_text_part(
    mut field: axum::extract::multipart::Field<'_>,
    max_bytes: usize,
) -> Result<String, UploadFailure> {
    if !has_exact_part_content_type(field.headers(), &mime::TEXT_PLAIN_UTF_8) {
        return Err(UploadFailure::Request(
            "Multipart text fields must use text/plain; charset=utf-8.",
        ));
    }
    let mut bytes = Vec::new();
    while let Some(chunk) = field.chunk().await.map_err(UploadFailure::multipart)? {
        let next_len = bytes
            .len()
            .checked_add(chunk.len())
            .ok_or(UploadFailure::Request(
                "A multipart text field exceeds its UTF-8 byte limit.",
            ))?;
        if next_len > max_bytes {
            return Err(UploadFailure::Request(
                "A multipart text field exceeds its UTF-8 byte limit.",
            ));
        }
        bytes.extend_from_slice(&chunk);
    }
    String::from_utf8(bytes)
        .map_err(|_| UploadFailure::Request("Multipart text fields must contain valid UTF-8."))
}

async fn write_file_part(
    mut field: axum::extract::multipart::Field<'_>,
    path: &PathBuf,
    aggregate_file_bytes: &mut usize,
) -> Result<(), UploadFailure> {
    if !has_exact_part_content_type(field.headers(), &mime::APPLICATION_OCTET_STREAM) {
        return Err(UploadFailure::Request(
            "Multipart file fields must use application/octet-stream.",
        ));
    }
    let mut options = tokio::fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    options.mode(0o600);
    let mut file = options
        .open(path)
        .await
        .map_err(|_| UploadFailure::Storage)?;
    #[cfg(unix)]
    tokio::fs::set_permissions(
        path,
        <std::fs::Permissions as std::os::unix::fs::PermissionsExt>::from_mode(0o600),
    )
    .await
    .map_err(|_| UploadFailure::Storage)?;

    let mut file_bytes = 0_usize;
    while let Some(chunk) = field.chunk().await.map_err(UploadFailure::multipart)? {
        file_bytes = file_bytes
            .checked_add(chunk.len())
            .ok_or(UploadFailure::Limit(
                "A creator file exceeds the 64 MiB limit.",
            ))?;
        let next_aggregate =
            aggregate_file_bytes
                .checked_add(chunk.len())
                .ok_or(UploadFailure::Limit(
                    "The creator files exceed the 192 MiB aggregate limit.",
                ))?;
        if file_bytes > MAX_CREATOR_FILE_BYTES {
            return Err(UploadFailure::Limit(
                "A creator file exceeds the 64 MiB limit.",
            ));
        }
        if next_aggregate > MAX_CREATOR_FILE_AGGREGATE_BYTES {
            return Err(UploadFailure::Limit(
                "The creator files exceed the 192 MiB aggregate limit.",
            ));
        }
        file.write_all(&chunk)
            .await
            .map_err(|_| UploadFailure::Storage)?;
        *aggregate_file_bytes = next_aggregate;
    }
    file.flush().await.map_err(|_| UploadFailure::Storage)
}

fn is_session_slug(value: &str) -> bool {
    let bytes = value.as_bytes();
    !bytes.is_empty()
        && bytes.len() <= MAX_SESSION_BYTES
        && bytes[0].is_ascii_lowercase()
        && bytes
            .iter()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || *byte == b'-')
}

pub(crate) enum UploadFailure {
    Request(&'static str),
    Limit(&'static str),
    Storage,
}

impl UploadFailure {
    fn multipart(error: axum::extract::multipart::MultipartError) -> Self {
        match error.status() {
            StatusCode::PAYLOAD_TOO_LARGE => {
                Self::Limit("The multipart request exceeds the configured wire or file limit.")
            }
            StatusCode::BAD_REQUEST => Self::Request("The multipart request is malformed."),
            _ => Self::Storage,
        }
    }

    pub(crate) fn into_http_failure(self, state: &AppState) -> HttpFailure {
        match self {
            Self::Request(detail) => HttpFailure::request(state, "local_request_denied", detail),
            Self::Limit(detail) => HttpFailure::limit(state, detail),
            Self::Storage => {
                HttpFailure::storage(state, "The upload staging area could not be written.")
            }
        }
    }
}
