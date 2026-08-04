//! S3 helper — provides the VEIL-shaped API that the codegen emits.
//!
//! The generated adapter code calls patterns like:
//! ```ignore
//! veil_ddb::s3::get_object(bucket, key).fetch_one(&self.client)
//! veil_ddb::s3::put_object(bucket, key, data).execute(&self.client)
//! veil_ddb::s3::delete_object(bucket, key).execute(&self.client)
//! veil_ddb::s3::head_object(bucket, key).fetch_one(&self.client)
//! veil_ddb::s3::list_objects(bucket, prefix).fetch_all(&self.client)
//! ```

/// Error type for S3 operations.
#[derive(Debug, thiserror::Error)]
pub enum S3Error {
    #[error("S3 error: {0}")]
    Sdk(String),
    #[error("Object not found")]
    NotFound,
}

impl From<S3Error> for String {
    fn from(e: S3Error) -> String {
        e.to_string()
    }
}

// ─── Get Object ───────────────────────────────────────────────────────────────

pub fn get_object(bucket: impl Into<String>, key: impl Into<String>) -> S3GetBuilder {
    S3GetBuilder {
        bucket: bucket.into(),
        key: key.into(),
    }
}

pub struct S3GetBuilder {
    bucket: String,
    key: String,
}

impl S3GetBuilder {
    /// Fetch the object body as bytes.
    pub fn fetch_one(self, _client: &aws_sdk_s3::Client) -> Vec<u8> {
        panic!("veil_ddb::s3::get_object::fetch_one — stub not yet wired to real S3")
    }
}

// ─── Put Object ───────────────────────────────────────────────────────────────

pub fn put_object(
    bucket: impl Into<String>,
    key: impl Into<String>,
    data: Vec<u8>,
) -> S3PutBuilder {
    S3PutBuilder {
        bucket: bucket.into(),
        key: key.into(),
        data,
    }
}

pub struct S3PutBuilder {
    bucket: String,
    key: String,
    data: Vec<u8>,
}

impl S3PutBuilder {
    /// Execute the put operation.
    pub async fn execute(self, _client: &aws_sdk_s3::Client) -> Result<(), String> {
        panic!("veil_ddb::s3::put_object::execute — stub not yet wired to real S3")
    }
}

// ─── Delete Object ────────────────────────────────────────────────────────────

pub fn delete_object(bucket: impl Into<String>, key: impl Into<String>) -> S3DeleteBuilder {
    S3DeleteBuilder {
        bucket: bucket.into(),
        key: key.into(),
    }
}

pub struct S3DeleteBuilder {
    bucket: String,
    key: String,
}

impl S3DeleteBuilder {
    /// Execute the delete operation.
    pub async fn execute(self, _client: &aws_sdk_s3::Client) -> Result<(), String> {
        panic!("veil_ddb::s3::delete_object::execute — stub not yet wired to real S3")
    }
}

// ─── Head Object ──────────────────────────────────────────────────────────────

pub fn head_object(bucket: impl Into<String>, key: impl Into<String>) -> S3HeadBuilder {
    S3HeadBuilder {
        bucket: bucket.into(),
        key: key.into(),
    }
}

pub struct S3HeadBuilder {
    bucket: String,
    key: String,
}

impl S3HeadBuilder {
    /// Check if object exists (returns bool) or get metadata.
    /// When used as `bool`, returns whether the object exists.
    /// When used for size, returns content_length.
    pub fn fetch_one<T: Default>(self, _client: &aws_sdk_s3::Client) -> T {
        panic!("veil_ddb::s3::head_object::fetch_one — stub not yet wired to real S3")
    }
}

// ─── List Objects ─────────────────────────────────────────────────────────────

pub fn list_objects(bucket: impl Into<String>, prefix: impl Into<String>) -> S3ListBuilder {
    S3ListBuilder {
        bucket: bucket.into(),
        prefix: prefix.into(),
    }
}

pub struct S3ListBuilder {
    bucket: String,
    prefix: String,
}

impl S3ListBuilder {
    /// List all object keys matching the prefix.
    pub fn fetch_all(self, _client: &aws_sdk_s3::Client) -> Vec<String> {
        panic!("veil_ddb::s3::list_objects::fetch_all — stub not yet wired to real S3")
    }
}
