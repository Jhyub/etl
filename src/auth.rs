use std::{
    env, fmt, fs, io,
    io::Write,
    path::{Path, PathBuf},
};

use directories::BaseDirs;
use tempfile::NamedTempFile;

#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;

#[derive(Debug, thiserror::Error)]
pub enum AuthError {
    #[error("no token configured; run `etl auth login` or set ETL_TOKEN")]
    MissingToken,
    #[error("could not determine the user data directory")]
    NoDataDirectory,
    #[error("ETL_TOKEN is not valid UTF-8")]
    InvalidEnvironment,
    #[error("token must be a single nonempty line")]
    InvalidTokenInput,
    #[error("credentials path is not a regular file or has unsafe permissions")]
    UnsafeCredentials,
    #[error("credentials file does not contain a single nonempty token")]
    MalformedCredentials,
    #[error("credential file error: {0}")]
    Io(#[from] io::Error),
}

impl AuthError {
    pub fn exit_code(&self) -> i32 {
        match self {
            Self::MissingToken | Self::InvalidEnvironment => 3,
            Self::InvalidTokenInput => 2,
            Self::NoDataDirectory
            | Self::UnsafeCredentials
            | Self::MalformedCredentials
            | Self::Io(_) => 4,
        }
    }
}

#[derive(Clone, Copy, Debug)]
pub enum TokenSource {
    Environment,
    CredentialsFile,
}

impl fmt::Display for TokenSource {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Environment => formatter.write_str("ETL_TOKEN"),
            Self::CredentialsFile => formatter.write_str("credentials file"),
        }
    }
}

pub struct ResolvedToken {
    pub value: String,
    pub source: TokenSource,
}

pub fn credentials_path() -> Result<PathBuf, AuthError> {
    let dirs = BaseDirs::new().ok_or(AuthError::NoDataDirectory)?;
    Ok(dirs.data_local_dir().join("etl").join("credentials"))
}

pub fn resolve_token() -> Result<ResolvedToken, AuthError> {
    let env_token = match env::var("ETL_TOKEN") {
        Ok(value) => Some(value),
        Err(env::VarError::NotPresent) => None,
        Err(env::VarError::NotUnicode(_)) => return Err(AuthError::InvalidEnvironment),
    };
    if let Some(value) = env_token.filter(|value| !value.trim().is_empty()) {
        return Ok(ResolvedToken {
            value,
            source: TokenSource::Environment,
        });
    }
    let value = read_credentials(&credentials_path()?)?.ok_or(AuthError::MissingToken)?;
    Ok(ResolvedToken {
        value,
        source: TokenSource::CredentialsFile,
    })
}

pub fn environment_override_present() -> bool {
    env::var("ETL_TOKEN")
        .ok()
        .is_some_and(|value| !value.trim().is_empty())
}

pub fn parse_token_input(raw: &str) -> Result<&str, AuthError> {
    let value = raw
        .strip_suffix("\r\n")
        .or_else(|| raw.strip_suffix('\n'))
        .unwrap_or(raw);
    if value.trim().is_empty() || value.chars().any(char::is_control) {
        return Err(AuthError::InvalidTokenInput);
    }
    Ok(value)
}

pub fn read_credentials(path: &Path) -> Result<Option<String>, AuthError> {
    let metadata = match fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error.into()),
    };
    if !metadata.file_type().is_file() {
        return Err(AuthError::UnsafeCredentials);
    }
    #[cfg(unix)]
    if metadata.permissions().mode() & 0o077 != 0 {
        return Err(AuthError::UnsafeCredentials);
    }
    let contents = fs::read_to_string(path)?;
    let token = parse_token_input(&contents).map_err(|_| AuthError::MalformedCredentials)?;
    Ok(Some(token.to_owned()))
}

pub fn save_credentials(path: &Path, token: &str) -> Result<(), AuthError> {
    parse_token_input(token)?;
    let directory = path.parent().ok_or(AuthError::NoDataDirectory)?;
    fs::create_dir_all(directory)?;
    if !fs::symlink_metadata(directory)?.file_type().is_dir() {
        return Err(AuthError::UnsafeCredentials);
    }
    #[cfg(unix)]
    fs::set_permissions(directory, fs::Permissions::from_mode(0o700))?;

    let mut temporary = NamedTempFile::new_in(directory)?;
    #[cfg(unix)]
    temporary
        .as_file()
        .set_permissions(fs::Permissions::from_mode(0o600))?;
    temporary.write_all(token.as_bytes())?;
    temporary.as_file().sync_all()?;
    temporary
        .persist(path)
        .map_err(|error| AuthError::Io(error.error))?;
    Ok(())
}

pub fn delete_credentials(path: &Path) -> Result<bool, AuthError> {
    match fs::remove_file(path) {
        Ok(()) => Ok(true),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(error.into()),
    }
}
