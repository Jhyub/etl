use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct Course {
    pub id: u64,
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub course_code: Option<String>,
    #[serde(default)]
    pub workflow_state: Option<String>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct Assignment {
    pub id: u64,
    pub name: String,
    #[serde(default)]
    pub due_at: Option<String>,
    #[serde(default)]
    pub unlock_at: Option<String>,
    #[serde(default)]
    pub lock_at: Option<String>,
    #[serde(default)]
    pub submission_types: Vec<String>,
    #[serde(default)]
    pub allowed_extensions: Option<Vec<String>>,
    #[serde(default)]
    pub allowed_attempts: Option<i64>,
    #[serde(rename = "submission", default)]
    pub current_submission: Option<Submission>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct Submission {
    #[serde(default)]
    pub submitted_at: Option<String>,
    #[serde(default)]
    pub attempt: Option<u32>,
    #[serde(default)]
    pub workflow_state: Option<String>,
    #[serde(default)]
    pub submission_type: Option<String>,
    #[serde(default)]
    pub attachments: Vec<Attachment>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct Attachment {
    #[serde(default)]
    pub id: Option<u64>,
    #[serde(default)]
    pub display_name: Option<String>,
    #[serde(default)]
    pub size: Option<u64>,
}
