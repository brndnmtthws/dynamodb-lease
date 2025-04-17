pub mod retry;

use aws_config::BehaviorVersion;
use aws_sdk_dynamodb::{
    error::{ProvideErrorMetadata, SdkError},
    operation::create_table::CreateTableError,
    types::{
        AttributeDefinition, BillingMode, KeySchemaElement, KeyType, ScalarAttributeType,
        TimeToLiveSpecification,
    },
};
use std::time::Duration;
use testcontainers_modules::testcontainers::{core::IntoContainerPort, ContainerAsync};
use testcontainers_modules::{dynamodb_local::DynamoDb, testcontainers::runners::AsyncRunner};
use tokio::sync::OnceCell;

/// Test wait timeout, generally long enough that something has probably gone wrong.
pub const TEST_WAIT: Duration = Duration::from_secs(4);

// Use OnceCell for efficiency: start the container & create client once for all tests.
// The container will be stopped automatically when the test process exits.
static DYNAMODB_INSTANCE: OnceCell<ContainerAsync<DynamoDb>> = OnceCell::const_new();

/// Gets the test dynamodb client and container handle, initializing it on first call.
pub async fn get_test_db() -> (aws_sdk_dynamodb::Client, &'static ContainerAsync<DynamoDb>) {
    let instance = DYNAMODB_INSTANCE
        .get_or_init(|| async {
            DynamoDb::default()
                .start()
                .await
                .expect("failed to start dynamodb local container")
        })
        .await;

    let host = instance.get_host().await.expect("failed to get host");
    let host_port = instance
        .get_host_port_ipv4(8000.tcp())
        .await
        .expect("failed to get host port");

    let conf = aws_config::defaults(BehaviorVersion::latest())
        .region("us-east-1")
        .load()
        .await;
    let conf = aws_sdk_dynamodb::config::Builder::from(&conf)
        .endpoint_url(format!("http://{}:{}", host, host_port))
        .build();
    let client = aws_sdk_dynamodb::Client::from_conf(conf);

    (client, instance)
}

/// Create the table, with "key" as a hash key, if it doesn't exist.
pub async fn create_lease_table(table_name: &str, client: &aws_sdk_dynamodb::Client) {
    let create_table = client
        .create_table()
        .table_name(table_name)
        .billing_mode(BillingMode::PayPerRequest)
        .attribute_definitions(
            AttributeDefinition::builder()
                .attribute_name("key")
                .attribute_type(ScalarAttributeType::S)
                .build()
                .unwrap(),
        )
        .key_schema(
            KeySchemaElement::builder()
                .attribute_name("key")
                .key_type(KeyType::Hash)
                .build()
                .unwrap(),
        )
        .send()
        .await;

    match create_table {
        Ok(_) => Ok(()),
        Err(SdkError::ServiceError(se))
            if matches!(se.err(), CreateTableError::ResourceInUseException(..)) =>
        {
            Ok(())
        }
        Err(e) => Err(e),
    }
    .expect("dynamodb create_table failed");

    let ttl_update = client
        .update_time_to_live()
        .table_name(table_name)
        .time_to_live_specification(
            TimeToLiveSpecification::builder()
                .enabled(true)
                .attribute_name("lease_expiry")
                .build()
                .unwrap(),
        )
        .send()
        .await;
    match ttl_update {
        Ok(_) => Ok(()),
        Err(SdkError::ServiceError(se))
            if se.err().code() == Some("ValidationException")
                && se.err().message() == Some("TimeToLive is already enabled") =>
        {
            Ok(())
        }

        Err(e) => Err(e),
    }
    .expect("dynamodb ttl_update failed");
}
