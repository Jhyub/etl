use std::{collections::BTreeMap, fs::File, path::Path, time::Duration};

use reqwest::{
    StatusCode,
    blocking::{Client as HttpClient, Response, multipart},
    header::{HeaderMap, LINK, LOCATION},
    redirect::Policy,
};
use serde::{Deserialize, de::DeserializeOwned};
use url::Url;

use crate::models::{Assignment, Course};

const BASE_URL: &str = "https://myetl.snu.ac.kr/";
const API_HOST: &str = "myetl.snu.ac.kr";

#[derive(Debug, thiserror::Error)]
pub enum ApiError {
    #[error("ETL_TOKEN is not set")]
    MissingToken,
    #[error("HTTP request failed")]
    Request(#[from] reqwest::Error),
    #[error("eTL rejected the token or request (HTTP {0})")]
    Auth(StatusCode),
    #[error("eTL API returned HTTP {0}")]
    Http(StatusCode),
    #[error("eTL redirected the API request instead of returning JSON")]
    Redirect,
    #[error("invalid eTL API response: {0}")]
    InvalidResponse(String),
    #[error("could not decode eTL JSON response: {0}")]
    Json(#[from] serde_json::Error),
    #[error("local file error: {0}")]
    Io(#[from] std::io::Error),
}

impl ApiError {
    pub fn exit_code(&self) -> i32 {
        match self {
            Self::MissingToken | Self::Auth(_) => 3,
            Self::Io(_) => 2,
            _ => 4,
        }
    }
}

#[derive(Debug, Deserialize)]
struct UploadSpec {
    upload_url: String,
    upload_params: BTreeMap<String, serde_json::Value>,
}

#[derive(Debug, Deserialize)]
struct UploadedFile {
    id: u64,
}

pub struct Client {
    token: String,
    api_http: HttpClient,
    upload_http: HttpClient,
    base_url: Url,
}

impl Client {
    pub fn from_env() -> Result<Self, ApiError> {
        let token = std::env::var("ETL_TOKEN").map_err(|_| ApiError::MissingToken)?;
        if token.trim().is_empty() {
            return Err(ApiError::MissingToken);
        }

        let api_http = HttpClient::builder()
            .redirect(Policy::none())
            .connect_timeout(Duration::from_secs(15))
            .timeout(Duration::from_secs(90))
            .build()?;
        // The storage URL is signed by Canvas. Never attach the eTL token to it.
        let upload_http = HttpClient::builder()
            .redirect(Policy::none())
            .connect_timeout(Duration::from_secs(20))
            .timeout(Duration::from_secs(600))
            .build()?;

        Ok(Self {
            token,
            api_http,
            upload_http,
            base_url: Url::parse(BASE_URL).map_err(|e| ApiError::InvalidResponse(e.to_string()))?,
        })
    }

    pub fn list_courses(&self) -> Result<Vec<Course>, ApiError> {
        let mut url = self.api_url("api/v1/courses")?;
        url.query_pairs_mut()
            .append_pair("enrollment_type", "student")
            .append_pair("per_page", "100");
        self.get_all_pages(url)
    }

    pub fn list_assignments(&self, course_id: u64) -> Result<Vec<Assignment>, ApiError> {
        let mut url = self.api_url(&format!("api/v1/courses/{course_id}/assignments"))?;
        url.query_pairs_mut()
            .append_pair("per_page", "100")
            .append_pair("include[]", "submission");
        self.get_all_pages(url)
    }

    pub fn get_assignment(
        &self,
        course_id: u64,
        assignment_id: u64,
    ) -> Result<Assignment, ApiError> {
        let mut url = self.api_url(&format!(
            "api/v1/courses/{course_id}/assignments/{assignment_id}"
        ))?;
        url.query_pairs_mut().append_pair("include[]", "submission");
        self.get_json(url)
    }

    pub fn upload_submission_file(
        &self,
        course_id: u64,
        assignment_id: u64,
        file_path: &Path,
        remote_filename: &str,
        size: u64,
        content_type: &str,
    ) -> Result<u64, ApiError> {
        let url = self.api_url(&format!(
            "api/v1/courses/{course_id}/assignments/{assignment_id}/submissions/self/files"
        ))?;
        let form = multipart::Form::new()
            .text("name", remote_filename.to_owned())
            .text("size", size.to_string())
            .text("content_type", content_type.to_owned());
        let response = self
            .api_http
            .post(url)
            .bearer_auth(&self.token)
            .header(reqwest::header::ACCEPT, "application/json")
            .multipart(form)
            .send()?;
        let spec: UploadSpec = self.decode_response(response)?;

        let upload_url = Url::parse(&spec.upload_url)
            .map_err(|e| ApiError::InvalidResponse(format!("invalid upload URL: {e}")))?;
        if upload_url.scheme() != "https" || upload_url.host_str().is_none() {
            return Err(ApiError::InvalidResponse(
                "eTL returned a non-HTTPS upload URL".to_owned(),
            ));
        }

        let file = File::open(file_path)?;
        let mut upload_form = multipart::Form::new();
        for (key, value) in spec.upload_params {
            match value {
                serde_json::Value::Array(values) => {
                    for value in values {
                        upload_form = upload_form.text(key.clone(), multipart_value(value));
                    }
                }
                value => upload_form = upload_form.text(key, multipart_value(value)),
            }
        }
        let part = multipart::Part::reader_with_length(file, size)
            .file_name(remote_filename.to_owned())
            .mime_str(content_type)?;
        upload_form = upload_form.part("file", part);

        let upload_response = self
            .upload_http
            .post(upload_url.clone())
            .multipart(upload_form)
            .send()?;
        let status = upload_response.status();
        let location = upload_response
            .headers()
            .get(LOCATION)
            .and_then(|value| value.to_str().ok())
            .map(str::to_owned);
        if !(status.is_success() || status.is_redirection()) {
            return Err(ApiError::Http(status));
        }

        let location = location.ok_or_else(|| {
            ApiError::InvalidResponse(format!(
                "upload server returned HTTP {status} without a completion location"
            ))
        })?;
        let completion_url = if let Ok(absolute) = Url::parse(&location) {
            absolute
        } else {
            self.base_url
                .join(&location)
                .map_err(|e| ApiError::InvalidResponse(e.to_string()))?
        };
        let uploaded: UploadedFile = self.get_json(completion_url)?;
        Ok(uploaded.id)
    }

    pub fn submit_file_id(
        &self,
        course_id: u64,
        assignment_id: u64,
        file_id: u64,
    ) -> Result<(), ApiError> {
        let url = self.api_url(&format!(
            "api/v1/courses/{course_id}/assignments/{assignment_id}/submissions"
        ))?;
        let form = multipart::Form::new()
            .text("submission[submission_type]", "online_upload")
            .text("submission[file_ids][]", file_id.to_string());
        let response = self
            .api_http
            .post(url)
            .bearer_auth(&self.token)
            .header(reqwest::header::ACCEPT, "application/json")
            .multipart(form)
            .send()?;
        self.check_status(response.status())
    }

    fn get_all_pages<T: DeserializeOwned>(&self, first_url: Url) -> Result<Vec<T>, ApiError> {
        let mut next_url = Some(first_url);
        let mut result = Vec::new();
        while let Some(url) = next_url.take() {
            self.ensure_api_url(&url)?;
            let response = self
                .api_http
                .get(url.clone())
                .bearer_auth(&self.token)
                .header(reqwest::header::ACCEPT, "application/json")
                .send()?;
            self.check_status(response.status())?;
            next_url = next_link(response.headers(), &url)?;
            let body = response.bytes()?;
            result.extend(serde_json::from_slice::<Vec<T>>(&body)?);
        }
        Ok(result)
    }

    fn get_json<T: DeserializeOwned>(&self, url: Url) -> Result<T, ApiError> {
        self.ensure_api_url(&url)?;
        let response = self
            .api_http
            .get(url)
            .bearer_auth(&self.token)
            .header(reqwest::header::ACCEPT, "application/json")
            .send()?;
        self.decode_response(response)
    }

    fn decode_response<T: DeserializeOwned>(&self, response: Response) -> Result<T, ApiError> {
        self.check_status(response.status())?;
        let body = response.bytes()?;
        Ok(serde_json::from_slice::<T>(&body)?)
    }

    fn check_status(&self, status: StatusCode) -> Result<(), ApiError> {
        if status == StatusCode::UNAUTHORIZED || status == StatusCode::FORBIDDEN {
            return Err(ApiError::Auth(status));
        }
        if status.is_redirection() {
            return Err(ApiError::Redirect);
        }
        if !status.is_success() {
            return Err(ApiError::Http(status));
        }
        Ok(())
    }

    fn api_url(&self, path: &str) -> Result<Url, ApiError> {
        self.base_url
            .join(path)
            .map_err(|e| ApiError::InvalidResponse(e.to_string()))
    }

    fn ensure_api_url(&self, url: &Url) -> Result<(), ApiError> {
        if url.scheme() != "https"
            || url.host_str() != Some(API_HOST)
            || url.port_or_known_default() != Some(443)
        {
            return Err(ApiError::InvalidResponse(
                "refusing to send eTL credentials outside https://myetl.snu.ac.kr".to_owned(),
            ));
        }
        Ok(())
    }
}

fn multipart_value(value: serde_json::Value) -> String {
    match value {
        serde_json::Value::String(value) => value,
        serde_json::Value::Null => String::new(),
        value => value.to_string(),
    }
}

fn next_link(headers: &HeaderMap, current_url: &Url) -> Result<Option<Url>, ApiError> {
    for value in headers.get_all(LINK).iter() {
        let value = value
            .to_str()
            .map_err(|e| ApiError::InvalidResponse(format!("invalid Link header: {e}")))?;
        for item in value.split(',') {
            let Some((target, attributes)) = item.split_once('>') else {
                continue;
            };
            if !attributes.to_ascii_lowercase().contains("rel=\"next\"")
                && !attributes.to_ascii_lowercase().contains("rel=next")
            {
                continue;
            }
            let target = target
                .strip_prefix('<')
                .ok_or_else(|| ApiError::InvalidResponse("malformed Link header".to_owned()))?;
            let next = current_url
                .join(target)
                .map_err(|e| ApiError::InvalidResponse(format!("invalid next page URL: {e}")))?;
            return Ok(Some(next));
        }
    }
    Ok(None)
}
