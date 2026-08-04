//! VEIL DDB/S3 helper crate.
//!
//! Provides the builder-pattern API that VEIL's codegen emits for DynamoDB and S3
//! adapter bodies. Wraps the real AWS SDK calls behind a simplified interface that
//! matches the VEIL `.stub` method signatures.

pub mod ddb;
pub mod s3;
