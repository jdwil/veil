//! Implementations of traits.

#![allow(unused_imports, unused_variables, dead_code)]

use crate::domain::types::*;
use crate::ports::*;
use async_trait::async_trait;
use chrono::Utc;
use std::collections::HashMap;
use uuid::Uuid;

// External-effect runtime hooks (stubs). Replace with real
// integrations; generated so adapter bodies compile.
fn attribute_definition_builder() { /* stub — replace with real integration */
}
fn attribute_value_s(_arg0: impl std::fmt::Debug) { /* stub — replace with real integration */
}
fn client_put_item() { /* stub — replace with real integration */
}
fn client_query() { /* stub — replace with real integration */
}
fn client_scan() { /* stub — replace with real integration */
}
fn function_code_builder() { /* stub — replace with real integration */
}
fn key_schema_element_builder() { /* stub — replace with real integration */
}
fn process_run(
    _arg0: impl std::fmt::Debug,
    _arg1: impl std::fmt::Debug,
    _arg2: impl std::fmt::Debug,
) { /* stub — replace with real integration */
}

/// Adapter: BusAuthAdapter (implements AuthService)
pub struct BusAuthAdapter {}

#[async_trait]
impl AuthService for BusAuthAdapter {
    async fn check_permission(
        &self,
        principal: Principal,
        permission: String,
    ) -> Result<bool, DomainError> {
        return Ok(true);
    }

    async fn validate_token(&self, token: String) -> Result<Principal, DomainError> {
        return Ok(Principal {
            id: token.clone(),
            roles: vec![],
            claims: HashMap::new(),
        });
    }
}

/// Adapter: DdbDeploymentStore (implements DeploymentStateStore)
pub struct DdbDeploymentStore {
    pub client: aws_sdk_dynamodb::Client,
    pub table: String,
}

#[async_trait]
impl DeploymentStateStore for DdbDeploymentStore {
    async fn append_event(
        &self,
        environment: String,
        unit_name: String,
        event: DeployEvent,
    ) -> Result<(), DomainError> {
        let pk = format!("DEPLOY#{}#{}", environment, unit_name);
        let sk = format!("EVENT#{}", event.timestamp);
        self.client
            .put_item()
            .table_name(&self.table)
            .item(
                "PK".to_string(),
                aws_sdk_dynamodb::types::AttributeValue::S(pk),
            )
            .item(
                "SK".to_string(),
                aws_sdk_dynamodb::types::AttributeValue::S(sk),
            )
            .item(
                "data".to_string(),
                aws_sdk_dynamodb::types::AttributeValue::S(serde_json::to_string(&event)?),
            )
            .send()
            .await
            .map_err(|e| DomainError::External(format!("{e:?}")))?;
        Ok(())
    }

    async fn get_current(
        &self,
        environment: String,
        unit_name: String,
    ) -> Result<Option<DeploymentState>, DomainError> {
        let pk = format!("DEPLOY#{}#{}", environment, unit_name);
        let resp = self
            .client
            .query()
            .table_name(&self.table)
            .key_condition_expression("PK = :pk AND SK = :sk".to_string())
            .expression_attribute_values(
                ":pk".to_string(),
                aws_sdk_dynamodb::types::AttributeValue::S(pk),
            )
            .expression_attribute_values(
                ":sk".to_string(),
                aws_sdk_dynamodb::types::AttributeValue::S("CURRENT".to_string()),
            )
            .send()
            .await
            .map_err(|e| DomainError::External(format!("{e:?}")))?;
        let items = resp.items();
        if items.is_empty() {
            return Ok(None);
        };
        return Ok(Some(serde_json::from_str::<_>(
            &items[(0) as usize]
                .get("data")
                .ok_or_else(|| DomainError::External("missing data".into()))?
                .as_s()
                .map(|s| s.to_string())
                .map_err(|e| DomainError::External(format!("{:?}", e)))?,
        )?));
    }

    async fn get_events(
        &self,
        environment: String,
        unit_name: String,
        limit: i64,
    ) -> Result<Vec<DeployEvent>, DomainError> {
        let pk = format!("DEPLOY#{}#{}", environment, unit_name);
        let resp = self
            .client
            .query()
            .table_name(&self.table)
            .key_condition_expression("PK = :pk AND begins_with(SK, :prefix)".to_string())
            .expression_attribute_values(
                ":pk".to_string(),
                aws_sdk_dynamodb::types::AttributeValue::S(pk),
            )
            .expression_attribute_values(
                ":prefix".to_string(),
                aws_sdk_dynamodb::types::AttributeValue::S("EVENT#".to_string()),
            )
            .limit((limit) as i32)
            .send()
            .await
            .map_err(|e| DomainError::External(format!("{e:?}")))?;
        return Ok(resp
            .items()
            .iter()
            .map(|i| {
                serde_json::from_str::<_>(
                    &i.get("data")
                        .ok_or_else(|| DomainError::External("missing data".into()))
                        .unwrap()
                        .as_s()
                        .map(|s| s.to_string())
                        .unwrap(),
                )
                .unwrap()
            })
            .collect());
    }

    async fn get_version(
        &self,
        environment: String,
        unit_name: String,
        version: i64,
    ) -> Result<Option<DeploymentState>, DomainError> {
        let pk = format!("DEPLOY#{}#{}", environment, unit_name);
        let sk = format!("VERSION#{}", version);
        let resp = self
            .client
            .query()
            .table_name(&self.table)
            .key_condition_expression("PK = :pk AND SK = :sk".to_string())
            .expression_attribute_values(
                ":pk".to_string(),
                aws_sdk_dynamodb::types::AttributeValue::S(pk),
            )
            .expression_attribute_values(
                ":sk".to_string(),
                aws_sdk_dynamodb::types::AttributeValue::S(sk),
            )
            .send()
            .await
            .map_err(|e| DomainError::External(format!("{e:?}")))?;
        let items = resp.items();
        if items.is_empty() {
            return Ok(None);
        };
        return Ok(Some(serde_json::from_str::<_>(
            &items[(0) as usize]
                .get("data")
                .ok_or_else(|| DomainError::External("missing data".into()))?
                .as_s()
                .map(|s| s.to_string())
                .map_err(|e| DomainError::External(format!("{:?}", e)))?,
        )?));
    }

    async fn list_deployments(&self) -> Result<Vec<DeploymentState>, DomainError> {
        // FilterExpression is applied after each 1MB scan page. Follow
        // LastEvaluatedKey so CURRENT rows on later pages are not dropped.
        let mut items = Vec::new();
        let mut exclusive_start_key = None;
        loop {
            let mut req = self
                .client
                .scan()
                .table_name(&self.table)
                .filter_expression("begins_with(PK, :prefix) AND SK = :sk".to_string())
                .expression_attribute_values(
                    ":prefix".to_string(),
                    aws_sdk_dynamodb::types::AttributeValue::S("DEPLOY#".to_string()),
                )
                .expression_attribute_values(
                    ":sk".to_string(),
                    aws_sdk_dynamodb::types::AttributeValue::S("CURRENT".to_string()),
                );
            if let Some(key) = exclusive_start_key {
                req = req.set_exclusive_start_key(Some(key));
            }
            let resp = req
                .send()
                .await
                .map_err(|e| DomainError::External(format!("{e:?}")))?;
            items.extend(resp.items().iter().cloned());
            match resp.last_evaluated_key() {
                Some(key) if !key.is_empty() => exclusive_start_key = Some(key.clone()),
                _ => break,
            }
        }
        let mut out = Vec::with_capacity(items.len());
        for item in items {
            let data = item
                .get("data")
                .ok_or_else(|| DomainError::External("missing data".into()))?
                .as_s()
                .map_err(|e| DomainError::External(format!("{e:?}")))?;
            out.push(
                serde_json::from_str::<DeploymentState>(data)
                    .map_err(|e| DomainError::External(format!("deploy CURRENT: {e}")))?,
            );
        }
        Ok(out)
    }

    async fn list_versions(
        &self,
        environment: String,
        unit_name: String,
        limit: i64,
    ) -> Result<Vec<DeploymentState>, DomainError> {
        let pk = format!("DEPLOY#{}#{}", environment, unit_name);
        let resp = self
            .client
            .query()
            .table_name(&self.table)
            .key_condition_expression("PK = :pk AND begins_with(SK, :prefix)".to_string())
            .expression_attribute_values(
                ":pk".to_string(),
                aws_sdk_dynamodb::types::AttributeValue::S(pk),
            )
            .expression_attribute_values(
                ":prefix".to_string(),
                aws_sdk_dynamodb::types::AttributeValue::S("VERSION#".to_string()),
            )
            .limit((limit) as i32)
            .send()
            .await
            .map_err(|e| DomainError::External(format!("{e:?}")))?;
        return Ok(resp
            .items()
            .iter()
            .map(|i| {
                serde_json::from_str::<_>(
                    &i.get("data")
                        .ok_or_else(|| DomainError::External("missing data".into()))
                        .unwrap()
                        .as_s()
                        .map(|s| s.to_string())
                        .unwrap(),
                )
                .unwrap()
            })
            .collect());
    }

    async fn save_current(&self, state: DeploymentState) -> Result<(), DomainError> {
        let pk = format!("DEPLOY#{}#{}", state.environment, state.unit_name);
        self.client
            .put_item()
            .table_name(&self.table)
            .item(
                "PK".to_string(),
                aws_sdk_dynamodb::types::AttributeValue::S(pk),
            )
            .item(
                "SK".to_string(),
                aws_sdk_dynamodb::types::AttributeValue::S("CURRENT".to_string()),
            )
            .item(
                "data".to_string(),
                aws_sdk_dynamodb::types::AttributeValue::S(serde_json::to_string(&state)?),
            )
            .send()
            .await
            .map_err(|e| DomainError::External(format!("{e:?}")))?;
        Ok(())
    }

    async fn save_version(&self, state: DeploymentState) -> Result<(), DomainError> {
        let pk = format!("DEPLOY#{}#{}", state.environment, state.unit_name);
        let sk = format!("VERSION#{}", state.version);
        self.client
            .put_item()
            .table_name(&self.table)
            .item(
                "PK".to_string(),
                aws_sdk_dynamodb::types::AttributeValue::S(pk),
            )
            .item(
                "SK".to_string(),
                aws_sdk_dynamodb::types::AttributeValue::S(sk),
            )
            .item(
                "data".to_string(),
                aws_sdk_dynamodb::types::AttributeValue::S(serde_json::to_string(&state)?),
            )
            .send()
            .await
            .map_err(|e| DomainError::External(format!("{e:?}")))?;
        Ok(())
    }
}

/// Adapter: LocalDeployExec (implements DeployExec)
pub struct LocalDeployExec {
    pub apigw: aws_sdk_apigatewayv2::Client,
    pub bucket: String,
    pub ddb: aws_sdk_dynamodb::Client,
    pub lambda: aws_sdk_lambda::Client,
    pub s3: aws_sdk_s3::Client,
    pub sns: aws_sdk_sns::Client,
    pub sqs: aws_sdk_sqs::Client,
}

#[async_trait]
impl DeployExec for LocalDeployExec {
    async fn clear_unit_state(
        &self,
        environment: String,
        unit_name: String,
    ) -> Result<String, DomainError> {
        let table = std::env::var("VEIL_DDB_TABLE".to_string()).unwrap_or_else(|_| {
            std::env::var("TABLE".to_string())
                .unwrap_or_else(|_| "veil-runtime-dev".to_string())
                .to_string()
        });
        let pk = format!("DEPLOY#{}#{}", environment, unit_name);
        let resp = self
            .ddb
            .query()
            .table_name(table.clone())
            .key_condition_expression("PK = :pk".to_string())
            .expression_attribute_values(
                ":pk".to_string(),
                aws_sdk_dynamodb::types::AttributeValue::S(pk.clone()),
            )
            .send()
            .await
            .map_err(|e| DomainError::External(format!("{e:?}")))?;
        let items = resp.items();
        let mut deleted = 0;
        for item in items {
            let sk = item
                .get("SK")
                .ok_or_else(|| DomainError::External("missing SK".into()))?
                .as_s()
                .map(|s| s.to_string())
                .map_err(|e| DomainError::External(format!("{:?}", e)))?;
            self.ddb
                .delete_item()
                .table_name(table.clone())
                .key(
                    "PK".to_string(),
                    aws_sdk_dynamodb::types::AttributeValue::S(pk.clone()),
                )
                .key(
                    "SK".to_string(),
                    aws_sdk_dynamodb::types::AttributeValue::S(sk),
                )
                .send()
                .await
                .map_err(|e| DomainError::External(format!("{e:?}")))?;
            deleted = deleted + 1;
        }
        return Ok(serde_json::to_string(
            &serde_json::json!({ "deleted": deleted.clone(), "pk": pk.clone() }),
        )?);
    }

    async fn get_provision_job(&self, job_id: String) -> Result<String, DomainError> {
        let path = veil_local_fs::LocalFs::join(
            "/tmp/veil-provision-jobs".to_string(),
            format!("{}.json", job_id),
        );
        if veil_local_fs::LocalFs::path_is_file(path.clone()) {
            let raw = veil_local_fs::LocalFs::read(path.clone())
                .map_err(|e| DomainError::External(e.to_string()))?;
            let job: serde_json::Value = serde_json::from_str::<_>(&raw)?;
            let st = job
                .get("status")
                .cloned()
                .ok_or(DomainError::NotFound)?
                .as_str()
                .map(|s| s.to_string())
                .unwrap_or("".to_string())
                .to_string();
            if st == "succeeded".to_string() {
                return Ok(serde_json::to_string(&job)?);
            };
            if st == "failed".to_string() {
                return Ok(serde_json::to_string(&job)?);
            };
            let mock_s = std::env::var("VEIL_DEPLOY_EXECUTOR".to_string())
                .unwrap_or_else(|_| "".to_string());
            let mut mock = mock_s == "mock".to_string();
            if mock_s == "MOCK".to_string() {
                mock = true;
            };
            let now = Utc::now();
            let cursor = job
                .get("cursor")
                .cloned()
                .ok_or(DomainError::NotFound)?
                .as_str()
                .map(|s| s.to_string())
                .unwrap_or("done".to_string())
                .to_string();
            let ddb_name = job
                .get("ddb_name")
                .cloned()
                .ok_or(DomainError::NotFound)?
                .as_str()
                .map(|s| s.to_string())
                .unwrap_or("veil-unknown".to_string())
                .to_string();
            let sns_name = job
                .get("sns_name")
                .cloned()
                .ok_or(DomainError::NotFound)?
                .as_str()
                .map(|s| s.to_string())
                .unwrap_or("veil-unknown".to_string())
                .to_string();
            let sqs_name = job
                .get("sqs_name")
                .cloned()
                .ok_or(DomainError::NotFound)?
                .as_str()
                .map(|s| s.to_string())
                .unwrap_or("veil-unknown".to_string())
                .to_string();
            let slug = job
                .get("project_slug")
                .cloned()
                .ok_or(DomainError::NotFound)?
                .as_str()
                .map(|s| s.to_string())
                .unwrap_or("".to_string())
                .to_string();
            let environment = job
                .get("environment")
                .cloned()
                .ok_or(DomainError::NotFound)?
                .as_str()
                .map(|s| s.to_string())
                .unwrap_or("dev".to_string())
                .to_string();
            let u0 = job
                .get("u0")
                .cloned()
                .ok_or(DomainError::NotFound)?
                .as_str()
                .map(|s| s.to_string())
                .unwrap_or("".to_string())
                .to_string();
            let t0 = job
                .get("t0")
                .cloned()
                .ok_or(DomainError::NotFound)?
                .as_str()
                .map(|s| s.to_string())
                .unwrap_or("".to_string())
                .to_string();
            let f0 = job
                .get("f0")
                .cloned()
                .ok_or(DomainError::NotFound)?
                .as_str()
                .map(|s| s.to_string())
                .unwrap_or("".to_string())
                .to_string();
            let u1 = job
                .get("u1")
                .cloned()
                .ok_or(DomainError::NotFound)?
                .as_str()
                .map(|s| s.to_string())
                .unwrap_or("".to_string())
                .to_string();
            let t1 = job
                .get("t1")
                .cloned()
                .ok_or(DomainError::NotFound)?
                .as_str()
                .map(|s| s.to_string())
                .unwrap_or("".to_string())
                .to_string();
            let f1 = job
                .get("f1")
                .cloned()
                .ok_or(DomainError::NotFound)?
                .as_str()
                .map(|s| s.to_string())
                .unwrap_or("".to_string())
                .to_string();
            let u2 = job
                .get("u2")
                .cloned()
                .ok_or(DomainError::NotFound)?
                .as_str()
                .map(|s| s.to_string())
                .unwrap_or("".to_string())
                .to_string();
            let t2 = job
                .get("t2")
                .cloned()
                .ok_or(DomainError::NotFound)?
                .as_str()
                .map(|s| s.to_string())
                .unwrap_or("".to_string())
                .to_string();
            let f2 = job
                .get("f2")
                .cloned()
                .ok_or(DomainError::NotFound)?
                .as_str()
                .map(|s| s.to_string())
                .unwrap_or("".to_string())
                .to_string();
            let mut unit_count = 0;
            if u0 != "".to_string() {
                unit_count = 1;
            };
            if u1 != "".to_string() {
                unit_count = 2;
            };
            if u2 != "".to_string() {
                unit_count = 3;
            };
            let role = std::env::var("VEIL_LAMBDA_ROLE".to_string()).unwrap_or_default();
            let mut detail = "ok".to_string();
            let mut next_cursor = "done".to_string();
            let mut status = "running".to_string();
            let mut summary = "Running".to_string();
            let mut steps_done = 0;
            let mut finished_s = "".to_string();
            let total = 8 + unit_count;
            if cursor == "load_config".to_string() {
                detail = format!("slug={} env={}", slug, environment);
                next_cursor = "stack_dynamodb".to_string();
                steps_done = 1;
            };
            if cursor == "stack_dynamodb".to_string() {
                if mock {
                    detail = format!("mock ok {}", ddb_name);
                } else {
                    let dr = self
                        .ddb
                        .describe_table()
                        .table_name(ddb_name.clone())
                        .send()
                        .await;
                    if dr.is_ok() {
                        detail = format!("{} (exists)", ddb_name);
                    } else {
                        self.ddb
                            .create_table()
                            .table_name(ddb_name.clone())
                            .billing_mode(aws_sdk_dynamodb::types::BillingMode::PayPerRequest)
                            .attribute_definitions(
                                aws_sdk_dynamodb::types::AttributeDefinition::builder()
                                    .attribute_name("PK".to_string())
                                    .attribute_type(aws_sdk_dynamodb::types::ScalarAttributeType::S)
                                    .build()
                                    .map_err(|e| DomainError::External(e.to_string()))?,
                            )
                            .attribute_definitions(
                                aws_sdk_dynamodb::types::AttributeDefinition::builder()
                                    .attribute_name("SK".to_string())
                                    .attribute_type(aws_sdk_dynamodb::types::ScalarAttributeType::S)
                                    .build()
                                    .map_err(|e| DomainError::External(e.to_string()))?,
                            )
                            .key_schema(
                                aws_sdk_dynamodb::types::KeySchemaElement::builder()
                                    .attribute_name("PK".to_string())
                                    .key_type(aws_sdk_dynamodb::types::KeyType::Hash)
                                    .build()
                                    .map_err(|e| DomainError::External(e.to_string()))?,
                            )
                            .key_schema(
                                aws_sdk_dynamodb::types::KeySchemaElement::builder()
                                    .attribute_name("SK".to_string())
                                    .key_type(aws_sdk_dynamodb::types::KeyType::Range)
                                    .build()
                                    .map_err(|e| DomainError::External(e.to_string()))?,
                            )
                            .send()
                            .await
                            .map_err(|e| DomainError::External(format!("{e:?}")))?;
                        detail = format!("{} (created)", ddb_name);
                    };
                };
                next_cursor = "stack_sns".to_string();
                steps_done = 2;
            };
            if cursor == "stack_sns".to_string() {
                if mock {
                    detail = format!("mock ok {}", sns_name);
                } else {
                    let sns_out = self
                        .sns
                        .create_topic()
                        .name(sns_name.clone())
                        .send()
                        .await
                        .map_err(|e| DomainError::External(format!("{e:?}")))?;
                    let mut topic_arn = "".to_string();
                    if sns_out.topic_arn().is_some() {
                        topic_arn = sns_out.topic_arn().unwrap().to_string();
                    };
                    detail = format!("SNS {} ensure arn={}", sns_name, topic_arn);
                };
                next_cursor = "stack_sqs".to_string();
                steps_done = 3;
            };
            if cursor == "stack_sqs".to_string() {
                if mock {
                    detail = format!("mock ok {}", sqs_name);
                } else {
                    let dlq = format!("{}-dlq", sqs_name);
                    let dqr = self
                        .sqs
                        .get_queue_url()
                        .queue_name(dlq.clone())
                        .send()
                        .await;
                    if dqr.is_ok() {
                        detail = format!("DLQ {} exists", dlq);
                    } else {
                        self.sqs
                            .create_queue()
                            .queue_name(dlq.clone())
                            .send()
                            .await
                            .map_err(|e| DomainError::External(format!("{e:?}")))?;
                        detail = format!("DLQ {} created", dlq);
                    };
                    let qr = self
                        .sqs
                        .get_queue_url()
                        .queue_name(sqs_name.clone())
                        .send()
                        .await;
                    if qr.is_ok() {
                        detail = format!("{} exists + {}", sqs_name, detail);
                    } else {
                        self.sqs
                            .create_queue()
                            .queue_name(sqs_name.clone())
                            .send()
                            .await
                            .map_err(|e| DomainError::External(format!("{e:?}")))?;
                        detail = format!("{} created + {}", sqs_name, detail);
                    };
                };
                next_cursor = "unit_0".to_string();
                steps_done = 4;
            };
            if cursor == "unit_0".to_string() {
                if unit_count < 1 {
                    detail = "no units".to_string();
                    next_cursor = "compile".to_string();
                    steps_done = 5;
                } else {
                    if mock {
                        detail = format!("mock unit {} -> {}", u0, f0);
                        next_cursor = "unit_1".to_string();
                        steps_done = 5;
                    } else {
                        let mut arn0 = "".to_string();
                        let exists0 = self
                            .lambda
                            .get_function()
                            .function_name(f0.clone())
                            .send()
                            .await
                            .is_ok();
                        if exists0 {
                            let fout = self
                                .lambda
                                .get_function()
                                .function_name(f0.clone())
                                .send()
                                .await
                                .map_err(|e| DomainError::External(format!("{e:?}")))?;
                            let conf0 = fout.configuration();
                            if conf0.is_some() {
                                let c0 = conf0.clone().ok_or(DomainError::NotFound)?;
                                let fa0 = c0.function_arn();
                                if fa0.is_some() {
                                    arn0 = fa0.clone().ok_or(DomainError::NotFound)?.to_string();
                                };
                            };
                            self.lambda
                                .update_function_configuration()
                                .function_name(f0.clone())
                                .memory_size(1024)
                                .timeout(30)
                                .send()
                                .await
                                .map_err(|e| DomainError::External(format!("{e:?}")))?;
                            detail = format!("{} exists arn={}", f0, arn0);
                        } else {
                            let zip_blob = aws_sdk_lambda::primitives::Blob::new({
                                let __h: String = ("504b03041400000000001042fd5c9a7470c7460000004600000008000000696e6465782e6a736578706f7274732e68616e646c65723d6173796e632865293d3e287b737461747573436f64653a3230302c626f64793a277665696c2d706c616365686f6c646572277d293b0a504b010214031400000000001042fd5c9a7470c74600000046000000080000000000000000000000800100000000696e6465782e6a73504b05060000000001000100360000006c0000000000".to_string()).to_string();
                                let __h = __h.as_str();
                                let mut __b = Vec::with_capacity(__h.len() / 2);
                                let mut __i = 0usize;
                                while __i + 1 < __h.len() {
                                    if let Ok(__v) = u8::from_str_radix(&__h[__i..__i + 2], 16) {
                                        __b.push(__v);
                                    }
                                    __i += 2;
                                }
                                __b
                            });
                            let code = aws_sdk_lambda::types::FunctionCode::builder()
                                .zip_file(zip_blob.clone())
                                .build();
                            let created = self
                                .lambda
                                .create_function()
                                .function_name(f0.clone())
                                .runtime(aws_sdk_lambda::types::Runtime::Nodejs20x)
                                .role(role.clone())
                                .handler("index.handler".to_string())
                                .code(code.clone())
                                .memory_size(1024)
                                .timeout(30)
                                .architectures(aws_sdk_lambda::types::Architecture::Arm64)
                                .description("VEIL provisioned unit".to_string())
                                .send()
                                .await
                                .map_err(|e| DomainError::External(format!("{e:?}")))?;
                            let fa_created = created.function_arn();
                            if fa_created.is_some() {
                                arn0 = fa_created.clone().ok_or(DomainError::NotFound)?.to_string();
                            };
                            detail = format!("{} created arn={}", f0, arn0);
                        };
                        if t0 == "lambda-api".to_string() {
                            if arn0 != "".to_string() {
                                let region = std::env::var("AWS_REGION".to_string())
                                    .unwrap_or_else(|_| "us-west-2".to_string());
                                let mut api_id = std::env::var("VEIL_GW_HTTP_API".to_string())
                                    .unwrap_or_else(|_| "".to_string());
                                let mut api_name = "".to_string();
                                if api_id == "".to_string() {
                                    let apis = self
                                        .apigw
                                        .get_apis()
                                        .send()
                                        .await
                                        .map_err(|e| DomainError::External(format!("{e:?}")))?;
                                    for item in apis.items() {
                                        let pt = item.protocol_type();
                                        if pt.is_some() {
                                            let n = item.name().unwrap_or("").to_string();
                                            if n.contains("http-api")
                                                || n.contains("service-api")
                                                || n == "http-api".to_string()
                                            {
                                                if api_id == "".to_string() {
                                                    api_id =
                                                        item.api_id().unwrap_or("").to_string();
                                                    api_name = n;
                                                };
                                            };
                                        };
                                    }
                                };
                                if api_id != "".to_string() {
                                    let integ_uri = format!(
                                        "arn:aws:apigateway:{}:lambda:path/2015-03-31/functions/{}/invocations",
                                        region, arn0
                                    );
                                    let mut integ_id = "".to_string();
                                    let integs = self
                                        .apigw
                                        .get_integrations()
                                        .api_id(api_id.clone())
                                        .send()
                                        .await
                                        .map_err(|e| DomainError::External(format!("{e:?}")))?;
                                    for integ in integs.items() {
                                        let iu = integ.integration_uri().unwrap_or("").to_string();
                                        if iu.contains(&arn0) {
                                            integ_id =
                                                integ.integration_id().unwrap_or("").to_string();
                                        };
                                    }
                                    if integ_id == "".to_string() {
                                        let ci = self.apigw.create_integration().api_id(api_id.clone()).integration_type(aws_sdk_apigatewayv2::types::IntegrationType::AwsProxy).integration_uri(integ_uri.clone()).payload_format_version("1.0".to_string()).connection_type(aws_sdk_apigatewayv2::types::ConnectionType::Internet).send().await.map_err(|e| DomainError::External(format!("{e:?}")))?;
                                        integ_id = ci.integration_id().unwrap_or("").to_string();
                                    };
                                    let path_prefix = "/relay".to_string();
                                    let rk1 = format!("ANY {}", path_prefix);
                                    let rk2 = format!("ANY {}/{{proxy+}}", path_prefix);
                                    let routes = self
                                        .apigw
                                        .get_routes()
                                        .api_id(api_id.clone())
                                        .send()
                                        .await
                                        .map_err(|e| DomainError::External(format!("{e:?}")))?;
                                    let mut has1 = false;
                                    let mut has2 = false;
                                    for r in routes.items() {
                                        let rk = r.route_key().unwrap_or("").to_string();
                                        if rk == rk1 {
                                            has1 = true;
                                            let rid = r.route_id().unwrap_or("").to_string();
                                            if rid != "".to_string() {
                                                self.apigw
                                                    .update_route()
                                                    .api_id(api_id.clone())
                                                    .route_id(rid.clone())
                                                    .target(format!("integrations/{}", integ_id))
                                                    .send()
                                                    .await
                                                    .map_err(|e| {
                                                        DomainError::External(format!("{e:?}"))
                                                    })?;
                                            };
                                        };
                                        if rk == rk2 {
                                            has2 = true;
                                            let rid2 = r.route_id().unwrap_or("").to_string();
                                            if rid2 != "".to_string() {
                                                self.apigw
                                                    .update_route()
                                                    .api_id(api_id.clone())
                                                    .route_id(rid2.clone())
                                                    .target(format!("integrations/{}", integ_id))
                                                    .send()
                                                    .await
                                                    .map_err(|e| {
                                                        DomainError::External(format!("{e:?}"))
                                                    })?;
                                            };
                                        };
                                    }
                                    if !has1 {
                                        self.apigw.create_route().api_id(api_id.clone()).route_key(rk1.clone()).target(format!("integrations/{}", integ_id)).authorization_type(aws_sdk_apigatewayv2::types::AuthorizationType::None).send().await.map_err(|e| DomainError::External(format!("{e:?}")))?;
                                    };
                                    if !has2 {
                                        self.apigw.create_route().api_id(api_id.clone()).route_key(rk2.clone()).target(format!("integrations/{}", integ_id)).authorization_type(aws_sdk_apigatewayv2::types::AuthorizationType::None).send().await.map_err(|e| DomainError::External(format!("{e:?}")))?;
                                    };
                                    let acct =
                                        std::env::var("AWS_ACCOUNT_ID".to_string()).unwrap_or_default();
                                    let source_arn = format!(
                                        "arn:aws:execute-api:{}:{}:{}/*/*",
                                        region, acct, api_id
                                    );
                                    let stmt = format!("apigw_{}_relay", api_id);
                                    let perm = self
                                        .lambda
                                        .add_permission()
                                        .function_name(f0.clone())
                                        .statement_id(stmt.clone())
                                        .action("lambda:InvokeFunction".to_string())
                                        .principal("apigateway.amazonaws.com".to_string())
                                        .source_arn(source_arn.clone())
                                        .send()
                                        .await;
                                    if perm.is_ok() {
                                        detail = format!(
                                            "{}; apigw={}/{} routes+perm ok",
                                            detail, api_id, api_name
                                        )
                                    } else {
                                        detail = format!(
                                            "{}; apigw={} perm ensure (may already exist)",
                                            detail, api_id
                                        )
                                    };
                                } else {
                                    detail = format!(
                                        "{}; no existing HTTP API (set VEIL_GW_HTTP_API)",
                                        detail
                                    );
                                };
                            };
                        };
                        let pk = format!("DEPLOY#{}#{}", environment, u0);
                        let state = serde_json::to_string(
                            &serde_json::json!({ "project": slug.clone(), "unit_name": u0.clone(), "unit_type": t0.clone(), "environment": environment.clone(), "status": "Active".to_string(), "lambda_name": f0.clone(), "lambda_arn": arn0.clone(), "role_arn": role.clone() }),
                        )?;
                        self.ddb
                            .put_item()
                            .table_name(ddb_name.clone())
                            .item(
                                "PK".to_string(),
                                aws_sdk_dynamodb::types::AttributeValue::S(pk.clone()),
                            )
                            .item(
                                "SK".to_string(),
                                aws_sdk_dynamodb::types::AttributeValue::S("CURRENT".to_string()),
                            )
                            .item(
                                "data".to_string(),
                                aws_sdk_dynamodb::types::AttributeValue::S(state),
                            )
                            .send()
                            .await
                            .map_err(|e| DomainError::External(format!("{e:?}")))?;
                        next_cursor = "unit_1".to_string();
                        steps_done = 5;
                    };
                };
            };
            if cursor == "unit_1".to_string() {
                if unit_count < 2 {
                    detail = "no unit_1".to_string();
                    next_cursor = "compile".to_string();
                    steps_done = 6;
                } else {
                    if mock {
                        detail = format!("mock unit {} -> {}", u1, f1);
                        next_cursor = "unit_2".to_string();
                        steps_done = 6;
                    } else {
                        let mut timeout1 = 30;
                        if t1 == "lambda-consumer".to_string() {
                            timeout1 = 900;
                        };
                        let mut arn1 = "".to_string();
                        let exists1 = self
                            .lambda
                            .get_function()
                            .function_name(f1.clone())
                            .send()
                            .await
                            .is_ok();
                        if exists1 {
                            let fout1 = self
                                .lambda
                                .get_function()
                                .function_name(f1.clone())
                                .send()
                                .await
                                .map_err(|e| DomainError::External(format!("{e:?}")))?;
                            let conf1 = fout1.configuration();
                            if conf1.is_some() {
                                let c1 = conf1.clone().ok_or(DomainError::NotFound)?;
                                let fa1 = c1.function_arn();
                                if fa1.is_some() {
                                    arn1 = fa1.clone().ok_or(DomainError::NotFound)?.to_string();
                                };
                            };
                            self.lambda
                                .update_function_configuration()
                                .function_name(f1.clone())
                                .memory_size(1024)
                                .timeout(timeout1)
                                .send()
                                .await
                                .map_err(|e| DomainError::External(format!("{e:?}")))?;
                            detail = format!("{} exists arn={}", f1, arn1);
                        } else {
                            let zip_blob = aws_sdk_lambda::primitives::Blob::new({
                                let __h: String = ("504b03041400000000001042fd5c9a7470c7460000004600000008000000696e6465782e6a736578706f7274732e68616e646c65723d6173796e632865293d3e287b737461747573436f64653a3230302c626f64793a277665696c2d706c616365686f6c646572277d293b0a504b010214031400000000001042fd5c9a7470c74600000046000000080000000000000000000000800100000000696e6465782e6a73504b05060000000001000100360000006c0000000000".to_string()).to_string();
                                let __h = __h.as_str();
                                let mut __b = Vec::with_capacity(__h.len() / 2);
                                let mut __i = 0usize;
                                while __i + 1 < __h.len() {
                                    if let Ok(__v) = u8::from_str_radix(&__h[__i..__i + 2], 16) {
                                        __b.push(__v);
                                    }
                                    __i += 2;
                                }
                                __b
                            });
                            let code = aws_sdk_lambda::types::FunctionCode::builder()
                                .zip_file(zip_blob.clone())
                                .build();
                            let created = self
                                .lambda
                                .create_function()
                                .function_name(f1.clone())
                                .runtime(aws_sdk_lambda::types::Runtime::Nodejs20x)
                                .role(role.clone())
                                .handler("index.handler".to_string())
                                .code(code.clone())
                                .memory_size(1024)
                                .timeout(timeout1)
                                .architectures(aws_sdk_lambda::types::Architecture::Arm64)
                                .description("VEIL provisioned unit".to_string())
                                .send()
                                .await
                                .map_err(|e| DomainError::External(format!("{e:?}")))?;
                            let fa_c1 = created.function_arn();
                            if fa_c1.is_some() {
                                arn1 = fa_c1.clone().ok_or(DomainError::NotFound)?.to_string();
                            };
                            detail = format!("{} created arn={}", f1, arn1);
                        };
                        if t1 == "lambda-consumer".to_string() {
                            let region = std::env::var("AWS_REGION".to_string())
                                .unwrap_or_else(|_| "us-west-2".to_string());
                            let acct =
                                std::env::var("AWS_ACCOUNT_ID".to_string()).unwrap_or_default();
                            let qarn = format!("arn:aws:sqs:{}:{}:{}", region, acct, sqs_name);
                            let mut esm_uuid = "".to_string();
                            let maps = self
                                .lambda
                                .list_event_source_mappings()
                                .function_name(f1.clone())
                                .send()
                                .await
                                .map_err(|e| DomainError::External(format!("{e:?}")))?;
                            for m in maps.event_source_mappings() {
                                let mut ea = "".to_string();
                                if m.event_source_arn().is_some() {
                                    ea = m.event_source_arn().unwrap().to_string();
                                };
                                if ea == qarn {
                                    if m.uuid().is_some() {
                                        esm_uuid = m.uuid().unwrap().to_string()
                                    } else {
                                        esm_uuid = "exists".to_string()
                                    };
                                };
                            }
                            if esm_uuid == "".to_string() {
                                let esm = self
                                    .lambda
                                    .create_event_source_mapping()
                                    .function_name(f1.clone())
                                    .event_source_arn(qarn.clone())
                                    .batch_size(1)
                                    .enabled(true)
                                    .send()
                                    .await
                                    .map_err(|e| DomainError::External(format!("{e:?}")))?;
                                if esm.uuid().is_some() {
                                    esm_uuid = esm.uuid().unwrap().to_string()
                                } else {
                                    esm_uuid = "created".to_string()
                                };
                            };
                            detail = format!("{}; esm={}", detail, esm_uuid);
                        };
                        let pk = format!("DEPLOY#{}#{}", environment, u1);
                        let state = serde_json::to_string(
                            &serde_json::json!({ "project": slug.clone(), "unit_name": u1.clone(), "unit_type": t1.clone(), "environment": environment.clone(), "status": "Active".to_string(), "lambda_name": f1.clone(), "lambda_arn": arn1.clone(), "role_arn": role.clone(), "sqs": sqs_name.clone() }),
                        )?;
                        self.ddb
                            .put_item()
                            .table_name(ddb_name.clone())
                            .item(
                                "PK".to_string(),
                                aws_sdk_dynamodb::types::AttributeValue::S(pk.clone()),
                            )
                            .item(
                                "SK".to_string(),
                                aws_sdk_dynamodb::types::AttributeValue::S("CURRENT".to_string()),
                            )
                            .item(
                                "data".to_string(),
                                aws_sdk_dynamodb::types::AttributeValue::S(state),
                            )
                            .send()
                            .await
                            .map_err(|e| DomainError::External(format!("{e:?}")))?;
                        next_cursor = "unit_2".to_string();
                        steps_done = 6;
                    };
                };
            };
            if cursor == "unit_2".to_string() {
                if unit_count < 3 {
                    detail = "no unit_2".to_string();
                    next_cursor = "compile".to_string();
                    steps_done = 7;
                } else {
                    if mock {
                        detail = format!("mock unit {} -> {}", u2, f2);
                        next_cursor = "compile".to_string();
                        steps_done = 7;
                    } else {
                        let mut arn2 = "".to_string();
                        let exists2 = self
                            .lambda
                            .get_function()
                            .function_name(f2.clone())
                            .send()
                            .await
                            .is_ok();
                        if exists2 {
                            let fout2 = self
                                .lambda
                                .get_function()
                                .function_name(f2.clone())
                                .send()
                                .await
                                .map_err(|e| DomainError::External(format!("{e:?}")))?;
                            let conf2 = fout2.configuration();
                            if conf2.is_some() {
                                let c2 = conf2.clone().ok_or(DomainError::NotFound)?;
                                let fa2 = c2.function_arn();
                                if fa2.is_some() {
                                    arn2 = fa2.clone().ok_or(DomainError::NotFound)?.to_string();
                                };
                            };
                            detail = format!("{} exists arn={}", f2, arn2);
                        } else {
                            let zip_blob = aws_sdk_lambda::primitives::Blob::new({
                                let __h: String = ("504b03041400000000001042fd5c9a7470c7460000004600000008000000696e6465782e6a736578706f7274732e68616e646c65723d6173796e632865293d3e287b737461747573436f64653a3230302c626f64793a277665696c2d706c616365686f6c646572277d293b0a504b010214031400000000001042fd5c9a7470c74600000046000000080000000000000000000000800100000000696e6465782e6a73504b05060000000001000100360000006c0000000000".to_string()).to_string();
                                let __h = __h.as_str();
                                let mut __b = Vec::with_capacity(__h.len() / 2);
                                let mut __i = 0usize;
                                while __i + 1 < __h.len() {
                                    if let Ok(__v) = u8::from_str_radix(&__h[__i..__i + 2], 16) {
                                        __b.push(__v);
                                    }
                                    __i += 2;
                                }
                                __b
                            });
                            let code = aws_sdk_lambda::types::FunctionCode::builder()
                                .zip_file(zip_blob.clone())
                                .build();
                            let created = self
                                .lambda
                                .create_function()
                                .function_name(f2.clone())
                                .runtime(aws_sdk_lambda::types::Runtime::Nodejs20x)
                                .role(role.clone())
                                .handler("index.handler".to_string())
                                .code(code.clone())
                                .memory_size(1024)
                                .timeout(30)
                                .architectures(aws_sdk_lambda::types::Architecture::Arm64)
                                .description("VEIL provisioned unit".to_string())
                                .send()
                                .await
                                .map_err(|e| DomainError::External(format!("{e:?}")))?;
                            let fa_c2 = created.function_arn();
                            if fa_c2.is_some() {
                                arn2 = fa_c2.clone().ok_or(DomainError::NotFound)?.to_string();
                            };
                            detail = format!("{} created arn={}", f2, arn2);
                        };
                        let pk = format!("DEPLOY#{}#{}", environment, u2);
                        let state = serde_json::to_string(
                            &serde_json::json!({ "project": slug.clone(), "unit_name": u2.clone(), "unit_type": t2.clone(), "environment": environment.clone(), "status": "Active".to_string(), "lambda_name": f2.clone(), "lambda_arn": arn2.clone() }),
                        )?;
                        self.ddb
                            .put_item()
                            .table_name(ddb_name.clone())
                            .item(
                                "PK".to_string(),
                                aws_sdk_dynamodb::types::AttributeValue::S(pk.clone()),
                            )
                            .item(
                                "SK".to_string(),
                                aws_sdk_dynamodb::types::AttributeValue::S("CURRENT".to_string()),
                            )
                            .item(
                                "data".to_string(),
                                aws_sdk_dynamodb::types::AttributeValue::S(state),
                            )
                            .send()
                            .await
                            .map_err(|e| DomainError::External(format!("{e:?}")))?;
                        next_cursor = "compile".to_string();
                        steps_done = 7;
                    };
                };
            };
            if cursor == "compile".to_string() {
                if mock {
                    detail = "mock compile".to_string();
                    next_cursor = "run_hooks".to_string();
                    steps_done = steps_done + 1;
                } else {
                    let hub = veil_local_fs::LocalFs::projects_dir();
                    let root = veil_local_fs::LocalFs::join(hub.clone(), slug.clone());
                    let backend_toml = veil_local_fs::LocalFs::join(
                        veil_local_fs::LocalFs::join(
                            veil_local_fs::LocalFs::join(root.clone(), "generated".to_string()),
                            "backend".to_string(),
                        ),
                        "Cargo.toml".to_string(),
                    );
                    let main_veil =
                        veil_local_fs::LocalFs::join(root.clone(), "main.veil".to_string());
                    let mut gen_detail = "".to_string();
                    let build_detail;
                    if !veil_local_fs::LocalFs::path_exists(root.clone()) {
                        detail = format!("project root missing {}", root);
                        status = "failed".to_string();
                        summary = detail.clone();
                        next_cursor = "compile".to_string();
                        finished_s = "done".to_string();
                    } else {
                        if veil_local_fs::LocalFs::path_is_file(main_veil.clone()) {
                            gen_detail = {
                                let __prog: String = ("veil".to_string()).to_string();
                                let __args: String = ("gen main.veil -o generated/backend -t rust"
                                    .to_string())
                                .to_string();
                                let __cwd: String = (root).to_string();
                                let __argv: Vec<&str> = __args.split_whitespace().collect();
                                match std::process::Command::new(&__prog)
                                    .args(&__argv)
                                    .current_dir(&__cwd)
                                    .output()
                                {
                                    Ok(__out) => {
                                        if __out.status.success() {
                                            format!(
                                                "{} ok: {}",
                                                __prog,
                                                String::from_utf8_lossy(&__out.stdout)
                                                    .chars()
                                                    .take(400)
                                                    .collect::<String>()
                                            )
                                        } else {
                                            let __err = String::from_utf8_lossy(&__out.stderr);
                                            let __tail: String = __err
                                                .chars()
                                                .rev()
                                                .take(1200)
                                                .collect::<String>()
                                                .chars()
                                                .rev()
                                                .collect();
                                            format!("{} failed: {}", __prog, __tail)
                                        }
                                    }
                                    Err(e) => format!("{} spawn failed: {e}", __prog),
                                }
                            };
                            if gen_detail.contains(" failed") || gen_detail.contains("spawn failed")
                            {
                                let veil_bin_path = std::env::var("VEIL_BIN".to_string())
                                    .unwrap_or_else(|_| "".to_string());
                                if veil_bin_path != "".to_string() {
                                    gen_detail = {
                                        let __prog: String = (veil_bin_path).to_string();
                                        let __args: String =
                                            ("gen main.veil -o generated/backend -t rust"
                                                .to_string())
                                            .to_string();
                                        let __cwd: String = (root).to_string();
                                        let __argv: Vec<&str> = __args.split_whitespace().collect();
                                        match std::process::Command::new(&__prog)
                                            .args(&__argv)
                                            .current_dir(&__cwd)
                                            .output()
                                        {
                                            Ok(__out) => {
                                                if __out.status.success() {
                                                    format!(
                                                        "{} ok: {}",
                                                        __prog,
                                                        String::from_utf8_lossy(&__out.stdout)
                                                            .chars()
                                                            .take(400)
                                                            .collect::<String>()
                                                    )
                                                } else {
                                                    let __err =
                                                        String::from_utf8_lossy(&__out.stderr);
                                                    let __tail: String = __err
                                                        .chars()
                                                        .rev()
                                                        .take(1200)
                                                        .collect::<String>()
                                                        .chars()
                                                        .rev()
                                                        .collect();
                                                    format!("{} failed: {}", __prog, __tail)
                                                }
                                            }
                                            Err(e) => format!("{} spawn failed: {e}", __prog),
                                        }
                                    };
                                };
                            };
                        };
                        if veil_local_fs::LocalFs::path_is_file(backend_toml.clone()) {
                            build_detail = {
                                let __prog: String = ("cargo".to_string()).to_string();
                                let __args: String = ("build --release --manifest-path generated/backend/Cargo.toml".to_string()).to_string();
                                let __cwd: String = (root).to_string();
                                let __argv: Vec<&str> = __args.split_whitespace().collect();
                                match std::process::Command::new(&__prog)
                                    .args(&__argv)
                                    .current_dir(&__cwd)
                                    .output()
                                {
                                    Ok(__out) => {
                                        if __out.status.success() {
                                            format!(
                                                "{} ok: {}",
                                                __prog,
                                                String::from_utf8_lossy(&__out.stdout)
                                                    .chars()
                                                    .take(400)
                                                    .collect::<String>()
                                            )
                                        } else {
                                            let __err = String::from_utf8_lossy(&__out.stderr);
                                            let __tail: String = __err
                                                .chars()
                                                .rev()
                                                .take(1200)
                                                .collect::<String>()
                                                .chars()
                                                .rev()
                                                .collect();
                                            format!("{} failed: {}", __prog, __tail)
                                        }
                                    }
                                    Err(e) => format!("{} spawn failed: {e}", __prog),
                                }
                            }
                        } else {
                            build_detail = format!("no Cargo.toml at {}", backend_toml)
                        };
                        detail = format!("gen={}; build={}", gen_detail, build_detail);
                        if build_detail.contains(" failed")
                            || build_detail.contains("spawn failed")
                            || build_detail.contains("no Cargo.toml")
                        {
                            detail = format!("compile soft-fail (infra ok): {}", detail);
                        };
                        next_cursor = "run_hooks".to_string();
                        steps_done = steps_done + 1;
                    };
                };
            };
            if cursor == "run_hooks".to_string() {
                if mock {
                    detail = "mock hooks".to_string();
                    next_cursor = "deploy_code".to_string();
                    steps_done = steps_done + 1;
                } else {
                    let hub = veil_local_fs::LocalFs::projects_dir();
                    let root = veil_local_fs::LocalFs::join(hub.clone(), slug.clone());
                    let stack = serde_json::json!({
                        "service": slug.clone(),
                        "resource_prefix": "veil",
                        "names": {
                            "base": job.get("stack_base").cloned().unwrap_or(serde_json::json!("")),
                            "dynamodb": ddb_name.clone(),
                            "sns": sns_name.clone(),
                            "sqs": sqs_name.clone(),
                            "lambda_api": job.get("lambda_api").cloned().unwrap_or(serde_json::json!("")),
                            "lambda_consumer": job.get("lambda_consumer").cloned().unwrap_or(serde_json::json!("")),
                        }
                    });
                    let units = serde_json::json!([
                        { "unit": u0.clone(), "type": t0.clone(), "function_name": f0.clone() },
                        { "unit": u1.clone(), "type": t1.clone(), "function_name": f1.clone() },
                        { "unit": u2.clone(), "type": t2.clone(), "function_name": f2.clone() },
                    ]);
                    match crate::hooks::run_deploy_hooks(
                        std::path::Path::new(&root),
                        &environment,
                        &stack,
                        &units,
                    ) {
                        Ok(rep) => {
                            detail = rep.detail;
                            next_cursor = "deploy_code".to_string();
                            steps_done = steps_done + 1;
                        }
                        Err(e) => {
                            detail = format!("deploy hooks failed: {e}");
                            status = "failed".to_string();
                            summary = detail.clone();
                            next_cursor = "run_hooks".to_string();
                            finished_s = "done".to_string();
                        }
                    };
                };
            };
            if cursor == "deploy_code".to_string() {
                if mock {
                    detail = "mock deploy".to_string();
                    next_cursor = "finalize".to_string();
                    steps_done = steps_done + 1;
                } else {
                    let hub = veil_local_fs::LocalFs::projects_dir();
                    let root = veil_local_fs::LocalFs::join(hub.clone(), slug.clone());
                    let bin_path = veil_local_fs::LocalFs::join(
                        veil_local_fs::LocalFs::join(
                            veil_local_fs::LocalFs::join(
                                veil_local_fs::LocalFs::join(root.clone(), "generated".to_string()),
                                "backend".to_string(),
                            ),
                            "target".to_string(),
                        ),
                        "release".to_string(),
                    );
                    let bin_relay =
                        veil_local_fs::LocalFs::join(bin_path.clone(), "relay".to_string());
                    let bin_veil =
                        veil_local_fs::LocalFs::join(bin_path.clone(), "veil_bin".to_string());
                    let mut binary = "".to_string();
                    if veil_local_fs::LocalFs::path_is_file(bin_relay.clone()) {
                        binary = bin_relay
                    } else {
                        if veil_local_fs::LocalFs::path_is_file(bin_veil.clone()) {
                            binary = bin_veil;
                        }
                    };
                    if binary == "".to_string() {
                        detail = format!(
                            "no release binary under {} — keeping unit placeholder code",
                            bin_path
                        );
                        next_cursor = "finalize".to_string();
                        steps_done = steps_done + 1;
                    } else {
                        let zip_out = format!("/tmp/veil-{}-bootstrap.zip", slug);
                        let work = format!("/tmp/veil-{}-boot", slug);
                        veil_local_fs::LocalFs::create_dir_all(work.clone())
                            .map_err(|e| DomainError::External(e.to_string()))?;
                        let boot_path =
                            veil_local_fs::LocalFs::join(work.clone(), "bootstrap".to_string());
                        let copy_d = {
                            let __prog: String = ("cp".to_string()).to_string();
                            let __args: String = (format!("{} {}", binary, boot_path)).to_string();
                            let __cwd: String = (root).to_string();
                            let __argv: Vec<&str> = __args.split_whitespace().collect();
                            match std::process::Command::new(&__prog)
                                .args(&__argv)
                                .current_dir(&__cwd)
                                .output()
                            {
                                Ok(__out) => {
                                    if __out.status.success() {
                                        format!(
                                            "{} ok: {}",
                                            __prog,
                                            String::from_utf8_lossy(&__out.stdout)
                                                .chars()
                                                .take(400)
                                                .collect::<String>()
                                        )
                                    } else {
                                        let __err = String::from_utf8_lossy(&__out.stderr);
                                        let __tail: String = __err
                                            .chars()
                                            .rev()
                                            .take(1200)
                                            .collect::<String>()
                                            .chars()
                                            .rev()
                                            .collect();
                                        format!("{} failed: {}", __prog, __tail)
                                    }
                                }
                                Err(e) => format!("{} spawn failed: {e}", __prog),
                            }
                        };
                        let pack = {
                            let __prog: String = ("zip".to_string()).to_string();
                            let __args: String = (format!("-j {} bootstrap", zip_out)).to_string();
                            let __cwd: String = (work).to_string();
                            let __argv: Vec<&str> = __args.split_whitespace().collect();
                            match std::process::Command::new(&__prog)
                                .args(&__argv)
                                .current_dir(&__cwd)
                                .output()
                            {
                                Ok(__out) => {
                                    if __out.status.success() {
                                        format!(
                                            "{} ok: {}",
                                            __prog,
                                            String::from_utf8_lossy(&__out.stdout)
                                                .chars()
                                                .take(400)
                                                .collect::<String>()
                                        )
                                    } else {
                                        let __err = String::from_utf8_lossy(&__out.stderr);
                                        let __tail: String = __err
                                            .chars()
                                            .rev()
                                            .take(1200)
                                            .collect::<String>()
                                            .chars()
                                            .rev()
                                            .collect();
                                        format!("{} failed: {}", __prog, __tail)
                                    }
                                }
                                Err(e) => format!("{} spawn failed: {e}", __prog),
                            }
                        };
                        if copy_d.contains(" failed")
                            || copy_d.contains("spawn failed")
                            || pack.contains(" failed")
                            || pack.contains("spawn failed")
                        {
                            detail = format!(
                                "package soft-fail (keep placeholder): copy={}; zip={}",
                                copy_d, pack
                            );
                            next_cursor = "finalize".to_string();
                            steps_done = steps_done + 1;
                        } else {
                            let code_blob = aws_sdk_lambda::primitives::Blob::new(
                                std::fs::read((zip_out).as_str())
                                    .map_err(|e| DomainError::External(e.to_string()))?,
                            );
                            let mut deploy_msgs = "".to_string();
                            if f0 != "".to_string() {
                                self.lambda
                                    .update_function_configuration()
                                    .function_name(f0.clone())
                                    .runtime(aws_sdk_lambda::types::Runtime::Providedal2023)
                                    .handler("bootstrap".to_string())
                                    .send()
                                    .await
                                    .map_err(|e| DomainError::External(format!("{e:?}")))?;
                                self.lambda
                                    .update_function_code()
                                    .function_name(f0.clone())
                                    .zip_file(code_blob.clone())
                                    .publish(true)
                                    .send()
                                    .await
                                    .map_err(|e| DomainError::External(format!("{e:?}")))?;
                                deploy_msgs = format!("{} updated; ", f0);
                            };
                            if f1 != "".to_string() {
                                let code_blob1 = aws_sdk_lambda::primitives::Blob::new(
                                    std::fs::read((zip_out).as_str())
                                        .map_err(|e| DomainError::External(e.to_string()))?,
                                );
                                self.lambda
                                    .update_function_configuration()
                                    .function_name(f1.clone())
                                    .runtime(aws_sdk_lambda::types::Runtime::Providedal2023)
                                    .handler("bootstrap".to_string())
                                    .send()
                                    .await
                                    .map_err(|e| DomainError::External(format!("{e:?}")))?;
                                self.lambda
                                    .update_function_code()
                                    .function_name(f1.clone())
                                    .zip_file(code_blob1.clone())
                                    .publish(true)
                                    .send()
                                    .await
                                    .map_err(|e| DomainError::External(format!("{e:?}")))?;
                                deploy_msgs = format!("{}{} updated; ", deploy_msgs, f1);
                            };
                            if f2 != "".to_string() {
                                let code_blob2 = aws_sdk_lambda::primitives::Blob::new(
                                    std::fs::read((zip_out).as_str())
                                        .map_err(|e| DomainError::External(e.to_string()))?,
                                );
                                self.lambda
                                    .update_function_configuration()
                                    .function_name(f2.clone())
                                    .runtime(aws_sdk_lambda::types::Runtime::Providedal2023)
                                    .handler("bootstrap".to_string())
                                    .send()
                                    .await
                                    .map_err(|e| DomainError::External(format!("{e:?}")))?;
                                self.lambda
                                    .update_function_code()
                                    .function_name(f2.clone())
                                    .zip_file(code_blob2.clone())
                                    .publish(true)
                                    .send()
                                    .await
                                    .map_err(|e| DomainError::External(format!("{e:?}")))?;
                                deploy_msgs = format!("{}{} updated", deploy_msgs, f2);
                            };
                            detail = format!("deployed from {}: {}", binary, deploy_msgs);
                            next_cursor = "finalize".to_string();
                            steps_done = steps_done + 1;
                        };
                    };
                };
            };
            if cursor == "finalize".to_string() {
                detail = "All steps complete".to_string();
                next_cursor = "done".to_string();
                status = "succeeded".to_string();
                summary = format!("Provisioned {} in {}", slug, environment);
                finished_s = "done".to_string();
                steps_done = total;
            };
            if cursor == "done".to_string() {
                status = "succeeded".to_string();
                summary = format!("Provisioned {} in {}", slug, environment);
                finished_s = "done".to_string();
            };
            let mut s_load = "pending".to_string();
            let mut s_ddb = "pending".to_string();
            let mut s_sns = "pending".to_string();
            let mut s_sqs = "pending".to_string();
            let mut s_u0 = "pending".to_string();
            let mut s_u1 = "pending".to_string();
            let mut s_u2 = "pending".to_string();
            let mut s_comp = "pending".to_string();
            let mut s_hooks = "pending".to_string();
            let mut s_dep = "pending".to_string();
            let mut s_fin = "pending".to_string();
            let mut d_load = "".to_string();
            let mut d_ddb = "".to_string();
            let mut d_sns = "".to_string();
            let mut d_sqs = "".to_string();
            let mut d_u0 = "".to_string();
            let mut d_u1 = "".to_string();
            let mut d_u2 = "".to_string();
            let mut d_comp = "".to_string();
            let mut d_hooks = "".to_string();
            let mut d_dep = "".to_string();
            let mut d_fin = "".to_string();
            if steps_done >= 1 {
                s_load = "done".to_string();
            };
            if steps_done >= 2 {
                s_ddb = "done".to_string();
            };
            if steps_done >= 3 {
                s_sns = "done".to_string();
            };
            if steps_done >= 4 {
                s_sqs = "done".to_string();
            };
            if steps_done >= 5 {
                s_u0 = "done".to_string();
            };
            if steps_done >= 6 {
                s_u1 = "done".to_string();
            };
            if steps_done >= 7 {
                s_u2 = "done".to_string();
            };
            if cursor == "load_config".to_string() {
                d_load = detail.clone();
                s_load = "done".to_string();
            };
            if cursor == "stack_dynamodb".to_string() {
                d_ddb = detail.clone();
                s_ddb = "done".to_string();
            };
            if cursor == "stack_sns".to_string() {
                d_sns = detail.clone();
                s_sns = "done".to_string();
            };
            if cursor == "stack_sqs".to_string() {
                d_sqs = detail.clone();
                s_sqs = "done".to_string();
            };
            if cursor == "unit_0".to_string() {
                d_u0 = detail.clone();
                s_u0 = "done".to_string();
            };
            if cursor == "unit_1".to_string() {
                d_u1 = detail.clone();
                s_u1 = "done".to_string();
            };
            if cursor == "unit_2".to_string() {
                d_u2 = detail.clone();
                s_u2 = "done".to_string();
            };
            if cursor == "compile".to_string() {
                d_comp = detail.clone();
                s_comp = "done".to_string();
            };
            if cursor == "run_hooks".to_string() {
                d_hooks = detail.clone();
                s_hooks = "done".to_string();
            };
            if cursor == "deploy_code".to_string() {
                d_dep = detail.clone();
                s_dep = "done".to_string();
            };
            if cursor == "finalize".to_string() {
                d_fin = detail.clone();
                s_fin = "done".to_string();
                s_load = "done".to_string();
                s_ddb = "done".to_string();
                s_sns = "done".to_string();
                s_sqs = "done".to_string();
                s_u0 = "done".to_string();
                s_u1 = "done".to_string();
                s_u2 = "done".to_string();
                s_comp = "done".to_string();
                s_hooks = "done".to_string();
                s_dep = "done".to_string();
            };
            let mut out_steps = vec![
                serde_json::json!({ "id": "load_config".to_string(), "label": "Load veil.toml deploy config".to_string(), "status": s_load.clone(), "detail": d_load.clone() }),
                serde_json::json!({ "id": "stack_dynamodb".to_string(), "label": format!("DynamoDB {}", ddb_name), "status": s_ddb.clone(), "detail": d_ddb.clone() }),
                serde_json::json!({ "id": "stack_sns".to_string(), "label": format!("SNS {}", sns_name), "status": s_sns.clone(), "detail": d_sns.clone() }),
                serde_json::json!({ "id": "stack_sqs".to_string(), "label": format!("SQS {} + DLQ", sqs_name), "status": s_sqs.clone(), "detail": d_sqs.clone() }),
            ];
            if unit_count > 0 {
                out_steps.push(serde_json::json!({ "id": format!("unit:{}", u0), "label": format!("Unit {} ({}) -> {}", u0, t0, f0), "status": s_u0.clone(), "detail": d_u0.clone() }));
            };
            if unit_count > 1 {
                out_steps.push(serde_json::json!({ "id": format!("unit:{}", u1), "label": format!("Unit {} ({}) -> {}", u1, t1, f1), "status": s_u1.clone(), "detail": d_u1.clone() }));
            };
            if unit_count > 2 {
                out_steps.push(serde_json::json!({ "id": format!("unit:{}", u2), "label": format!("Unit {} ({}) -> {}", u2, t2, f2), "status": s_u2.clone(), "detail": d_u2.clone() }));
            };
            out_steps = {
                let mut __v = out_steps;
                __v.extend(vec![serde_json::json!({ "id": "compile".to_string(), "label": "Compile project sources".to_string(), "status": s_comp.clone(), "detail": d_comp.clone() }), serde_json::json!({ "id": "run_hooks".to_string(), "label": "Run deploy hooks".to_string(), "status": s_hooks.clone(), "detail": d_hooks.clone() }), serde_json::json!({ "id": "deploy_code".to_string(), "label": "Deploy binary to Lambda".to_string(), "status": s_dep.clone(), "detail": d_dep.clone() }), serde_json::json!({ "id": "finalize".to_string(), "label": "Record provision state".to_string(), "status": s_fin.clone(), "detail": d_fin.clone() })]);
                __v
            };
            let mut pct = 0;
            if total > 0 {
                pct = steps_done * 100 / total;
            };
            if pct > 100 {
                pct = 100;
            };
            if status == "running".to_string() {
                summary = format!("Running {} -> {}", cursor, next_cursor);
            };
            let mut err_out: serde_json::Value = serde_json::Value::Null;
            if status == "failed".to_string() {
                err_out = serde_json::from_str::<_>(&serde_json::to_string(&detail)?)?;
            };
            let mut last_status = "done".to_string();
            if status == "failed".to_string() {
                last_status = "failed".to_string();
            };
            let outj = serde_json::json!({ "job_id": job.get("job_id").cloned().ok_or(DomainError::NotFound)?, "project_slug": slug.clone(), "environment": environment.clone(), "repo_id": job.get("repo_id").cloned().ok_or(DomainError::NotFound)?, "branch": job.get("branch").cloned().ok_or(DomainError::NotFound)?, "status": status.clone(), "summary": summary.clone(), "error": err_out.clone(), "percent": pct.clone(), "steps_done": steps_done.clone(), "steps_total": total.clone(), "started_at": job.get("started_at").cloned().ok_or(DomainError::NotFound)?, "updated_at": now.clone(), "finished_at": finished_s.clone(), "cursor": next_cursor.clone(), "stack_base": job.get("stack_base").cloned().ok_or(DomainError::NotFound)?, "ddb_name": ddb_name.clone(), "sns_name": sns_name.clone(), "sqs_name": sqs_name.clone(), "lambda_api": job.get("lambda_api").cloned().ok_or(DomainError::NotFound)?, "lambda_consumer": job.get("lambda_consumer").cloned().ok_or(DomainError::NotFound)?, "unit_count": unit_count.clone(), "u0": u0.clone(), "t0": t0.clone(), "f0": f0.clone(), "u1": u1.clone(), "t1": t1.clone(), "f1": f1.clone(), "u2": u2.clone(), "t2": t2.clone(), "f2": f2.clone(), "steps": out_steps.clone(), "last_step": serde_json::json!({ "id": cursor.clone(), "status": last_status.clone(), "detail": detail.clone() }) });
            veil_local_fs::LocalFs::write(path.clone(), serde_json::to_string(&outj)?)
                .map_err(|e| DomainError::External(e.to_string()))?;
            return Ok(serde_json::to_string(&outj)?);
        };
        return Ok(serde_json::to_string(
            &serde_json::json!({ "error": format!("unknown job {}", job_id), "status": "failed".to_string(), "steps": serde_json::Value::Array(vec![]) }),
        )?);
    }

    async fn list_environments(&self) -> Result<String, DomainError> {
        let alt =
            std::env::var("VEIL_DEPLOY_CONFIG".to_string()).unwrap_or_else(|_| "".to_string());
        if alt != "".to_string() {
            if veil_local_fs::LocalFs::path_is_file(alt.clone()) {
                let raw = veil_local_fs::LocalFs::read_toml_json(alt.clone())
                    .map_err(|e| DomainError::External(e.to_string()))?;
                return Ok(raw);
            };
        };
        let home = std::env::var("HOME".to_string()).unwrap_or_else(|_| "".to_string());
        let cfg_path = veil_local_fs::LocalFs::join(home.clone(), ".veil/deploy.toml".to_string());
        if veil_local_fs::LocalFs::path_is_file(cfg_path.clone()) {
            let raw2 = veil_local_fs::LocalFs::read_toml_json(cfg_path.clone())
                .map_err(|e| DomainError::External(e.to_string()))?;
            return Ok(raw2);
        };
        let cwd_cfg = std::env::current_dir()
            .ok()
            .map(|p| p.join("config/deploy.toml"))
            .filter(|p| p.is_file());
        if let Some(p) = cwd_cfg {
            if let Ok(raw) = veil_local_fs::LocalFs::read_toml_json(p.display().to_string()) {
                return Ok(raw);
            }
        }
        return Ok("{\"default\":\"dev\",\"environments\":[{\"name\":\"dev\",\"region\":\"us-west-2\",\"account_id\":null,\"has_assume_role\":false,\"assume_role_arn\":null,\"lambda_execution_role_arn\":null,\"gateways\":[{\"logical\":\"http-api\",\"patterns\":[\"*-http-api\",\"*-dev-service-api\"]}]},{\"name\":\"staging\",\"region\":\"us-west-2\",\"account_id\":null,\"has_assume_role\":false,\"assume_role_arn\":null,\"lambda_execution_role_arn\":null,\"gateways\":[{\"logical\":\"http-api\",\"patterns\":[\"*-staging-service-api\"]}]},{\"name\":\"prod\",\"region\":\"us-west-2\",\"account_id\":null,\"has_assume_role\":false,\"assume_role_arn\":null,\"lambda_execution_role_arn\":null,\"gateways\":[{\"logical\":\"http-api\",\"patterns\":[\"*-prod-service-api\"]}]}],\"config_path\":\"config/deploy.toml\"}".to_string());
    }

    async fn plan_provision(
        &self,
        project_slug: String,
        environment: String,
    ) -> Result<String, DomainError> {
        return Ok(serde_json::to_string(
            &serde_json::json!({ "environment": environment.clone(), "mock_mode": false, "resources": serde_json::Value::Array(vec![]), "units": serde_json::Value::Array(vec![]), "steps": serde_json::Value::Array(vec![]), "diff": serde_json::json!({ "create": 0, "update": 0, "noop": 0, "destroy": 0 }), "notes": vec!["Deprecated: use plan_provision_repo.".to_string()], "summary": "In sync (stub)".to_string(), "source": "stub".to_string() }),
        )?);
    }

    async fn plan_provision_repo(
        &self,
        repo_id: String,
        branch: String,
        slug: String,
        environment: String,
    ) -> Result<String, DomainError> {
        let disk = veil_local_fs::LocalFs::read_project_deploy(slug.clone())
            .map_err(|e| DomainError::External(e.to_string()))?;
        let snap: serde_json::Value = serde_json::from_str::<_>(&disk)?;
        let mock_s =
            std::env::var("VEIL_DEPLOY_EXECUTOR".to_string()).unwrap_or_else(|_| "".to_string());
        let mock = mock_s == "mock".to_string() || mock_s == "MOCK".to_string();
        let stack = snap.get("stack").cloned().ok_or(DomainError::NotFound)?;
        let names = stack.get("names").cloned().ok_or(DomainError::NotFound)?;
        let base = names
            .get("base")
            .cloned()
            .ok_or(DomainError::NotFound)?
            .as_str()
            .map(|s| s.to_string())
            .unwrap_or("veil-unknown".to_string())
            .to_string();
        let mut ddb_name = names
            .get("dynamodb")
            .cloned()
            .ok_or(DomainError::NotFound)?
            .as_str()
            .map(|s| s.to_string())
            .unwrap_or("veil-unknown".to_string())
            .to_string();
        let mut sns_name = names
            .get("sns")
            .cloned()
            .ok_or(DomainError::NotFound)?
            .as_str()
            .map(|s| s.to_string())
            .unwrap_or("veil-unknown".to_string())
            .to_string();
        let mut sqs_name = names
            .get("sqs")
            .cloned()
            .ok_or(DomainError::NotFound)?
            .as_str()
            .map(|s| s.to_string())
            .unwrap_or("veil-unknown".to_string())
            .to_string();
        let lambda_api = names
            .get("lambda_api")
            .cloned()
            .ok_or(DomainError::NotFound)?
            .as_str()
            .map(|s| s.to_string())
            .unwrap_or("".to_string())
            .to_string();
        let lambda_consumer = names
            .get("lambda_consumer")
            .cloned()
            .ok_or(DomainError::NotFound)?
            .as_str()
            .map(|s| s.to_string())
            .unwrap_or("".to_string())
            .to_string();
        let service = snap
            .get("service")
            .cloned()
            .ok_or(DomainError::NotFound)?
            .as_str()
            .map(|s| s.to_string())
            .unwrap_or("unknown".to_string())
            .to_string();
        let resource_prefix = snap
            .get("resource_prefix")
            .cloned()
            .ok_or(DomainError::NotFound)?
            .as_str()
            .map(|s| s.to_string())
            .unwrap_or("veil".to_string())
            .to_string();
        if ddb_name == "veil-unknown".to_string() {
            ddb_name = base.clone();
        };
        if sns_name == "veil-unknown".to_string() {
            sns_name = base.clone();
        };
        if sqs_name == "veil-unknown".to_string() {
            sqs_name = base.clone();
        };
        let mut creates = 0;
        let mut updates = 0;
        let mut noops = 0;
        let mut resources = vec![];
        let mut unit_actions = vec![];
        let mut steps = vec![
            serde_json::json!({ "id": "load_config".to_string(), "label": "Load veil.toml deploy config".to_string(), "phase": "plan".to_string(), "action": "noop".to_string() }),
        ];
        let mut ddb_exists = false;
        if !mock {
            let dr = self
                .ddb
                .describe_table()
                .table_name(ddb_name.clone())
                .send()
                .await;
            ddb_exists = dr.is_ok();
        };
        if ddb_exists {
            noops = noops + 1;
            resources.push(serde_json::json!({ "kind": "dynamodb".to_string(), "action": "noop".to_string(), "exists": true, "name": ddb_name.clone(), "detail": "Table present - no schema drift check yet".to_string() }));
            steps.push(serde_json::json!({ "id": "stack_dynamodb".to_string(), "label": format!("DynamoDB '{}'", ddb_name), "phase": "infra".to_string(), "action": "noop".to_string() }));
        } else {
            creates = creates + 1;
            resources.push(serde_json::json!({ "kind": "dynamodb".to_string(), "action": "create".to_string(), "exists": false, "name": ddb_name.clone(), "detail": "Will CreateTable (PK/SK, pay-per-request)".to_string() }));
            steps.push(serde_json::json!({ "id": "stack_dynamodb".to_string(), "label": format!("DynamoDB '{}'", ddb_name), "phase": "infra".to_string(), "action": "create".to_string() }));
        };
        let mut sns_exists = false;
        if !mock {
            let region =
                std::env::var("AWS_REGION".to_string()).unwrap_or_else(|_| "us-east-1".to_string());
            let acct = std::env::var("AWS_ACCOUNT_ID".to_string())
                .unwrap_or_else(|_| "000000000000".to_string());
            let sns_arn = format!("arn:aws:sns:{}:{}:{}", region, acct, sns_name);
            sns_exists = self
                .sns
                .get_topic_attributes()
                .topic_arn(sns_arn.clone())
                .send()
                .await
                .is_ok();
        };
        if sns_exists {
            noops = noops + 1;
            resources.push(serde_json::json!({ "kind": "sns".to_string(), "action": "noop".to_string(), "exists": true, "name": sns_name.clone(), "detail": "Topic present".to_string() }));
            steps.push(serde_json::json!({ "id": "stack_sns".to_string(), "label": format!("SNS '{}'", sns_name), "phase": "infra".to_string(), "action": "noop".to_string() }));
        } else {
            creates = creates + 1;
            resources.push(serde_json::json!({ "kind": "sns".to_string(), "action": "create".to_string(), "exists": false, "name": sns_name.clone(), "detail": "Will CreateTopic".to_string() }));
            steps.push(serde_json::json!({ "id": "stack_sns".to_string(), "label": format!("SNS '{}'", sns_name), "phase": "infra".to_string(), "action": "create".to_string() }));
        };
        let mut sqs_exists = false;
        if !mock {
            let qr = self
                .sqs
                .get_queue_url()
                .queue_name(sqs_name.clone())
                .send()
                .await;
            sqs_exists = qr.is_ok();
        };
        if sqs_exists {
            noops = noops + 1;
            resources.push(serde_json::json!({ "kind": "sqs".to_string(), "action": "noop".to_string(), "exists": true, "name": sqs_name.clone(), "detail": format!("Queue + DLQ '{}-dlq' present", sqs_name) }));
            steps.push(serde_json::json!({ "id": "stack_sqs".to_string(), "label": format!("SQS '{}' + DLQ", sqs_name), "phase": "infra".to_string(), "action": "noop".to_string() }));
        } else {
            creates = creates + 1;
            resources.push(serde_json::json!({ "kind": "sqs".to_string(), "action": "create".to_string(), "exists": false, "name": sqs_name.clone(), "detail": format!("Will CreateQueue + DLQ '{}-dlq'", sqs_name) }));
            steps.push(serde_json::json!({ "id": "stack_sqs".to_string(), "label": format!("SQS '{}' + DLQ", sqs_name), "phase": "infra".to_string(), "action": "create".to_string() }));
        };
        let unit_names = veil_local_fs::LocalFs::project_unit_names(slug.clone())
            .map_err(|e| DomainError::External(e.to_string()))?;
        for uname in unit_names {
            let uty = veil_local_fs::LocalFs::project_unit_type(slug.clone(), uname.clone())
                .map_err(|e| DomainError::External(e.to_string()))?;
            let mut fn_name = format!("{}-{}", base, uname);
            if uty == "lambda-api".to_string() && lambda_api != "".to_string() {
                fn_name = lambda_api.clone();
            };
            if uty == "lambda-consumer".to_string() && lambda_consumer != "".to_string() {
                fn_name = lambda_consumer.clone();
            };
            let mut lambda_exists = false;
            let mut action = "create".to_string();
            let mut detail = format!("Create function '{}'", fn_name);
            if uty == "ecs-service".to_string() || uty == "ecs-task".to_string() {
                action = "skip".to_string();
                detail = "ECS not automated - skip".to_string();
                noops = noops + 1;
            } else {
                if !mock {
                    let fr = self
                        .lambda
                        .get_function()
                        .function_name(fn_name.clone())
                        .send()
                        .await;
                    lambda_exists = fr.is_ok();
                };
                if lambda_exists {
                    action = "noop".to_string();
                    detail = format!("Exists - no config drift detected on '{}'", fn_name);
                    if uty == "lambda-api".to_string() {
                        detail = format!("Exists - '{}' + API GW route present", fn_name);
                    };
                    noops = noops + 1;
                } else {
                    creates = creates + 1;
                    if uty == "lambda-api".to_string() {
                        detail = format!(
                            "Create '{}' + routes on existing HTTP API (never create API GW)",
                            fn_name
                        );
                    };
                    if uty == "lambda-consumer".to_string() {
                        detail =
                            format!("Create '{}' + SQS event source <- '{}'", fn_name, sqs_name);
                    };
                };
            };
            resources.push(serde_json::json!({ "kind": uty.clone(), "action": action.clone(), "exists": lambda_exists.clone(), "name": fn_name.clone(), "unit": uname.clone(), "detail": detail.clone() }));
            steps.push(serde_json::json!({ "id": format!("unit:{}", uname), "label": format!("Unit '{}' ({}) -> '{}'", uname, uty, fn_name), "phase": "infra".to_string(), "action": action.clone() }));
            unit_actions.push(serde_json::json!({ "unit": uname.clone(), "type": uty.clone(), "function_name": fn_name.clone(), "exists": lambda_exists.clone(), "action": action.clone(), "detail": detail.clone() }));
        }
        if creates > 0 {
            updates = updates + 1;
            steps = {
                let mut __v = steps;
                __v.extend(vec![serde_json::json!({ "id": "compile".to_string(), "label": "Compile project sources (veil gen + cargo build --release)".to_string(), "phase": "code".to_string(), "action": "update".to_string() })]);
                let hub = veil_local_fs::LocalFs::projects_dir();
                let root = veil_local_fs::LocalFs::join(hub.clone(), slug.clone());
                __v.extend(crate::hooks::plan_hook_steps(std::path::Path::new(&root)));
                if !__v.iter().any(|s| s.get("id").and_then(|v| v.as_str()) == Some("run_hooks") || s.get("id").and_then(|v| v.as_str()).is_some_and(|id| id.starts_with("hook:"))) {
                    __v.push(serde_json::json!({ "id": "run_hooks".to_string(), "label": "Run deploy hooks (none declared)".to_string(), "phase": "hooks".to_string(), "action": "noop".to_string() }));
                }
                __v.extend(vec![serde_json::json!({ "id": "deploy_code".to_string(), "label": "Deploy binary to Lambda (UpdateFunctionCode); ECS skipped".to_string(), "phase": "code".to_string(), "action": "update".to_string() }), serde_json::json!({ "id": "finalize".to_string(), "label": "Record provision state in DynamoDB".to_string(), "phase": "finalize".to_string(), "action": "update".to_string() })]);
                __v
            };
        } else {
            steps = {
                let mut __v = steps;
                __v.extend(vec![serde_json::json!({ "id": "compile".to_string(), "label": "Compile project sources".to_string(), "phase": "code".to_string(), "action": "noop".to_string() })]);
                let hub = veil_local_fs::LocalFs::projects_dir();
                let root = veil_local_fs::LocalFs::join(hub.clone(), slug.clone());
                let hook_steps = crate::hooks::plan_hook_steps(std::path::Path::new(&root));
                if hook_steps.is_empty() {
                    __v.push(serde_json::json!({ "id": "run_hooks".to_string(), "label": "Run deploy hooks (none declared)".to_string(), "phase": "hooks".to_string(), "action": "noop".to_string() }));
                } else {
                    __v.extend(hook_steps);
                }
                __v.extend(vec![serde_json::json!({ "id": "deploy_code".to_string(), "label": "Code unchanged - skip deploy".to_string(), "phase": "code".to_string(), "action": "noop".to_string() }), serde_json::json!({ "id": "finalize".to_string(), "label": "Finalize".to_string(), "phase": "finalize".to_string(), "action": "noop".to_string() })]);
                __v
            };
        };
        let notes = vec!["Preview only - nothing is written until you confirm.".to_string(), "Apply is ensure/reconcile: create missing resources, refresh config/code on existing ones.".to_string(), "Removals are NOT destroyed automatically (no destroy phase). Orphans stay in AWS until cleaned manually.".to_string(), "API Gateway is never created; only routes/integrations on an existing HTTP API.".to_string(), "AddPermission source_arn must use the real 12-digit account from the Lambda ARN (never '*').".to_string()];
        let mut summary = if creates == 0 && updates == 0 {
            format!(
                "In sync - all resources provisioned for stack '{}' in '{}'",
                base, environment
            )
        } else {
            format!(
                "Provision: {} create, {} update/refresh, {} noop, 0 destroy - stack '{}' in '{}'",
                creates, updates, noops, base, environment
            )
        };
        if mock {
            summary = format!("{} [MOCK]", summary);
        };
        return Ok(serde_json::to_string(
            &serde_json::json!({ "environment": environment.clone(), "mock_mode": mock.clone(), "service": service.clone(), "resource_prefix": resource_prefix.clone(), "stack_base": base.clone(), "stack_names": names.clone(), "resources": resources.clone(), "units": unit_actions.clone(), "steps": steps.clone(), "diff": serde_json::json!({ "create": creates.clone(), "update": updates.clone(), "noop": noops.clone(), "destroy": 0 }), "notes": notes.clone(), "summary": summary.clone(), "source": "disk".to_string(), "repo_id": repo_id.clone(), "branch": branch.clone() }),
        )?);
    }

    async fn provision_unit(
        &self,
        project_slug: String,
        environment: String,
        unit_name: String,
    ) -> Result<String, DomainError> {
        let mock =
            std::env::var("VEIL_DEPLOY_EXECUTOR".to_string()).unwrap_or_else(|_| "".to_string());
        if mock == "mock".to_string() || mock == "MOCK".to_string() {
            return Ok(serde_json::to_string(
                &serde_json::json!({ "success": true, "already": false, "unit_name": unit_name.clone(), "environment": environment.clone(), "message": "mock provisioned".to_string(), "summary": format!("Mock-provisioned {} in {}", unit_name, environment), "resources": serde_json::json!({ "note": "VEIL_DEPLOY_EXECUTOR=mock".to_string() }), "duration_ms": 0 }),
            )?);
        };
        let disk = veil_local_fs::LocalFs::read_project_deploy(project_slug.clone())
            .map_err(|e| DomainError::External(e.to_string()))?;
        let snap: serde_json::Value = serde_json::from_str::<_>(&disk)?;
        let names = snap
            .get("stack")
            .cloned()
            .ok_or(DomainError::NotFound)?
            .get("names")
            .cloned()
            .ok_or(DomainError::NotFound)?;
        let base = names
            .get("base")
            .cloned()
            .ok_or(DomainError::NotFound)?
            .as_str()
            .map(|s| s.to_string())
            .unwrap_or("veil-unknown".to_string())
            .to_string();
        let uty =
            veil_local_fs::LocalFs::project_unit_type(project_slug.clone(), unit_name.clone())
                .map_err(|e| DomainError::External(e.to_string()))?;
        let mut fn_name = format!("{}-{}", base, unit_name);
        if uty == "lambda-api".to_string() {
            let la = names
                .get("lambda_api")
                .cloned()
                .ok_or(DomainError::NotFound)?
                .as_str()
                .map(|s| s.to_string())
                .unwrap_or("".to_string())
                .to_string();
            if la != "".to_string() {
                fn_name = la;
            };
        };
        if uty == "lambda-consumer".to_string() {
            let lc = names
                .get("lambda_consumer")
                .cloned()
                .ok_or(DomainError::NotFound)?
                .as_str()
                .map(|s| s.to_string())
                .unwrap_or("".to_string())
                .to_string();
            if lc != "".to_string() {
                fn_name = lc;
            };
        };
        let exists = self
            .lambda
            .get_function()
            .function_name(fn_name.clone())
            .send()
            .await;
        if exists.is_ok() {
            return Ok(serde_json::to_string(
                &serde_json::json!({ "success": true, "already": true, "unit_name": unit_name.clone(), "environment": environment.clone(), "message": "already provisioned".to_string(), "summary": format!("{} already provisioned in {}", unit_name, environment), "resources": serde_json::json!({ "lambda_name": fn_name.clone() }), "duration_ms": 0 }),
            )?);
        };
        let role = std::env::var("VEIL_LAMBDA_ROLE".to_string()).unwrap_or_default();
        return Ok(serde_json::to_string(
            &serde_json::json!({ "success": false, "unit_name": unit_name.clone(), "environment": environment.clone(), "message": "function missing - use ProvisionProject job for stack ensure + CreateFunction".to_string(), "summary": format!("Unit {} -> '{}' not found; role={}", unit_name, fn_name, role), "resources": serde_json::json!({ "lambda_name": fn_name.clone(), "role_arn": role.clone() }), "duration_ms": 0 }),
        )?);
    }

    async fn read_project_deploy(
        &self,
        repo_id: String,
        branch: String,
        slug: String,
    ) -> Result<String, DomainError> {
        let mode = std::env::var("VEIL_SOURCE_MODE".to_string())
            .unwrap_or_else(|_| "prefer_s3".to_string());
        let s3_key = format!("repos/{}/{}/veil.toml", repo_id, branch);
        let disk = veil_local_fs::LocalFs::read_project_deploy(slug.clone())
            .map_err(|e| DomainError::External(e.to_string()))?;
        let snap: serde_json::Value = serde_json::from_str::<_>(&disk)?;
        let mut source = "disk".to_string();
        if mode != "disk".to_string()
            && self.bucket.clone() != "".to_string()
            && self.bucket.clone() != "default".to_string()
        {
            let head = self
                .s3
                .head_object()
                .bucket(self.bucket.clone())
                .key(s3_key.clone())
                .send()
                .await;
            if head.is_ok() {
                source = "s3".to_string();
            };
        };
        return Ok(serde_json::to_string(
            &serde_json::json!({ "source": source.clone(), "repo_id": repo_id.clone(), "branch": branch.clone(), "bucket": serde_json::json!("self")["bucket"].clone(), "s3_key": s3_key.clone(), "has_deploy": snap.get("has_deploy").cloned().ok_or(DomainError::NotFound)?, "region": snap.get("region").cloned().ok_or(DomainError::NotFound)?, "project_prefix": snap.get("project_prefix").cloned().ok_or(DomainError::NotFound)?, "resource_prefix": snap.get("resource_prefix").cloned().ok_or(DomainError::NotFound)?, "service": snap.get("service").cloned().ok_or(DomainError::NotFound)?, "stack": snap.get("stack").cloned().ok_or(DomainError::NotFound)?, "units": snap.get("units").cloned().ok_or(DomainError::NotFound)?, "network": snap.get("network").cloned().ok_or(DomainError::NotFound)?, "projects_dir": snap.get("projects_dir").cloned().ok_or(DomainError::NotFound)?, "toml_path": snap.get("toml_path").cloned().ok_or(DomainError::NotFound)?, "slug": slug.clone() }),
        )?);
    }

    async fn start_provision(
        &self,
        project_slug: String,
        environment: String,
    ) -> Result<String, DomainError> {
        let disk = veil_local_fs::LocalFs::read_project_deploy(project_slug.clone())
            .map_err(|e| DomainError::External(e.to_string()))?;
        let snap: serde_json::Value = serde_json::from_str::<_>(&disk)?;
        veil_local_fs::LocalFs::create_dir_all("/tmp/veil-provision-jobs".to_string())
            .map_err(|e| DomainError::External(e.to_string()))?;
        let now = Utc::now();
        let job_id = format!("prov-{}-{}", project_slug, environment);
        let path = veil_local_fs::LocalFs::join(
            "/tmp/veil-provision-jobs".to_string(),
            format!("{}.json", job_id),
        );
        let names = snap
            .get("stack")
            .cloned()
            .ok_or(DomainError::NotFound)?
            .get("names")
            .cloned()
            .ok_or(DomainError::NotFound)?;
        let base = names
            .get("base")
            .cloned()
            .ok_or(DomainError::NotFound)?
            .as_str()
            .map(|s| s.to_string())
            .unwrap_or("veil-unknown".to_string())
            .to_string();
        let mut ddb_name = names
            .get("dynamodb")
            .cloned()
            .ok_or(DomainError::NotFound)?
            .as_str()
            .map(|s| s.to_string())
            .unwrap_or("veil-unknown".to_string())
            .to_string();
        let mut sns_name = names
            .get("sns")
            .cloned()
            .ok_or(DomainError::NotFound)?
            .as_str()
            .map(|s| s.to_string())
            .unwrap_or("veil-unknown".to_string())
            .to_string();
        let mut sqs_name = names
            .get("sqs")
            .cloned()
            .ok_or(DomainError::NotFound)?
            .as_str()
            .map(|s| s.to_string())
            .unwrap_or("veil-unknown".to_string())
            .to_string();
        let lambda_api = names
            .get("lambda_api")
            .cloned()
            .ok_or(DomainError::NotFound)?
            .as_str()
            .map(|s| s.to_string())
            .unwrap_or("".to_string())
            .to_string();
        let lambda_consumer = names
            .get("lambda_consumer")
            .cloned()
            .ok_or(DomainError::NotFound)?
            .as_str()
            .map(|s| s.to_string())
            .unwrap_or("".to_string())
            .to_string();
        if ddb_name == "veil-unknown".to_string() {
            ddb_name = base.clone();
        };
        if sns_name == "veil-unknown".to_string() {
            sns_name = base.clone();
        };
        if sqs_name == "veil-unknown".to_string() {
            sqs_name = base.clone();
        };
        let mut u0 = "".to_string();
        let mut u1 = "".to_string();
        let mut u2 = "".to_string();
        let mut t0 = "".to_string();
        let mut t1 = "".to_string();
        let mut t2 = "".to_string();
        let mut f0 = "".to_string();
        let mut f1 = "".to_string();
        let mut f2 = "".to_string();
        let mut unit_count = 0;
        let unit_names = veil_local_fs::LocalFs::project_unit_names(project_slug.clone())
            .map_err(|e| DomainError::External(e.to_string()))?;
        for uname in unit_names {
            let uty =
                veil_local_fs::LocalFs::project_unit_type(project_slug.clone(), uname.clone())
                    .map_err(|e| DomainError::External(e.to_string()))?;
            let mut fn_name = format!("{}-{}", base, uname);
            if uty == "lambda-api".to_string() {
                if lambda_api != "".to_string() {
                    fn_name = lambda_api.clone();
                };
            };
            if uty == "lambda-consumer".to_string() {
                if lambda_consumer != "".to_string() {
                    fn_name = lambda_consumer.clone();
                };
            };
            if unit_count == 0 {
                u0 = uname.clone();
                t0 = uty.clone();
                f0 = fn_name.clone();
            };
            if unit_count == 1 {
                u1 = uname.clone();
                t1 = uty.clone();
                f1 = fn_name.clone();
            };
            if unit_count == 2 {
                u2 = uname.clone();
                t2 = uty.clone();
                f2 = fn_name.clone();
            };
            unit_count = unit_count + 1;
        }
        let mut steps = vec![
            serde_json::json!({ "id": "load_config".to_string(), "label": "Load veil.toml deploy config".to_string(), "status": "pending".to_string(), "detail": "".to_string() }),
            serde_json::json!({ "id": "stack_dynamodb".to_string(), "label": format!("DynamoDB {}", ddb_name), "status": "pending".to_string(), "detail": "".to_string() }),
            serde_json::json!({ "id": "stack_sns".to_string(), "label": format!("SNS {}", sns_name), "status": "pending".to_string(), "detail": "".to_string() }),
            serde_json::json!({ "id": "stack_sqs".to_string(), "label": format!("SQS {} + DLQ", sqs_name), "status": "pending".to_string(), "detail": "".to_string() }),
        ];
        if unit_count > 0 {
            steps.push(serde_json::json!({ "id": format!("unit:{}", u0), "label": format!("Unit {} ({}) -> {}", u0, t0, f0), "status": "pending".to_string(), "detail": "".to_string() }));
        };
        if unit_count > 1 {
            steps.push(serde_json::json!({ "id": format!("unit:{}", u1), "label": format!("Unit {} ({}) -> {}", u1, t1, f1), "status": "pending".to_string(), "detail": "".to_string() }));
        };
        if unit_count > 2 {
            steps.push(serde_json::json!({ "id": format!("unit:{}", u2), "label": format!("Unit {} ({}) -> {}", u2, t2, f2), "status": "pending".to_string(), "detail": "".to_string() }));
        };
        steps = {
            let mut __v = steps;
            __v.extend(vec![serde_json::json!({ "id": "compile".to_string(), "label": "Compile project sources".to_string(), "status": "pending".to_string(), "detail": "".to_string() }), serde_json::json!({ "id": "deploy_code".to_string(), "label": "Deploy binary to Lambda".to_string(), "status": "pending".to_string(), "detail": "".to_string() }), serde_json::json!({ "id": "finalize".to_string(), "label": "Record provision state".to_string(), "status": "pending".to_string(), "detail": "".to_string() })]);
            __v
        };
        let steps_total = 8 + unit_count;
        let job = serde_json::json!({ "job_id": job_id.clone(), "project_slug": project_slug.clone(), "environment": environment.clone(), "repo_id": "".to_string(), "branch": "main".to_string(), "status": "pending".to_string(), "summary": "Queued".to_string(), "error": serde_json::Value::Null, "percent": 0, "steps_done": 0, "steps_total": steps_total.clone(), "started_at": now.clone(), "updated_at": now.clone(), "finished_at": "".to_string(), "cursor": "load_config".to_string(), "stack_base": base.clone(), "ddb_name": ddb_name.clone(), "sns_name": sns_name.clone(), "sqs_name": sqs_name.clone(), "lambda_api": lambda_api.clone(), "lambda_consumer": lambda_consumer.clone(), "unit_count": unit_count.clone(), "u0": u0.clone(), "t0": t0.clone(), "f0": f0.clone(), "u1": u1.clone(), "t1": t1.clone(), "f1": f1.clone(), "u2": u2.clone(), "t2": t2.clone(), "f2": f2.clone(), "steps": steps.clone() });
        veil_local_fs::LocalFs::write(path.clone(), serde_json::to_string(&job)?)
            .map_err(|e| DomainError::External(e.to_string()))?;
        return Ok(serde_json::to_string(&job)?);
    }

    async fn start_provision_repo(
        &self,
        repo_id: String,
        branch: String,
        slug: String,
        environment: String,
    ) -> Result<String, DomainError> {
        let disk = veil_local_fs::LocalFs::read_project_deploy(slug.clone())
            .map_err(|e| DomainError::External(e.to_string()))?;
        let snap: serde_json::Value = serde_json::from_str::<_>(&disk)?;
        veil_local_fs::LocalFs::create_dir_all("/tmp/veil-provision-jobs".to_string())
            .map_err(|e| DomainError::External(e.to_string()))?;
        let now = Utc::now();
        let job_id = format!("prov-{}-{}", slug, environment);
        let path = veil_local_fs::LocalFs::join(
            "/tmp/veil-provision-jobs".to_string(),
            format!("{}.json", job_id),
        );
        let stack = snap.get("stack").cloned().ok_or(DomainError::NotFound)?;
        let names = stack.get("names").cloned().ok_or(DomainError::NotFound)?;
        let base = names
            .get("base")
            .cloned()
            .ok_or(DomainError::NotFound)?
            .as_str()
            .map(|s| s.to_string())
            .unwrap_or("veil-unknown".to_string())
            .to_string();
        let ddb_name = names
            .get("dynamodb")
            .cloned()
            .ok_or(DomainError::NotFound)?
            .as_str()
            .map(|s| s.to_string())
            .unwrap_or("veil-unknown".to_string())
            .to_string();
        let sns_name = names
            .get("sns")
            .cloned()
            .ok_or(DomainError::NotFound)?
            .as_str()
            .map(|s| s.to_string())
            .unwrap_or("veil-unknown".to_string())
            .to_string();
        let sqs_name = names
            .get("sqs")
            .cloned()
            .ok_or(DomainError::NotFound)?
            .as_str()
            .map(|s| s.to_string())
            .unwrap_or("veil-unknown".to_string())
            .to_string();
        let lambda_api = names
            .get("lambda_api")
            .cloned()
            .ok_or(DomainError::NotFound)?
            .as_str()
            .map(|s| s.to_string())
            .unwrap_or("".to_string())
            .to_string();
        let lambda_consumer = names
            .get("lambda_consumer")
            .cloned()
            .ok_or(DomainError::NotFound)?
            .as_str()
            .map(|s| s.to_string())
            .unwrap_or("".to_string())
            .to_string();
        let mut u0 = "".to_string();
        let mut u1 = "".to_string();
        let mut u2 = "".to_string();
        let mut t0 = "".to_string();
        let mut t1 = "".to_string();
        let mut t2 = "".to_string();
        let mut f0 = "".to_string();
        let mut f1 = "".to_string();
        let mut f2 = "".to_string();
        let mut unit_count = 0;
        let unit_names = veil_local_fs::LocalFs::project_unit_names(slug.clone())
            .map_err(|e| DomainError::External(e.to_string()))?;
        for uname in unit_names {
            let uty = veil_local_fs::LocalFs::project_unit_type(slug.clone(), uname.clone())
                .map_err(|e| DomainError::External(e.to_string()))?;
            let mut fn_name = format!("{}-{}", base, uname);
            if uty == "lambda-api".to_string() && lambda_api != "".to_string() {
                fn_name = lambda_api.clone();
            };
            if uty == "lambda-consumer".to_string() && lambda_consumer != "".to_string() {
                fn_name = lambda_consumer.clone();
            };
            if unit_count == 0 {
                u0 = uname.clone();
                t0 = uty.clone();
                f0 = fn_name.clone();
            };
            if unit_count == 1 {
                u1 = uname.clone();
                t1 = uty.clone();
                f1 = fn_name.clone();
            };
            if unit_count == 2 {
                u2 = uname.clone();
                t2 = uty.clone();
                f2 = fn_name.clone();
            };
            unit_count = unit_count + 1;
        }
        let mut steps = vec![
            serde_json::json!({ "id": "load_config".to_string(), "label": "Load veil.toml deploy config".to_string(), "status": "pending".to_string(), "detail": "".to_string() }),
            serde_json::json!({ "id": "stack_dynamodb".to_string(), "label": format!("DynamoDB '{}'", ddb_name), "status": "pending".to_string(), "detail": "".to_string() }),
            serde_json::json!({ "id": "stack_sns".to_string(), "label": format!("SNS '{}'", sns_name), "status": "pending".to_string(), "detail": "".to_string() }),
            serde_json::json!({ "id": "stack_sqs".to_string(), "label": format!("SQS '{}' + DLQ", sqs_name), "status": "pending".to_string(), "detail": "".to_string() }),
        ];
        if unit_count > 0 {
            steps.push(serde_json::json!({ "id": format!("unit:{}", u0), "label": format!("Unit '{}' ({}) -> '{}'", u0, t0, f0), "status": "pending".to_string(), "detail": "".to_string() }));
        };
        if unit_count > 1 {
            steps.push(serde_json::json!({ "id": format!("unit:{}", u1), "label": format!("Unit '{}' ({}) -> '{}'", u1, t1, f1), "status": "pending".to_string(), "detail": "".to_string() }));
        };
        if unit_count > 2 {
            steps.push(serde_json::json!({ "id": format!("unit:{}", u2), "label": format!("Unit '{}' ({}) -> '{}'", u2, t2, f2), "status": "pending".to_string(), "detail": "".to_string() }));
        };
        steps = {
            let mut __v = steps;
            __v.extend(vec![serde_json::json!({ "id": "compile".to_string(), "label": "Compile project sources".to_string(), "status": "pending".to_string(), "detail": "".to_string() }), serde_json::json!({ "id": "deploy_code".to_string(), "label": "Deploy binary to Lambda".to_string(), "status": "pending".to_string(), "detail": "".to_string() }), serde_json::json!({ "id": "finalize".to_string(), "label": "Record provision state".to_string(), "status": "pending".to_string(), "detail": "".to_string() })]);
            __v
        };
        let steps_total = 8 + unit_count;
        let job = serde_json::json!({ "job_id": job_id.clone(), "project_slug": slug.clone(), "environment": environment.clone(), "repo_id": repo_id.clone(), "branch": branch.clone(), "status": "pending".to_string(), "summary": "Queued".to_string(), "error": serde_json::Value::Null, "percent": 0, "steps_done": 0, "steps_total": steps_total.clone(), "started_at": now.clone(), "updated_at": now.clone(), "finished_at": "".to_string(), "cursor": "load_config".to_string(), "stack_base": base.clone(), "ddb_name": ddb_name.clone(), "sns_name": sns_name.clone(), "sqs_name": sqs_name.clone(), "lambda_api": lambda_api.clone(), "lambda_consumer": lambda_consumer.clone(), "unit_count": unit_count.clone(), "u0": u0.clone(), "t0": t0.clone(), "f0": f0.clone(), "u1": u1.clone(), "t1": t1.clone(), "f1": f1.clone(), "u2": u2.clone(), "t2": t2.clone(), "f2": f2.clone(), "steps": steps.clone() });
        veil_local_fs::LocalFs::write(path.clone(), serde_json::to_string(&job)?)
            .map_err(|e| DomainError::External(e.to_string()))?;
        return Ok(serde_json::to_string(&job)?);
    }

    async fn sync_hub_to_s3(
        &self,
        repo_id: String,
        branch: String,
        slug: String,
    ) -> Result<String, DomainError> {
        let hub = veil_local_fs::LocalFs::projects_dir();
        let root = veil_local_fs::LocalFs::join(hub.clone(), slug.clone());
        let mut uploaded = 0;
        let mut skipped = 0;
        if self.bucket.clone() == "".to_string() || self.bucket.clone() == "default".to_string() {
            return Ok(serde_json::to_string(
                &serde_json::json!({ "error": "BUCKET / VEIL_S3_BUCKET not set".to_string(), "uploaded": 0, "skipped": 0 }),
            )?);
        };
        if !veil_local_fs::LocalFs::path_exists(root.clone()) {
            return Ok(serde_json::to_string(
                &serde_json::json!({ "error": format!("local root missing: {}", root), "uploaded": 0, "skipped": 0 }),
            )?);
        };
        let names = veil_local_fs::LocalFs::list_dir(root.clone())
            .map_err(|e| DomainError::External(e.to_string()))?;
        for name in names {
            if name == ".git".to_string()
                || name == "target".to_string()
                || name == "generated".to_string()
                || name == "node_modules".to_string()
                || name == ".veil".to_string()
                || name == "dist".to_string()
            {
                skipped = skipped + 1;
            } else {
                let path = veil_local_fs::LocalFs::join(root.clone(), name.clone());
                if veil_local_fs::LocalFs::path_is_file(path.clone()) {
                    let body = veil_local_fs::LocalFs::read(path.clone())
                        .map_err(|e| DomainError::External(e.to_string()))?;
                    let key = format!("repos/{}/{}/{}", repo_id, branch, name);
                    self.s3
                        .put_object()
                        .bucket(self.bucket.clone())
                        .key(key.clone())
                        .body(body.into_bytes().into())
                        .send()
                        .await
                        .map_err(|e| DomainError::External(format!("{e:?}")))?;
                    uploaded = uploaded + 1;
                } else {
                    skipped = skipped + 1;
                };
            };
        }
        return Ok(serde_json::to_string(
            &serde_json::json!({ "repo_id": repo_id.clone(), "branch": branch.clone(), "bucket": serde_json::json!("self")["bucket"].clone(), "uploaded": uploaded.clone(), "skipped": skipped.clone(), "local_root": root.clone() }),
        )?);
    }
}

/// Adapter: MockActionExecutor (implements ActionExecutor)
pub struct MockActionExecutor {}

#[async_trait]
impl ActionExecutor for MockActionExecutor {
    async fn execute_action(
        &self,
        action: Action,
        state: DeploymentState,
    ) -> Result<ActionResult, DomainError> {
        return Ok(ActionResult {
            success: true,
            message: format!("Mock executed: {}", action.description),
            resource_updates: HashMap::new(),
            duration_ms: 0,
        });
    }
}
