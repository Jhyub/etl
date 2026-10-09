mod api;
mod auth;
mod models;

use std::{
    fs,
    io::{self, IsTerminal, Read, Write},
    path::{Path, PathBuf},
    process::ExitCode,
};

use api::{ApiError, Client};
use auth::AuthError;
use clap::{Parser, Subcommand};
use models::Assignment;
use serde::Serialize;
use url::Url;

#[derive(Debug, Parser)]
#[command(name = "etl", version, about = "A personal CLI for SNU eTL")]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Debug, Subcommand)]
enum Command {
    /// Manage the saved eTL token.
    Auth {
        #[command(subcommand)]
        command: AuthCommand,
    },
    /// List active student courses.
    Courses {
        #[arg(long, short = 'j')]
        json: bool,
    },
    /// List assignments in a course.
    Assignments {
        #[arg(
            value_name = "ID",
            conflicts_with = "course_id_flag",
            required_unless_present = "course_id_flag"
        )]
        course_id: Option<u64>,
        #[arg(
            long = "course-id",
            short = 'c',
            value_name = "ID",
            conflicts_with = "course_id"
        )]
        course_id_flag: Option<u64>,
        #[arg(long, short = 'j')]
        json: bool,
    },
    /// Upload one or more files in a single assignment submission.
    Submit {
        #[arg(
            long,
            short = 'c',
            requires = "assignment_id",
            required_unless_present = "assignment_url"
        )]
        course_id: Option<u64>,
        #[arg(
            long,
            short = 'a',
            requires = "course_id",
            required_unless_present = "assignment_url"
        )]
        assignment_id: Option<u64>,
        /// Assignment page URL; its course and assignment IDs are extracted.
        #[arg(
            long,
            visible_alias = "url",
            short = 'u',
            value_name = "URL",
            conflicts_with_all = ["course_id", "assignment_id"]
        )]
        assignment_url: Option<String>,
        /// Local files to upload, in order.
        #[arg(value_name = "PATH", required = true, num_args = 1..)]
        paths: Vec<PathBuf>,
        /// Filenames to show in eTL, in PATH order. Supply one per PATH.
        #[arg(long = "filename", short = 'n', value_name = "NAME", num_args = 1..)]
        filenames: Vec<String>,
        /// Validate the request without uploading or submitting.
        #[arg(long, short = 'd')]
        dry_run: bool,
        /// Submit without an interactive confirmation prompt.
        #[arg(long, short = 'y', conflicts_with = "dry_run")]
        yes: bool,
        /// Permit a new attempt when a previous submission already exists.
        #[arg(long, short = 'r')]
        resubmit: bool,
        #[arg(long, short = 'j')]
        json: bool,
    },
}

#[derive(Debug, Subcommand)]
enum AuthCommand {
    /// Read a token from stdin, validate it, and save it.
    Login,
    /// Remove the saved token.
    Logout,
    /// Check the effective token with one read-only request.
    Status,
}

#[derive(Debug, thiserror::Error)]
enum AppError {
    #[error("{0}")]
    Validation(String),
    #[error(transparent)]
    Api(#[from] ApiError),
    #[error("upload of {file} failed; no assignment submission was sent: {source}")]
    Upload {
        file: String,
        #[source]
        source: ApiError,
    },
    #[error(transparent)]
    Auth(#[from] AuthError),
    #[error("could not read local file: {0}")]
    Io(#[from] std::io::Error),
    #[error("could not read local file {}: {source}", path.display())]
    FileIo {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("could not format JSON output: {0}")]
    Json(#[from] serde_json::Error),
    #[error("status already reported")]
    Reported(u8),
}

impl AppError {
    fn exit_code(&self) -> i32 {
        match self {
            Self::Validation(_) | Self::Io(_) | Self::FileIo { .. } => 2,
            Self::Api(error) => error.exit_code(),
            Self::Upload { source, .. } => source.exit_code(),
            Self::Auth(error) => error.exit_code(),
            Self::Json(_) => 4,
            Self::Reported(code) => i32::from(*code),
        }
    }
}

#[derive(Serialize)]
struct SubmitOutput<'a> {
    status: &'a str,
    course_id: u64,
    assignment_id: u64,
    assignment_name: &'a str,
    files: &'a [PreparedFile],
    submitted_at: Option<&'a str>,
    attempt: Option<u32>,
}

#[derive(Serialize)]
struct PreparedFile {
    #[serde(rename = "file")]
    path: PathBuf,
    #[serde(rename = "filename")]
    remote_filename: String,
    size: u64,
    content_type: &'static str,
}

fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(AppError::Reported(code)) => ExitCode::from(code),
        Err(error) => {
            eprintln!("error: {error}");
            ExitCode::from(error.exit_code() as u8)
        }
    }
}

fn run() -> Result<(), AppError> {
    let cli = Cli::parse();

    match cli.command {
        Command::Auth { command } => match command {
            AuthCommand::Login => auth_login(),
            AuthCommand::Logout => auth_logout(),
            AuthCommand::Status => auth_status(),
        },
        Command::Courses { json } => list_courses(&authenticated_client()?, json),
        Command::Assignments {
            course_id,
            course_id_flag,
            json,
        } => {
            let course_id = course_id.or(course_id_flag).ok_or_else(|| {
                AppError::Validation("provide an assignment course ID".to_owned())
            })?;
            list_assignments(&authenticated_client()?, course_id, json)
        }
        Command::Submit {
            course_id,
            assignment_id,
            assignment_url,
            paths,
            filenames,
            dry_run,
            yes,
            resubmit,
            json,
        } => {
            let (course_id, assignment_id) =
                resolve_destination(course_id, assignment_id, assignment_url.as_deref())?;
            submit(
                &authenticated_client()?,
                course_id,
                assignment_id,
                &paths,
                &filenames,
                dry_run,
                yes,
                resubmit,
                json,
            )
        }
    }
}

fn authenticated_client() -> Result<Client, AppError> {
    Ok(Client::with_token(auth::resolve_token()?.value)?)
}

fn read_login_token() -> Result<String, AppError> {
    let raw = if io::stdin().is_terminal() {
        rpassword::prompt_password("eTL token: ")?
    } else {
        let mut input = String::new();
        io::stdin().read_to_string(&mut input)?;
        input
    };
    Ok(auth::parse_token_input(&raw)?.to_owned())
}

fn auth_login() -> Result<(), AppError> {
    let token = read_login_token()?;
    let path = auth::credentials_path()?;
    Client::with_token(token.clone())?.validate_token()?;
    auth::save_credentials(&path, &token)?;
    println!("Token validated and saved.");
    if auth::environment_override_present() {
        println!("ETL_TOKEN is set and will continue to override the saved token.");
    }
    Ok(())
}

fn auth_logout() -> Result<(), AppError> {
    let removed = auth::delete_credentials(&auth::credentials_path()?)?;
    println!(
        "{}",
        if removed {
            "Saved token removed."
        } else {
            "No saved token was present."
        }
    );
    if auth::environment_override_present() {
        println!("ETL_TOKEN remains set and will continue to authenticate requests.");
    }
    Ok(())
}

fn auth_status() -> Result<(), AppError> {
    let resolved = match auth::resolve_token() {
        Ok(token) => token,
        Err(AuthError::MissingToken) => {
            println!("Token: absent.\nValidity: not checked.");
            return Err(AppError::Reported(3));
        }
        Err(error) => return Err(error.into()),
    };
    println!("Token: present (source: {}).", resolved.source);
    match Client::with_token(resolved.value)?.validate_token() {
        Ok(()) => println!("Validity: valid."),
        Err(ApiError::Auth(status)) => {
            println!("Validity: invalid (HTTP {status}).");
            return Err(AppError::Reported(3));
        }
        Err(error) => {
            println!("Validity: unavailable ({error}).");
            return Err(AppError::Reported(4));
        }
    }
    Ok(())
}

fn resolve_destination(
    course_id: Option<u64>,
    assignment_id: Option<u64>,
    assignment_url: Option<&str>,
) -> Result<(u64, u64), AppError> {
    match (course_id, assignment_id, assignment_url) {
        (Some(course_id), Some(assignment_id), None) => Ok((course_id, assignment_id)),
        (None, None, Some(assignment_url)) => parse_assignment_url(assignment_url),
        (Some(_), _, Some(_)) | (_, Some(_), Some(_)) => Err(AppError::Validation(
            "use either --url or both --course-id and --assignment-id, not both".to_owned(),
        )),
        (None, None, None) => Err(AppError::Validation(
            "provide --url or both --course-id and --assignment-id".to_owned(),
        )),
        _ => Err(AppError::Validation(
            "--course-id and --assignment-id must be provided together".to_owned(),
        )),
    }
}

fn parse_assignment_url(input: &str) -> Result<(u64, u64), AppError> {
    let url =
        Url::parse(input).map_err(|_| AppError::Validation("invalid assignment URL".to_owned()))?;
    if url.scheme() != "https"
        || url.host_str() != Some("myetl.snu.ac.kr")
        || url.port_or_known_default() != Some(443)
        || !url.username().is_empty()
        || url.password().is_some()
    {
        return Err(AppError::Validation(
            "assignment URL must use https://myetl.snu.ac.kr".to_owned(),
        ));
    }

    let path_segments = url
        .path_segments()
        .ok_or_else(|| AppError::Validation("invalid assignment URL path".to_owned()))?
        .collect::<Vec<_>>();
    let path_segments = path_segments.strip_suffix(&[""]).unwrap_or(&path_segments);
    if path_segments.len() != 4
        || path_segments[0] != "courses"
        || path_segments[2] != "assignments"
    {
        return Err(AppError::Validation(
            "assignment URL path must be /courses/{course_id}/assignments/{assignment_id}"
                .to_owned(),
        ));
    }

    let course_id = path_segments[1].parse::<u64>().map_err(|_| {
        AppError::Validation("course ID in assignment URL must be numeric".to_owned())
    })?;
    let assignment_id = path_segments[3].parse::<u64>().map_err(|_| {
        AppError::Validation("assignment ID in assignment URL must be numeric".to_owned())
    })?;
    Ok((course_id, assignment_id))
}

fn list_courses(api: &Client, json: bool) -> Result<(), AppError> {
    let courses = api.list_courses()?;
    if json {
        println!("{}", serde_json::to_string_pretty(&courses)?);
    } else {
        println!("ID\tCOURSE CODE\tNAME\tSTATE");
        for course in courses {
            println!(
                "{}\t{}\t{}\t{}",
                course.id,
                display_opt(course.course_code.as_deref()),
                course.name.as_deref().unwrap_or("(unnamed course)"),
                display_opt(course.workflow_state.as_deref())
            );
        }
    }
    Ok(())
}

fn list_assignments(api: &Client, course_id: u64, json: bool) -> Result<(), AppError> {
    let assignments = api.list_assignments(course_id)?;
    if json {
        println!("{}", serde_json::to_string_pretty(&assignments)?);
    } else {
        println!("ID\tNAME\tDUE\tTYPE\tEXTENSIONS\tSUBMISSION");
        for assignment in assignments {
            let types = assignment.submission_types.join(",");
            let extensions = assignment
                .allowed_extensions
                .as_ref()
                .map(|items| items.join(","))
                .unwrap_or_else(|| "any".to_owned());
            let state = assignment
                .current_submission
                .as_ref()
                .and_then(|submission| submission.workflow_state.as_deref())
                .unwrap_or("unknown");
            println!(
                "{}\t{}\t{}\t{}\t{}\t{}",
                assignment.id,
                assignment.name,
                display_opt(assignment.due_at.as_deref()),
                types,
                extensions,
                state
            );
        }
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn submit(
    api: &Client,
    course_id: u64,
    assignment_id: u64,
    paths: &[PathBuf],
    requested_filenames: &[String],
    dry_run: bool,
    yes: bool,
    resubmit: bool,
    json: bool,
) -> Result<(), AppError> {
    let files = prepare_files(paths, requested_filenames)?;
    let assignment = api.get_assignment(course_id, assignment_id)?;
    validate_assignment(&assignment, &files, resubmit)?;

    if dry_run {
        if json {
            println!(
                "{}",
                serde_json::to_string_pretty(&serde_json::json!({
                    "status": "dry_run",
                    "course_id": course_id,
                    "assignment_id": assignment_id,
                    "assignment_name": assignment.name,
                    "files": files,
                    "due_at": assignment.due_at,
                    "allowed_extensions": assignment.allowed_extensions,
                }))?
            );
        } else {
            print_submit_preview(course_id, assignment_id, &assignment, &files);
            println!("Dry run: no file was uploaded and no submission was made.");
        }
        return Ok(());
    }

    if !yes && !confirm_submission(course_id, assignment_id, &assignment, &files)? {
        println!("Cancelled; no file was uploaded.");
        return Ok(());
    }

    // Re-read after confirmation and before any write, so a concurrent manual
    // submission cannot silently become a duplicate attempt.
    let assignment = api.get_assignment(course_id, assignment_id)?;
    validate_assignment(&assignment, &files, resubmit)?;

    let mut file_ids = Vec::with_capacity(files.len());
    for file in &files {
        let file_id = api
            .upload_submission_file(
                course_id,
                assignment_id,
                &file.path,
                &file.remote_filename,
                file.size,
                file.content_type,
            )
            .map_err(|source| AppError::Upload {
                file: file.path.display().to_string(),
                source,
            })?;
        file_ids.push(file_id);
    }

    // The upload itself does not create a submission. Check once more before
    // the single submission POST; never automatically repeat that POST.
    let before_submit = api.get_assignment(course_id, assignment_id)?;
    validate_assignment(&before_submit, &files, resubmit)?;
    api.submit_file_ids(course_id, assignment_id, &file_ids)?;

    let verified = api.get_assignment(course_id, assignment_id)?;
    let submission = verified.current_submission.as_ref();
    let is_verified = submission.is_some_and(|submission| {
        submission.submitted_at.is_some()
            && files.iter().zip(&file_ids).all(|(file, file_id)| {
                submission.attachments.iter().any(|attachment| {
                    attachment.display_name.as_deref() == Some(file.remote_filename.as_str())
                        && attachment.id == Some(*file_id)
                })
            })
    });
    if !is_verified {
        return Err(AppError::Validation(
            "the submission request was sent once, but eTL did not return a receipt containing every file; do not retry automatically".to_owned(),
        ));
    }

    let submission = submission.expect("verified submission exists");
    let output = SubmitOutput {
        status: "submitted",
        course_id,
        assignment_id,
        assignment_name: &verified.name,
        files: &files,
        submitted_at: submission.submitted_at.as_deref(),
        attempt: submission.attempt,
    };
    if json {
        println!("{}", serde_json::to_string_pretty(&output)?);
    } else {
        println!(
            "Submitted {} file(s) to {} (course {}, assignment {}, attempt {}).",
            files.len(),
            verified.name,
            course_id,
            assignment_id,
            submission
                .attempt
                .map(|attempt| attempt.to_string())
                .unwrap_or_else(|| "unknown".to_owned())
        );
        for file in &files {
            println!("  {} -> {}", file.path.display(), file.remote_filename);
        }
        if let Some(submitted_at) = &submission.submitted_at {
            println!("Submitted at: {submitted_at}");
        }
    }
    Ok(())
}

fn prepare_files(
    paths: &[PathBuf],
    requested_filenames: &[String],
) -> Result<Vec<PreparedFile>, AppError> {
    if paths.is_empty() {
        return Err(AppError::Validation("provide at least one PATH".to_owned()));
    }
    if !requested_filenames.is_empty() && requested_filenames.len() != paths.len() {
        return Err(AppError::Validation(format!(
            "--filename requires one NAME per PATH ({} paths, {} names)",
            paths.len(),
            requested_filenames.len()
        )));
    }

    paths
        .iter()
        .enumerate()
        .map(|(index, path)| {
            let metadata = fs::metadata(path).map_err(|source| AppError::FileIo {
                path: path.clone(),
                source,
            })?;
            if !metadata.is_file() {
                return Err(AppError::Validation(format!(
                    "{} is not a regular file",
                    path.display()
                )));
            }
            fs::File::open(path).map_err(|source| AppError::FileIo {
                path: path.clone(),
                source,
            })?;
            let local_filename =
                path.file_name()
                    .and_then(|name| name.to_str())
                    .ok_or_else(|| {
                        AppError::Validation(format!("{} has no valid filename", path.display()))
                    })?;
            let remote_filename = requested_filenames
                .get(index)
                .map(String::as_str)
                .unwrap_or(local_filename);
            validate_remote_filename(remote_filename)?;
            Ok(PreparedFile {
                path: path.clone(),
                remote_filename: remote_filename.to_owned(),
                size: metadata.len(),
                content_type: content_type_for(remote_filename),
            })
        })
        .collect()
}

fn validate_assignment(
    assignment: &Assignment,
    files: &[PreparedFile],
    resubmit: bool,
) -> Result<(), AppError> {
    if !assignment
        .submission_types
        .iter()
        .any(|kind| kind == "online_upload")
    {
        return Err(AppError::Validation(format!(
            "assignment {} ({}) does not allow online file uploads",
            assignment.id, assignment.name
        )));
    }

    if let Some(allowed) = &assignment.allowed_extensions {
        for file in files {
            let extension = Path::new(&file.remote_filename)
                .extension()
                .and_then(|value| value.to_str())
                .unwrap_or_default()
                .to_ascii_lowercase();
            if !allowed.is_empty()
                && !allowed.iter().any(|candidate| {
                    candidate
                        .trim_start_matches('.')
                        .eq_ignore_ascii_case(&extension)
                })
            {
                return Err(AppError::Validation(format!(
                    "{} has an unaccepted extension .{extension}; allowed extensions: {}",
                    file.remote_filename,
                    allowed.join(", ")
                )));
            }
        }
    }

    if !resubmit && submission_already_exists(assignment) {
        return Err(AppError::Validation(format!(
            "assignment {} already has a submission; refusing to create another attempt (use --resubmit only when intended)",
            assignment.id
        )));
    }
    Ok(())
}

fn submission_already_exists(assignment: &Assignment) -> bool {
    assignment
        .current_submission
        .as_ref()
        .is_some_and(|submission| {
            submission.submitted_at.is_some()
                || submission.attempt.unwrap_or_default() > 0
                || submission.attachments.iter().any(|file| file.id.is_some())
                || matches!(
                    submission.workflow_state.as_deref(),
                    Some("submitted" | "graded" | "pending_review")
                )
        })
}

fn validate_remote_filename(filename: &str) -> Result<(), AppError> {
    if filename.trim().is_empty()
        || filename == "."
        || filename == ".."
        || filename.contains('/')
        || filename.contains('\\')
        || filename.chars().any(char::is_control)
    {
        return Err(AppError::Validation(
            "--filename must be a nonempty filename without path separators or control characters"
                .to_owned(),
        ));
    }
    Ok(())
}

fn content_type_for(filename: &str) -> &'static str {
    match Path::new(filename)
        .extension()
        .and_then(|value| value.to_str())
        .unwrap_or_default()
        .to_ascii_lowercase()
        .as_str()
    {
        "zip" => "application/zip",
        "pdf" => "application/pdf",
        "txt" => "text/plain",
        "doc" => "application/msword",
        "docx" => "application/vnd.openxmlformats-officedocument.wordprocessingml.document",
        _ => "application/octet-stream",
    }
}

fn confirm_submission(
    course_id: u64,
    assignment_id: u64,
    assignment: &Assignment,
    files: &[PreparedFile],
) -> Result<bool, AppError> {
    if !io::stdin().is_terminal() {
        return Err(AppError::Validation(
            "non-interactive submission requires --yes".to_owned(),
        ));
    }
    eprintln!(
        "Submit {} file(s) to {} (course {course_id}, assignment {assignment_id})? [y/N]",
        files.len(),
        assignment.name
    );
    for file in files {
        eprintln!("  {} -> {}", file.path.display(), file.remote_filename);
    }
    eprint!("Confirm: ");
    io::stderr().flush()?;
    let mut answer = String::new();
    io::stdin().read_line(&mut answer)?;
    Ok(matches!(
        answer.trim().to_ascii_lowercase().as_str(),
        "y" | "yes"
    ))
}

fn print_submit_preview(
    course_id: u64,
    assignment_id: u64,
    assignment: &Assignment,
    files: &[PreparedFile],
) {
    println!("Course ID: {course_id}");
    println!("Assignment: {} ({assignment_id})", assignment.name);
    println!("Due: {}", display_opt(assignment.due_at.as_deref()));
    println!("Files:");
    for file in files {
        println!(
            "  {} ({} bytes) -> {}",
            file.path.display(),
            file.size,
            file.remote_filename
        );
    }
    println!("Allowed extensions: {}", extension_list(assignment));
}

fn extension_list(assignment: &Assignment) -> String {
    assignment
        .allowed_extensions
        .as_ref()
        .map(|items| items.join(", "))
        .unwrap_or_else(|| "any".to_owned())
}

fn display_opt(value: Option<&str>) -> &str {
    value.unwrap_or("-")
}

#[cfg(test)]
mod tests {
    use super::*;

    const EXAMPLE_URL: &str = "https://myetl.snu.ac.kr/courses/305887/assignments/381293";

    #[test]
    fn parses_assignment_page_url() {
        assert_eq!(parse_assignment_url(EXAMPLE_URL).unwrap(), (305887, 381293));
    }

    #[test]
    fn accepts_trailing_slash_query_and_fragment() {
        let url = format!("{EXAMPLE_URL}/?module_item_id=42#details");
        assert_eq!(parse_assignment_url(&url).unwrap(), (305887, 381293));
    }

    #[test]
    fn rejects_urls_outside_the_expected_assignment_route() {
        let invalid_urls = [
            "http://myetl.snu.ac.kr/courses/305887/assignments/381293",
            "https://example.com/courses/305887/assignments/381293",
            "https://myetl.snu.ac.kr.evil.test/courses/305887/assignments/381293",
            "https://myetl.snu.ac.kr/courses/nope/assignments/381293",
            "https://myetl.snu.ac.kr/courses/305887/assignments/nope",
            "https://myetl.snu.ac.kr/courses/305887/assignments/381293/submissions",
            "https://user@myetl.snu.ac.kr/courses/305887/assignments/381293",
            "https://myetl.snu.ac.kr:8443/courses/305887/assignments/381293",
        ];

        for url in invalid_urls {
            assert!(
                parse_assignment_url(url).is_err(),
                "accepted invalid URL: {url}"
            );
        }
    }

    #[test]
    fn cli_accepts_url_and_existing_id_destination_forms() {
        let url_cli = Cli::try_parse_from([
            "etl",
            "submit",
            "--url",
            EXAMPLE_URL,
            "hw.zip",
            "notes.pdf",
            "-n",
            "source.zip",
            "report.pdf",
        ])
        .unwrap();
        let Command::Submit {
            course_id,
            assignment_id,
            assignment_url,
            ..
        } = url_cli.command
        else {
            panic!("expected submit command");
        };
        assert_eq!(
            resolve_destination(course_id, assignment_id, assignment_url.as_deref()).unwrap(),
            (305887, 381293)
        );
        let ids_cli =
            Cli::try_parse_from(["etl", "submit", "-c", "305887", "-a", "381293", "hw.zip"])
                .unwrap();
        let Command::Submit {
            course_id,
            assignment_id,
            assignment_url,
            ..
        } = ids_cli.command
        else {
            panic!("expected submit command");
        };
        assert_eq!(
            resolve_destination(course_id, assignment_id, assignment_url.as_deref()).unwrap(),
            (305887, 381293)
        );
    }

    #[test]
    fn cli_rejects_mixed_or_incomplete_destination_arguments() {
        assert!(
            Cli::try_parse_from([
                "etl",
                "submit",
                "--assignment-url",
                EXAMPLE_URL,
                "--course-id",
                "305887",
                "hw.zip",
            ])
            .is_err()
        );
        assert!(
            Cli::try_parse_from(["etl", "submit", "--course-id", "305887", "hw.zip",]).is_err()
        );
        assert!(Cli::try_parse_from(["etl", "submit", "--file", "hw.zip"]).is_err());
    }
}
