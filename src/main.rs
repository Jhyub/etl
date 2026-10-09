mod api;
mod models;

use std::{
    fs,
    io::{self, IsTerminal, Write},
    path::{Path, PathBuf},
    process::ExitCode,
};

use api::{ApiError, Client};
use clap::{Parser, Subcommand};
use models::Assignment;
use serde::Serialize;

#[derive(Debug, Parser)]
#[command(name = "etl", version, about = "A personal CLI for SNU eTL")]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Debug, Subcommand)]
enum Command {
    /// List active student courses.
    Courses {
        #[arg(long)]
        json: bool,
    },
    /// List assignments in a course.
    Assignments {
        #[arg(long)]
        course_id: u64,
        #[arg(long)]
        json: bool,
    },
    /// Upload a file and submit it to an assignment.
    Submit {
        #[arg(long)]
        course_id: u64,
        #[arg(long)]
        assignment_id: u64,
        #[arg(long)]
        file: PathBuf,
        /// Filename to show in eTL. The local file is not renamed.
        #[arg(long)]
        filename: Option<String>,
        /// Validate the request without uploading or submitting.
        #[arg(long)]
        dry_run: bool,
        /// Submit without an interactive confirmation prompt.
        #[arg(long, conflicts_with = "dry_run")]
        yes: bool,
        /// Permit a new attempt when a previous submission already exists.
        #[arg(long)]
        resubmit: bool,
        #[arg(long)]
        json: bool,
    },
}

#[derive(Debug, thiserror::Error)]
enum AppError {
    #[error("{0}")]
    Validation(String),
    #[error(transparent)]
    Api(#[from] ApiError),
    #[error("could not read local file: {0}")]
    Io(#[from] std::io::Error),
    #[error("could not format JSON output: {0}")]
    Json(#[from] serde_json::Error),
}

impl AppError {
    fn exit_code(&self) -> i32 {
        match self {
            Self::Validation(_) | Self::Io(_) => 2,
            Self::Api(error) => error.exit_code(),
            Self::Json(_) => 4,
        }
    }
}

#[derive(Serialize)]
struct SubmitOutput<'a> {
    status: &'a str,
    course_id: u64,
    assignment_id: u64,
    assignment_name: &'a str,
    file: String,
    filename: &'a str,
    submitted_at: Option<&'a str>,
    attempt: Option<u32>,
}

fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("error: {error}");
            ExitCode::from(error.exit_code() as u8)
        }
    }
}

fn run() -> Result<(), AppError> {
    let cli = Cli::parse();
    let api = Client::from_env()?;

    match cli.command {
        Command::Courses { json } => list_courses(&api, json),
        Command::Assignments { course_id, json } => list_assignments(&api, course_id, json),
        Command::Submit {
            course_id,
            assignment_id,
            file,
            filename,
            dry_run,
            yes,
            resubmit,
            json,
        } => submit(
            &api,
            course_id,
            assignment_id,
            &file,
            filename.as_deref(),
            dry_run,
            yes,
            resubmit,
            json,
        ),
    }
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
    file_path: &Path,
    requested_filename: Option<&str>,
    dry_run: bool,
    yes: bool,
    resubmit: bool,
    json: bool,
) -> Result<(), AppError> {
    let metadata = fs::metadata(file_path)?;
    if !metadata.is_file() {
        return Err(AppError::Validation(format!(
            "{} is not a regular file",
            file_path.display()
        )));
    }
    let local_filename = file_path
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or_else(|| AppError::Validation("file path has no valid filename".to_owned()))?;
    let remote_filename = requested_filename.unwrap_or(local_filename);
    validate_remote_filename(remote_filename)?;

    let assignment = api.get_assignment(course_id, assignment_id)?;
    validate_assignment(&assignment, remote_filename, resubmit)?;
    let content_type = content_type_for(remote_filename);

    if dry_run {
        if json {
            println!(
                "{}",
                serde_json::to_string_pretty(&serde_json::json!({
                    "status": "dry_run",
                    "course_id": course_id,
                    "assignment_id": assignment_id,
                    "assignment_name": assignment.name,
                    "file": file_path,
                    "filename": remote_filename,
                    "size": metadata.len(),
                    "content_type": content_type,
                    "due_at": assignment.due_at,
                    "allowed_extensions": assignment.allowed_extensions,
                }))?
            );
        } else {
            print_submit_preview(
                course_id,
                assignment_id,
                &assignment,
                file_path,
                remote_filename,
                metadata.len(),
            );
            println!("Dry run: no file was uploaded and no submission was made.");
        }
        return Ok(());
    }

    if !yes && !confirm_submission(course_id, assignment_id, &assignment, remote_filename)? {
        println!("Cancelled; no file was uploaded.");
        return Ok(());
    }

    // Re-read after confirmation and before any write, so a concurrent manual
    // submission cannot silently become a duplicate attempt.
    let assignment = api.get_assignment(course_id, assignment_id)?;
    validate_assignment(&assignment, remote_filename, resubmit)?;

    let file_id = api.upload_submission_file(
        course_id,
        assignment_id,
        file_path,
        remote_filename,
        metadata.len(),
        content_type,
    )?;

    // The upload itself does not create a submission. Check once more before
    // the single submission POST; never automatically repeat that POST.
    let before_submit = api.get_assignment(course_id, assignment_id)?;
    validate_assignment(&before_submit, remote_filename, resubmit)?;
    api.submit_file_id(course_id, assignment_id, file_id)?;

    let verified = api.get_assignment(course_id, assignment_id)?;
    let submission = verified.current_submission.as_ref();
    let is_verified = submission.is_some_and(|submission| {
        submission.submitted_at.is_some()
            && submission.attachments.iter().any(|attachment| {
                attachment.display_name.as_deref() == Some(remote_filename)
                    && attachment.id == Some(file_id)
            })
    });
    if !is_verified {
        return Err(AppError::Validation(
            "the submission request was sent once, but eTL did not return a matching receipt; do not retry automatically".to_owned(),
        ));
    }

    let submission = submission.expect("verified submission exists");
    let output = SubmitOutput {
        status: "submitted",
        course_id,
        assignment_id,
        assignment_name: &verified.name,
        file: file_path.display().to_string(),
        filename: remote_filename,
        submitted_at: submission.submitted_at.as_deref(),
        attempt: submission.attempt,
    };
    if json {
        println!("{}", serde_json::to_string_pretty(&output)?);
    } else {
        println!(
            "Submitted {} to {} (course {}, assignment {}, attempt {}).",
            remote_filename,
            verified.name,
            course_id,
            assignment_id,
            submission
                .attempt
                .map(|attempt| attempt.to_string())
                .unwrap_or_else(|| "unknown".to_owned())
        );
        if let Some(submitted_at) = &submission.submitted_at {
            println!("Submitted at: {submitted_at}");
        }
    }
    Ok(())
}

fn validate_assignment(
    assignment: &Assignment,
    remote_filename: &str,
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
        let extension = Path::new(remote_filename)
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
                "filename extension .{extension} is not accepted; allowed extensions: {}",
                allowed.join(", ")
            )));
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
    filename: &str,
) -> Result<bool, AppError> {
    if !io::stdin().is_terminal() {
        return Err(AppError::Validation(
            "non-interactive submission requires --yes".to_owned(),
        ));
    }
    eprintln!(
        "Submit {filename} to {} (course {course_id}, assignment {assignment_id})? [y/N]",
        assignment.name
    );
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
    file_path: &Path,
    remote_filename: &str,
    size: u64,
) {
    println!("Course ID: {course_id}");
    println!("Assignment: {} ({assignment_id})", assignment.name);
    println!("Due: {}", display_opt(assignment.due_at.as_deref()));
    println!("Local file: {} ({size} bytes)", file_path.display());
    println!("Uploaded filename: {remote_filename}");
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
