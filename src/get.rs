use std::{
    fs,
    io::{self, Read, Write},
    path::{Path, PathBuf},
};

use serde::Serialize;
use tempfile::NamedTempFile;
use url::Url;

use crate::{api::ApiError, api::Client, models::RemoteFile};

#[derive(Debug, thiserror::Error)]
pub enum GetError {
    #[error("{0}")]
    Validation(String),
    #[error("file {file_id}: {source}")]
    Api {
        file_id: u64,
        #[source]
        source: ApiError,
    },
    #[error("file {file_id}: download stream failed")]
    Stream { file_id: u64 },
    #[error("file {file_id}: expected {expected} bytes but received {actual}")]
    SizeMismatch {
        file_id: u64,
        expected: u64,
        actual: u64,
    },
    #[error("could not save downloaded file: {0}")]
    Io(#[from] io::Error),
    #[error("could not format JSON output: {0}")]
    Json(#[from] serde_json::Error),
}

impl GetError {
    pub fn exit_code(&self) -> i32 {
        match self {
            Self::Validation(_) | Self::Io(_) => 2,
            Self::Api { source, .. } => source.exit_code(),
            Self::Stream { .. } | Self::SizeMismatch { .. } | Self::Json(_) => 4,
        }
    }
}

#[derive(Clone, Copy, Debug)]
struct FileReference {
    course_id: Option<u64>,
    file_id: u64,
}

#[derive(Serialize)]
struct GetOutput {
    files: Vec<DownloadedFile>,
}

#[derive(Serialize)]
struct DownloadedFile {
    file_id: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    course_id: Option<u64>,
    saved_path: PathBuf,
    bytes: u64,
}

pub fn run_get(
    api: &Client,
    urls: &[String],
    output_dir: Option<&Path>,
    json: bool,
) -> Result<(), GetError> {
    if urls.is_empty() {
        return Err(GetError::Validation(
            "provide at least one file URL".to_owned(),
        ));
    }

    // Validate every input before downloading the first file.
    let files = urls
        .iter()
        .map(|url| parse_file_url(url))
        .collect::<Result<Vec<_>, _>>()?;

    let requested_dir = output_dir.unwrap_or_else(|| Path::new("."));
    fs::create_dir_all(requested_dir)?;
    let destination_dir = fs::canonicalize(requested_dir)?;
    if !destination_dir.is_dir() {
        return Err(GetError::Validation(
            "output path is not a directory".to_owned(),
        ));
    }

    let mut downloaded = Vec::with_capacity(files.len());
    for file in files {
        let result = download_one(api, file, &destination_dir)?;
        if json {
            eprintln!(
                "Saved file {} to {} ({} bytes)",
                result.file_id,
                result.saved_path.display(),
                result.bytes
            );
        } else {
            println!(
                "Saved file {} to {} ({} bytes)",
                result.file_id,
                result.saved_path.display(),
                result.bytes
            );
        }
        downloaded.push(result);
    }

    if json {
        println!(
            "{}",
            serde_json::to_string_pretty(&GetOutput { files: downloaded })?
        );
    }
    Ok(())
}

fn parse_file_url(input: &str) -> Result<FileReference, GetError> {
    let url =
        Url::parse(input).map_err(|_| GetError::Validation("invalid eTL file URL".to_owned()))?;
    if url.scheme() != "https"
        || url.host_str() != Some("myetl.snu.ac.kr")
        || url.port_or_known_default() != Some(443)
        || !url.username().is_empty()
        || url.password().is_some()
    {
        return Err(GetError::Validation(
            "file URL must use https://myetl.snu.ac.kr".to_owned(),
        ));
    }

    let segments = url
        .path_segments()
        .ok_or_else(|| GetError::Validation("invalid eTL file URL path".to_owned()))?
        .collect::<Vec<_>>();
    let segments = segments.strip_suffix(&[""]).unwrap_or(&segments);

    let (course_id, file_id) = match segments {
        ["courses", course_id, "files", file_id]
        | ["courses", course_id, "files", file_id, "download"] => (
            Some(parse_id(course_id, "course")?),
            parse_id(file_id, "file")?,
        ),
        ["files", file_id] | ["files", file_id, "download"] => (None, parse_id(file_id, "file")?),
        _ => {
            return Err(GetError::Validation(
                "file URL path must be /courses/{course_id}/files/{file_id}[/download] or /files/{file_id}[/download]".to_owned(),
            ));
        }
    };

    Ok(FileReference { course_id, file_id })
}

fn parse_id(value: &str, kind: &str) -> Result<u64, GetError> {
    let id = value
        .parse::<u64>()
        .map_err(|_| GetError::Validation(format!("{kind} ID in file URL must be numeric")))?;
    if id == 0 {
        return Err(GetError::Validation(format!(
            "{kind} ID in file URL must be positive"
        )));
    }
    Ok(id)
}

fn download_one(
    api: &Client,
    reference: FileReference,
    destination_dir: &Path,
) -> Result<DownloadedFile, GetError> {
    let file = api
        .get_file(reference.course_id, reference.file_id)
        .map_err(|source| GetError::Api {
            file_id: reference.file_id,
            source,
        })?;
    if file.id != reference.file_id {
        return Err(GetError::Api {
            file_id: reference.file_id,
            source: ApiError::InvalidResponse(
                "file metadata ID did not match the requested ID".to_owned(),
            ),
        });
    }

    let filename = choose_filename(&file).ok_or_else(|| GetError::Api {
        file_id: reference.file_id,
        source: ApiError::InvalidResponse(
            "file metadata did not contain a safe filename".to_owned(),
        ),
    })?;
    let download_url = file.url.as_deref().ok_or_else(|| GetError::Api {
        file_id: reference.file_id,
        source: ApiError::InvalidResponse(
            "file metadata did not contain a download URL".to_owned(),
        ),
    })?;
    let mut response = api
        .open_download(download_url)
        .map_err(|source| GetError::Api {
            file_id: reference.file_id,
            source,
        })?;

    let mut temporary = NamedTempFile::new_in(destination_dir)?;
    let mut buffer = [0u8; 64 * 1024];
    let mut bytes = 0u64;
    loop {
        let count = response.read(&mut buffer).map_err(|_| GetError::Stream {
            file_id: reference.file_id,
        })?;
        if count == 0 {
            break;
        }
        bytes = bytes.saturating_add(count as u64);
        if let Some(expected) = file.size {
            if bytes > expected {
                return Err(GetError::SizeMismatch {
                    file_id: reference.file_id,
                    expected,
                    actual: bytes,
                });
            }
        }
        temporary.write_all(&buffer[..count])?;
    }

    if let Some(expected) = file.size {
        if bytes != expected {
            return Err(GetError::SizeMismatch {
                file_id: reference.file_id,
                expected,
                actual: bytes,
            });
        }
    }
    temporary.as_file().sync_all()?;
    let saved_path = persist_without_overwrite(temporary, destination_dir, filename)?;

    Ok(DownloadedFile {
        file_id: reference.file_id,
        course_id: reference.course_id,
        saved_path,
        bytes,
    })
}

fn choose_filename(file: &RemoteFile) -> Option<&str> {
    file.display_name
        .as_deref()
        .filter(|name| is_safe_filename(name))
        .or_else(|| {
            file.filename
                .as_deref()
                .filter(|name| is_safe_filename(name))
        })
}

fn is_safe_filename(name: &str) -> bool {
    if name.is_empty()
        || name.trim().is_empty()
        || name == "."
        || name == ".."
        || name.ends_with([' ', '.'])
        || name
            .chars()
            .any(|character| character.is_control() || "<>:\"/\\|?*".contains(character))
    {
        return false;
    }

    let device_name = name
        .split('.')
        .next()
        .unwrap_or_default()
        .to_ascii_uppercase();
    if matches!(device_name.as_str(), "CON" | "PRN" | "AUX" | "NUL") {
        return false;
    }
    if let Some(number) = device_name
        .strip_prefix("COM")
        .or_else(|| device_name.strip_prefix("LPT"))
        .and_then(|number| number.parse::<u8>().ok())
        && (1..=9).contains(&number)
    {
        return false;
    }
    true
}

fn persist_without_overwrite(
    mut temporary: NamedTempFile,
    destination_dir: &Path,
    filename: &str,
) -> Result<PathBuf, GetError> {
    let mut suffix = 0u64;
    loop {
        let candidate_name = numbered_filename(filename, suffix);
        let candidate = destination_dir.join(candidate_name);
        match temporary.persist_noclobber(&candidate) {
            Ok(_) => return Ok(candidate),
            Err(error) if error.error.kind() == io::ErrorKind::AlreadyExists => {
                temporary = error.file;
                suffix = suffix.checked_add(1).ok_or_else(|| {
                    GetError::Validation("too many filename collisions".to_owned())
                })?;
            }
            Err(error) => return Err(GetError::Io(error.error)),
        }
    }
}

fn numbered_filename(filename: &str, suffix: u64) -> String {
    if suffix == 0 {
        return filename.to_owned();
    }
    match filename.rfind('.') {
        Some(index) if index > 0 => {
            format!("{} ({suffix}){}", &filename[..index], &filename[index..])
        }
        _ => format!("{filename} ({suffix})"),
    }
}
