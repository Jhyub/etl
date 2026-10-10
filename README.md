# etl

## NAME

`etl` — list SNU eTL courses and assignments, download files, and submit local files to an assignment.

## SYNOPSIS

```text
etl auth login
etl auth status
etl auth logout
etl courses [-j | --json]
etl assignments (ID | -c ID | --course-id ID) [-j | --json]
etl get URL... [-o DIR | --output-dir DIR] [-j | --json]
etl get (-u | --url) URL... [-o DIR | --output-dir DIR] [-j | --json]
etl submit (--url URL | --assignment-url URL | -u URL |
            -c COURSE_ID -a ASSIGNMENT_ID)
           PATH... [-n NAME... | --filename NAME...]
           [-d | --dry-run | -y | --yes] [-r | --resubmit] [-j | --json]
etl --help
etl --version
```

Long options `--course-id` and `--assignment-id` also work with `etl submit`. Run `etl COMMAND --help` for command-specific help.

## INSTALLATION

From the repository directory, with a Rust toolchain installed:

```sh
cargo install --path .
```

## AUTHENTICATION

`etl` uses a Canvas API **access token** for your SNU eTL account. It does not use your MySNU password or a browser cookie.

To obtain a token in the Canvas account interface:

1. Sign in at [myetl.snu.ac.kr](https://myetl.snu.ac.kr/).
2. Open **Account → Settings** and find **Approved Integrations**.
3. Select **Add New Access Token**, enter a purpose such as `etl`, provide an expiration date if required, and select **Generate Token**.
4. Copy the generated token and keep it private. Canvas lets institutions disable this control; if it is unavailable in eTL, contact eTL support.

These steps follow [Instructure's access-token guide](https://community.instructure.com/en/kb/articles/662901-how-do-i-manage-api-access-tokens-in-my-user-account). Access tokens grant access to your account's Canvas resources and should be protected like passwords. [Instructure's API documentation](https://developerdocs.instructure.com/services/canvas/oauth2/file.oauth)

Save the token with:

```sh
etl auth login
etl auth status
```

When run in a terminal, `etl auth login` prompts without echoing the token. It also accepts one token line from standard input. Login makes one read-only API request to validate the token before saving it; `etl auth status` checks the effective token with one read-only request. Neither command prints the token.

`etl auth logout` removes the locally saved token. To invalidate the token itself, delete it from **Approved Integrations** in eTL. An `ETL_TOKEN` environment variable, when nonempty, overrides the saved token; logout does not unset that variable. The CLI does not read `.env` files automatically.

## COMMANDS

**`etl courses`** lists active student courses. Use `-j` or `--json` for JSON output.

**`etl assignments ID`** lists assignments in a course. `-c ID` and `--course-id ID` are equivalent to the positional course ID. Use `-j` or `--json` for JSON output.

**`etl get URL...`** downloads one or more files from eTL. Provide URLs as positional arguments, or use `-u URL...` / `--url URL...`; the two forms cannot be mixed. Both course file URLs such as `https://myetl.snu.ac.kr/courses/305832/files/9222279/download?wrap=1` and `/files/{file_id}` URLs are accepted, with or without a trailing `/download` path. Query parameters such as `wrap=1` are ignored when parsing the file ID. The command accepts only HTTPS URLs on `myetl.snu.ac.kr`.

Files are saved in the current directory by default. Use `-o DIR` or `--output-dir DIR` to select a directory; it is created if needed. The local name comes from the Canvas file metadata. If that name already exists, `etl` appends a number such as ` (1)` before the extension instead of replacing the existing file. URLs are processed in order. If a download fails, the command stops and any earlier completed downloads remain saved.

Use `-j` or `--json` for machine-readable output. On success, JSON contains a `files` array with `file_id`, optional `course_id`, absolute `saved_path`, and `bytes` for each download. In JSON mode, progress messages are written to stderr and the JSON result to stdout.

## GET OPTIONS

- `URL...`: one or more eTL file URLs. Supply these positionally or with `-u/--url`; do not mix the forms.
- `-u, --url URL...`: optional URL form; repeat it to provide multiple URLs.
- `-o, --output-dir DIR`: output directory. Defaults to the current directory.
- `-j, --json`: write machine-readable results to stdout and progress messages to stderr.

**`etl submit`** submits one or more regular local files to one assignment. Specify its destination with either a course and assignment ID or an assignment-page URL such as `https://myetl.snu.ac.kr/courses/123/assignments/456`. `--url` and `-u` are aliases for `--assignment-url`. The command accepts only assignment URLs on `https://myetl.snu.ac.kr`.

List file paths as space-separated positional arguments. By default, each uploaded file keeps its local basename. To change the names shown in eTL, put `-n NAME...` or `--filename NAME...` **after the paths**, with exactly one name per path in the same order. Quote any path or name containing spaces. This does not rename local files.

The command checks the files and assignment before uploading. It refuses assignments that do not allow file uploads, filenames with disallowed extensions, and assignments with an existing submission unless `-r` or `--resubmit` is given. It uploads each file, makes one final submission request containing all uploaded file IDs, and reports success only after confirming all attachments in the resulting submission. If an upload fails, no final submission request is sent; earlier uploaded files may remain in eTL.

## SUBMIT OPTIONS

- `-c, --course-id ID` and `-a, --assignment-id ID`: destination IDs; use both together.
- `-u, --assignment-url URL, --url URL`: assignment-page URL; use instead of destination IDs.
- `-n, --filename NAME...`: remote names, one per `PATH`, in order.
- `-d, --dry-run`: validate and show the planned submission without uploading or submitting. It still reads assignment details from eTL.
- `-y, --yes`: submit without a confirmation prompt. Required when submitting from a non-interactive script. Cannot be combined with `--dry-run`.
- `-r, --resubmit`: permit another attempt when a submission already exists.
- `-j, --json`: write machine-readable JSON instead of the normal text result. Submission output has a `files` array; each entry includes `file`, `filename`, `size`, and `content_type`.

Without `--yes` or `--dry-run`, `etl submit` shows the destination and every path/name pair, then asks for confirmation in the terminal.

## EXAMPLES

```sh
etl courses -j
etl assignments 123

etl get 'https://myetl.snu.ac.kr/courses/305832/files/9222279/download?wrap=1'
etl get 'https://myetl.snu.ac.kr/courses/305832/files/9222279/download?wrap=1' \
  'https://myetl.snu.ac.kr/files/9222280/download' --output-dir ./downloads -j
etl get -u 'https://myetl.snu.ac.kr/files/9222280/download' \
  -u 'https://myetl.snu.ac.kr/files/9222281/download'

etl submit --url 'https://myetl.snu.ac.kr/courses/123/assignments/456' \
  ./report.pdf --dry-run

etl submit -c 123 -a 456 ./report.pdf ./source.zip \
  -n final-report.pdf source.zip --yes
```

## ENVIRONMENT

`ETL_TOKEN` — when set to a nonblank value, use this token instead of the saved token. An empty or whitespace-only value falls back to the saved token.

## FILES

The saved token is kept in the platform's local user data directory: `$XDG_DATA_HOME/etl/credentials` on Linux (normally `~/.local/share/etl/credentials`), `~/Library/Application Support/etl/credentials` on macOS, or the `etl/credentials` directory under Local AppData on Windows. On Unix, `etl` creates the directory with mode `0700` and the credentials file with mode `0600`.

## EXIT STATUS

- `0`: command completed successfully.
- `2`: invalid arguments, a local file problem, or a submission validation failure.
- `3`: missing or rejected credentials.
- `4`: an API, network, response, or credentials-file error.

If a submission request fails or its receipt cannot be verified, inspect the assignment in eTL before trying again. The CLI does not automatically repeat the final submission request.
