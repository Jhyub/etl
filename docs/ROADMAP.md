# SNU eTL CLI: Research and Roadmap

- **Status:** Phases 0–2 and 2.5 implemented; the submission flow and download endpoint were verified live
- **Research date:** 2026-10-10
- **Target:** a personal CLI for using SNU's new eTL (`myetl.snu.ac.kr`)

## Goal

Build a personal CLI for eTL that supports:

1. Scriptable course and assignment discovery, file downloads, and assignment submissions.
2. An interactive terminal flow that lets the student choose a course, assignment, file in the current working directory, and the filename shown to the instructor.

Official OAuth login is out of scope. The CLI must not ask for or store a MySNU password.

## Research findings

### SNU access and assignment behavior

SNU's Information Service describes new eTL as its online learning platform and gives the normal access route as logging into MySNU and opening eTL. Direct access to `myetl.snu.ac.kr` currently redirects to an SSO login page. This confirms the normal human login path, but does not establish which non-OAuth API authentication methods the SNU installation permits. [SNU Information Service: eTL](https://ist.snu.ac.kr/%EC%9D%B4%EB%9F%AC%EB%8B%9D/) · [myetl.snu.ac.kr](https://myetl.snu.ac.kr/)

The LearningX student guide describes assignment requirements as instructor-configured. An assignment may allow file upload, text, or URL submission; availability dates, accepted file types, and resubmission limits can vary. The student-facing guide says to review the requirements, upload the file, submit, and check the resulting submission details. The CLI should therefore only offer file submission when the assignment allows online uploads and should show relevant dates and constraints before sending. [SNU new eTL learner guide](https://www.ontactlearning.com/snu_help/student) · [LearningX guide: submitting an assignment](https://www.ontactlearning.com/b27c670f-fd50-4932-9594-7fc997d78c00)

### Canvas API workflow (standard Canvas behavior; end-to-end confirmed for PA1)

Canvas documents endpoints for listing a user's courses and a course's assignments. Assignment records can include the current submission, due and lock dates, submission types, and allowed extensions. These are enough to drive the course and assignment selectors and preflight checks if the SNU instance exposes the standard API. [Courses API](https://canvas.instructure.com/doc/api/courses.html) · [Assignments API](https://canvas.instructure.com/doc/api/assignments.html)

Canvas also documents course-scoped and global file metadata endpoints, plus download routes that redirect to file storage. The saved token at `~/.local/share/etl/credentials` accessed metadata for course `305832`, file `9222279` with HTTP 200. A one-byte range request to the returned HTTPS URL returned HTTP 206 after redirecting to `kr.object.gov-ncloudstorage.com`. No file was saved during this check. The CLI must not send its bearer token to that storage URL. [Canvas Files API](https://developerdocs.instructure.com/services/canvas/resources/files.md)

For an online file assignment, Canvas documents a multi-step flow:

1. Start an upload through the assignment's submission-file endpoint, passing the intended upload name and file metadata.
2. POST the returned upload parameters and file bytes to the returned upload URL. Copy the server-provided parameters exactly; do not send the Canvas authorization token to the storage URL.
3. Complete the upload using the returned redirect/location as documented.
4. Submit the resulting file ID using `submission[submission_type]=online_upload` and `submission[file_ids][]`.
5. Read the current submission and report success only after confirming the submitted attempt and attachment.

Canvas warns that omitting the completion request can leave the file unavailable. A submission is a state-changing operation and can create a new attempt, so an ambiguous timeout must be reconciled by reading submission state before retrying. [Canvas file upload API](https://canvas.instructure.com/doc/api/file.file_uploads.html) · [Canvas submissions API](https://canvas.instructure.com/doc/api/submissions.html)

The documented submission API only accepts a submission type allowed by that assignment. Some assignment types, such as external tools, are not equivalent to a normal file upload; the MVP should reject them with a clear explanation instead of attempting a generic submission. [Canvas submissions API](https://canvas.instructure.com/doc/api/submissions.html)

### Authentication constraint and feasibility gate

Canvas's API accepts a bearer access token. Its documentation describes manually generated access tokens as a way to test an application before OAuth is implemented, says tokens are password-equivalent, and says applications used by multiple users must use OAuth. This project excludes OAuth and is scoped to the user's personal account. The supplied token worked for that account, but this does not establish whether SNU allows general token generation or broader use. [Canvas authentication and manual token generation](https://canvas.instructure.com/doc/api/file.oauth.html)

The token the user provided was accepted by `GET /api/v1/courses` and the course assignment list. It was also used for the single, explicitly authorized PA1 submission described below. This confirms the API flow for this account and assignment; it does not establish SNU's general policy for API tokens. The CLI accepts a saved personal token or `ETL_TOKEN`; it does not parse `.env` files. Do not build around guessed SSO endpoints, store the MySNU password, or extract browser cookies.

Do not distribute this as a multi-user service or ask classmates to create tokens for it. The Canvas documentation's policy boundary means the OAuth exclusion constrains the intended scope to personal use unless SNU provides another approved authentication method.

## Proposed user experience

### Scriptable mode

Command name: `etl`.

```sh
etl auth login   # reads a token from stdin; prompts without echo on a terminal
etl auth status  # checks the active token with one read-only request
etl auth logout  # removes the saved token
etl courses -j
etl assignments 123 -j
etl assignments --course-id 123 --json
etl get 'https://myetl.snu.ac.kr/courses/123/files/456/download?wrap=1'
etl get 'https://myetl.snu.ac.kr/files/456/download' 'https://myetl.snu.ac.kr/files/789/download' -o ./downloads -j
etl submit -c 123 -a 456 ./hw2.pdf -n 2025-12345_hw2.pdf -d
etl submit --url 'https://myetl.snu.ac.kr/courses/123/assignments/456' \
  ./report.pdf ./src.zip -n final-report.pdf source.zip -y
```

Requirements:

- Use numeric IDs in automation so duplicate or changing course titles cannot select the wrong destination.
- Use human-readable output by default and JSON via `--json` or `-j`; send diagnostics to stderr and define documented exit codes. Submission JSON uses a `files` array of `{file, filename, size, content_type}` objects, including for one file; the former top-level `file` and `filename` fields are removed.
- For submission, require one or more positional `PATH` arguments and either both numeric IDs or an assignment URL. `--url` and `-u` alias `--assignment-url`; the URL form extracts IDs from an HTTPS eTL assignment page and does not change the fixed API host. `--file` is no longer accepted.
- `--filename` or `-n` accepts space-separated names after the paths. Supply one name per path in order, or omit the option to use local basenames. `-c`, `-a`, `-d`, `-y`, `-r`, and `-j` abbreviate `--course-id`, `--assignment-id`, `--dry-run`, `--yes`, `--resubmit`, and `--json`. `--dry-run` validates without uploading; `--yes` suppresses interactive confirmation for scripts.
- Upload every file before making one final submission request with all file IDs. If any upload fails, no assignment submission is sent, although earlier uploaded files may remain in eTL.
- Never silently retry a submission after an uncertain response. First query the latest submission state and distinguish success from an unsubmitted upload.
- Report course and assignment names/IDs, every local path and remote filename, submission attempt, timestamp, and a link or status when available.

Authentication uses a single credentials file under the platform's local user data directory: `$XDG_DATA_HOME/etl/credentials` on Linux (default `~/.local/share/etl/credentials`), `~/Library/Application Support/etl/credentials` on macOS, and the `etl/credentials` directory under Local AppData on Windows. `etl auth login` validates the stdin token through one `GET /api/v1/courses?per_page=1` request before saving it. A piped token should contain one line; terminal input is hidden. On Unix, the app directory is mode `0700` and the file is mode `0600`.

A nonempty `ETL_TOKEN` overrides the saved token. If the variable is empty or absent, commands use the credentials file. `etl auth status` reports which source is active and whether eTL accepts it; an invalid environment token does not trigger a retry with the saved token. `etl auth logout` deletes only the saved file and cannot unset an environment variable. Status exits with code 0 for a valid token, 3 for missing or rejected credentials, and 4 for a request or file error. The CLI never prints token contents.

### Interactive mode

Running `etl` with no arguments, or `etl submit --interactive`, opens a simple prompt flow:

1. Choose a course from the active courses list.
2. Choose an eligible assignment, with due/lock dates, submission type, current status, and attempt limits shown.
3. Choose a regular file from the current working directory. Do not recurse into subdirectories in the first version.
4. Enter or accept the upload filename. This changes the name sent to eTL; it must not rename the local file.
5. Review the source path, destination, filename, any late/attempt warning, then explicitly confirm submission.
6. Show the verified receipt and the resulting submission link/status.

Start with a line-oriented prompt interface; add a full-screen TUI only if the interaction needs richer navigation. Both modes should call the same application and API layers.

## Suggested architecture

- **CLI layer:** argument parsing, interactive prompts, JSON/human output, and exit codes.
- **Application layer:** list courses/assignments, validate an assignment/file pair, run a dry-run, submit, and verify the result.
- **Canvas client:** pagination, assignment/submission retrieval, file metadata, download handling, upload handshake, upload completion, and submission creation. Keep authenticated API calls fixed to `https://myetl.snu.ac.kr`; never forward the eTL bearer token to upload or download storage URLs.
- **Auth provider:** use a nonempty `ETL_TOKEN` first, then the private credentials file in the OS local data directory. Never print the token, put it in a URL, or include it in logs/errors. The application does not load `.env` files.
- **Local file selection:** inspect only current-directory files for the interactive picker; validate regular-file status, readability, size, and allowed extension before starting upload.

## Roadmap

### Phase 0 — Non-OAuth feasibility spike (completed)

- The user's token was accepted by the course-list and assignment-list API endpoints.
- Confirmed target: course `305887`, `2026-2 Data Structures (001)`; assignment `381293`, `PA1 (Due: 10/13)`.
- Before the upload, PA1 allowed `online_upload` with `.zip`; the user's submission was `unsubmitted` with no attachment.
- `src.zip` passed `unzip -t` and had SHA-256 `7c7439ba4ba1db69cdfa854f26c7d6b89db32421921208df8932587d7675533b`.
- The user authorized one submission. It completed and was verified as attempt 1 at `2026-10-08T08:43:45Z`, with the `src.zip` attachment. The CLI sent one final submission request and did not retry.

### Phase 1 — Read-only scriptable CLI (implemented)

- Implemented the Rust 2024 `etl` binary with `clap`, blocking `reqwest`/Rustls, and `serde`/`serde_json`.
- Reads `ETL_TOKEN` or the saved credentials file; provides `courses` and `assignments ID` (also `--course-id ID`) with human and JSON output, pagination, and stable exit codes.
- Surfaces assignment type, due/lock information, allowed extensions, allowed attempts, current submission status, and IDs.

### Phase 2 — Safe file submission (implemented; single-file flow exercised once)

- Added `submit` with course/assignment IDs, positional local paths, optional remote `--filename` names, `--dry-run`, interactive confirmation, `--yes`, JSON output, and explicit `--resubmit` override.
- Also accepts `--assignment-url`, `--url`, or `-u` as an alternative destination input, extracting numeric IDs only from the expected eTL assignment-page path.
- Refuses incompatible assignment types, disallowed extensions, and existing submissions by default. Does not alter the source file.
- Implements Canvas's documented file-upload handshake for each file, makes at most one final submission request with all uploaded IDs per invocation, and verifies the resulting attempt contains every attachment.
- Submitted `src.zip` to PA1 after rechecking that the current submission remained unsubmitted. eTL returned a matching receipt for attempt 1. No second submission request was sent. The CLI does not automatically retry an ambiguous submission request.

### Phase 2.5 — File download (implemented)

- Added `get` with positional file URLs and the optional `-u`/`--url` form, supporting multiple URLs in either form.
- Parses course-scoped and global file URLs on the eTL host, then gets metadata through the Canvas Files API.
- Streams returned HTTPS download URLs without sending the bearer token to storage hosts. Redirects are followed only over HTTPS.
- Saves files in the current directory or `--output-dir`, chooses a safe metadata filename, and adds numeric suffixes to avoid overwriting existing files.
- Reports file IDs, saved paths, and byte counts as human-readable output or JSON. The example URL was verified with a metadata request and one-byte download probe; no full file was downloaded during the feasibility check.

### Phase 3 — Interactive selection (not implemented)

- Add course, assignment, cwd-file, and remote-filename prompts over the same application layer.
- Display due date/time with timezone, file restrictions, current submission/attempt count, and final confirmation.
- Keep the initial interface accessible through ordinary terminal input and output; add a TUI only if needed.

### Phase 4 — Packaging and operational polish (not implemented)

- Choose a package/install path for the Rust binary.
- Document token setup/revocation, examples, exit codes, failure recovery, and the personal-use limitation.
- Add shell completion and optional config for non-secret defaults; keep secrets out of ordinary config files.

## MVP acceptance criteria

- A script can list courses and assignments as JSON and submit one or more specified local files using explicit IDs.
- A script can download one or more files from eTL URLs without overwriting local files or sending the bearer token to storage hosts.
- The user can override each uploaded filename without renaming local files.
- The CLI refuses incompatible submission types and locally detectable file restrictions before upload.
- An interactive run can choose among cwd files, courses, and assignments and requires a final confirmation.
- Success is reported only after eTL's submission record shows the new attempt/file; errors do not expose authentication material.
- No OAuth flow, MySNU password storage, or multi-user token onboarding is implemented.

## Deferred scope

- Interactive course/assignment/file selection and a full-screen TUI remain Phase 3.
- Packaging polish and shell completion remain Phase 4.
- The standard file-upload and submission flow was confirmed end-to-end for Data Structures PA1. Other assignment types and courses have not been exercised; the CLI must stop rather than repeat a write when the result is uncertain.
- The multi-file flow has not been exercised against eTL. It uses Canvas's documented repeated `submission[file_ids][]` parameters in one final request.
