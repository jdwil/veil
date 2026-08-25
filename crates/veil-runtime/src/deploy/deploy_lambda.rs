//! Lambda deploy step — upload zip to S3, update function code.

use std::path::Path;
use tracing::info;

/// Deploy a Lambda function: upload zip to S3, then update function code.
pub async fn run(
    slug: &str,
    artifact_path: &Path,
    artifact_bucket: &str,
    version: &str,
    function_names: &[String],
    s3_client: &aws_sdk_s3::Client,
    lambda_client: &aws_sdk_lambda::Client,
) -> Result<DeployLambdaResult, String> {
    // Step 1: Upload zip to S3
    let s3_key = format!("deploys/{slug}/{version}/lambda.zip");

    let body = tokio::fs::read(artifact_path)
        .await
        .map_err(|e| format!("read artifact: {e}"))?;

    s3_client
        .put_object()
        .bucket(artifact_bucket)
        .key(&s3_key)
        .body(body.into())
        .send()
        .await
        .map_err(|e| format!("S3 upload failed: {e:?}"))?;

    info!(slug, s3_key = %s3_key, "artifact uploaded to S3");

    // Step 2: Update each Lambda function
    let mut updated_functions = Vec::new();
    for function_name in function_names {
        let result = lambda_client
            .update_function_code()
            .function_name(function_name)
            .s3_bucket(artifact_bucket)
            .s3_key(&s3_key)
            .publish(true) // Always publish a version (palace: veil-native-deploy-provision)
            .send()
            .await
            .map_err(|e| format!("update Lambda {function_name} failed: {e:?}"))?;

        let published_version = result.version().unwrap_or("$LATEST").to_string();
        info!(
            slug,
            function_name,
            version = %published_version,
            "Lambda updated"
        );
        updated_functions.push(UpdatedFunction {
            name: function_name.clone(),
            version: published_version,
        });
    }

    Ok(DeployLambdaResult {
        s3_key,
        updated_functions,
    })
}

#[derive(Debug, Clone)]
pub struct DeployLambdaResult {
    pub s3_key: String,
    pub updated_functions: Vec<UpdatedFunction>,
}

#[derive(Debug, Clone)]
pub struct UpdatedFunction {
    pub name: String,
    pub version: String,
}
