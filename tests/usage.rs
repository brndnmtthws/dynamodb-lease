mod util;

use anyhow::Context;
use aws_sdk_dynamodb::types::{
    AttributeDefinition, BillingMode, KeySchemaElement, KeyType, ScalarAttributeType,
};
use std::time::Duration;
use time;
use util::*;
use uuid::Uuid;

#[tokio::test]
async fn try_acquire() {
    let lease_table = "test-locker-leases";
    let (db_client, instance) = get_test_db().await;
    create_lease_table(lease_table, &db_client).await;

    let client = dynamodb_lease::Client::builder()
        .table_name(lease_table)
        .build_and_check_db(db_client.clone())
        .await
        .unwrap();
    // use 2 clients to avoid local locking / simulate distributed usage
    let client2 = dynamodb_lease::Client::builder()
        .table_name(lease_table)
        .build_and_check_db(db_client.clone())
        .await
        .unwrap();

    let lease_key = format!("try_acquire:{}", Uuid::new_v4());

    // acquiring a lease should work
    let lease1 = client.try_acquire(&lease_key).await.unwrap();
    assert!(lease1.is_some());

    // subsequent attempts should fail
    let lease2 = client2.try_acquire(&lease_key).await.unwrap();
    assert!(lease2.is_none());
    let lease2 = client2.try_acquire(&lease_key).await.unwrap();
    assert!(lease2.is_none());

    // dropping should asynchronously end the lease
    drop(lease1);

    // in shortish order the key should be acquirable again
    retry::until_ok(|| async {
        client2
            .try_acquire(&lease_key)
            .await
            .and_then(|maybe_lease| maybe_lease.context("did not acquire"))
    })
    .await;
    let _ = instance.stop().await;
}

#[tokio::test]
async fn local_try_acquire() {
    let lease_table = "test-locker-leases";
    let (db_client, instance) = get_test_db().await;
    create_lease_table(lease_table, &db_client).await;

    let client = dynamodb_lease::Client::builder()
        .table_name(lease_table)
        .build_and_check_db(db_client)
        .await
        .unwrap();

    let lease_key = format!("try_acquire:{}", Uuid::new_v4());

    // acquiring a lease should work
    let lease1 = client.try_acquire(&lease_key).await.unwrap();
    assert!(lease1.is_some());

    // subsequent attempts should fail
    let lease2 = client.try_acquire(&lease_key).await.unwrap();
    assert!(lease2.is_none());
    let lease2 = client.try_acquire(&lease_key).await.unwrap();
    assert!(lease2.is_none());

    // dropping should asynchronously end the lease
    drop(lease1);

    // in shortish order the key should be acquirable again
    retry::until_ok(|| async {
        client
            .try_acquire(&lease_key)
            .await
            .and_then(|maybe_lease| maybe_lease.context("did not acquire"))
    })
    .await;
    let _ = instance.stop().await;
}

#[tokio::test]
#[ignore = "slow"]
async fn try_acquire_extend_past_ttl() {
    let lease_table = "test-locker-leases";
    let (db_client, instance) = get_test_db().await;
    create_lease_table(lease_table, &db_client).await;

    let client = dynamodb_lease::Client::builder()
        .table_name(lease_table)
        .lease_ttl_seconds(2)
        .build_and_check_db(db_client.clone())
        .await
        .unwrap();
    // use 2 clients to avoid local locking / simulate distributed usage
    let client2 = dynamodb_lease::Client::builder()
        .table_name(lease_table)
        .lease_ttl_seconds(2)
        .build_and_check_db(db_client)
        .await
        .unwrap();

    let lease_key = format!("try_acquire_extend_past_expiry:{}", Uuid::new_v4());

    // acquiring a lease should work
    let lease1 = client.try_acquire(&lease_key).await.unwrap();
    assert!(lease1.is_some());

    // subsequent attempts should fail
    assert!(client2.try_acquire(&lease_key).await.unwrap().is_none());

    // after some time the original lease will have expired
    // however, a background task should have extended it so it should still be active.
    // Note: Need to wait ages to reliably trigger ttl deletion :(
    tokio::time::sleep(Duration::from_secs(10)).await;
    assert!(
        client2.try_acquire(&lease_key).await.unwrap().is_none(),
        "lease should have been extended"
    );
    let _ = instance.stop().await;
}

#[tokio::test]
async fn acquire() {
    let lease_table = "test-locker-leases";
    let (db_client, instance) = get_test_db().await;
    create_lease_table(lease_table, &db_client).await;

    let client = dynamodb_lease::Client::builder()
        .table_name(lease_table)
        .build_and_check_db(db_client.clone())
        .await
        .unwrap();
    // use 2 clients to avoid local locking / simulate distributed usage
    let client2 = dynamodb_lease::Client::builder()
        .table_name(lease_table)
        .build_and_check_db(db_client)
        .await
        .unwrap();

    let lease_key = format!("acquire:{}", Uuid::new_v4());

    // acquiring a lease should work
    let lease1 = client.acquire(&lease_key).await.unwrap();

    // subsequent attempts should fail
    let lease2 =
        tokio::time::timeout(Duration::from_millis(100), client2.acquire(&lease_key)).await;
    assert!(lease2.is_err(), "should not acquire while lease1 is alive");

    // dropping should asynchronously end the lease
    drop(lease1);

    // in shortish order the key should be acquirable again
    tokio::time::timeout(TEST_WAIT, client2.acquire(&lease_key))
        .await
        .expect("could not acquire after drop")
        .expect("failed to acquire");
    let _ = instance.stop().await;
}

#[tokio::test]
async fn release_try_acquire() {
    let lease_table = "test-locker-leases";
    let (db_client, instance) = get_test_db().await;
    create_lease_table(lease_table, &db_client).await;

    let client = dynamodb_lease::Client::builder()
        .table_name(lease_table)
        .build_and_check_db(db_client.clone())
        .await
        .unwrap();
    // use 2 clients to avoid local locking / simulate distributed usage
    let client2 = dynamodb_lease::Client::builder()
        .table_name(lease_table)
        .build_and_check_db(db_client.clone())
        .await
        .unwrap();

    let lease_key = format!("acquire:{}", Uuid::new_v4());

    // acquiring a lease should work
    let lease1 = client.acquire(&lease_key).await.unwrap();

    // subsequent attempts should fail
    assert!(
        client2.try_acquire(&lease_key).await.unwrap().is_none(),
        "should not be able to acquire while lease1 is alive"
    );

    // Release the lease and await deletion
    lease1.release().await.unwrap();

    // Verify the item is actually deleted from dynamodb
    let get_item_output = db_client
        .get_item()
        .table_name(lease_table)
        .key(
            "key",
            aws_sdk_dynamodb::types::AttributeValue::S(lease_key.clone()),
        )
        .send()
        .await
        .expect("GetItem failed after release");

    assert!(
        get_item_output.item.is_none(),
        "Item should have been deleted from DynamoDB after release"
    );

    // now another client can immediately acquire
    client2
        .try_acquire(lease_key)
        .await
        .unwrap()
        .expect("failed to acquire after release");

    let _ = instance.stop().await;
}

#[tokio::test]
async fn local_acquire() {
    let lease_table = "test-locker-leases";
    let (db_client, instance) = get_test_db().await;
    create_lease_table(lease_table, &db_client).await;

    let client = dynamodb_lease::Client::builder()
        .table_name(lease_table)
        .build_and_check_db(db_client)
        .await
        .unwrap();

    let lease_key = format!("acquire:{}", Uuid::new_v4());

    // acquiring a lease should work
    let lease1 = client.acquire(&lease_key).await.unwrap();

    // subsequent attempts should fail
    let lease2 = tokio::time::timeout(Duration::from_millis(100), client.acquire(&lease_key)).await;
    assert!(lease2.is_err(), "should not acquire while lease1 is alive");

    // dropping should asynchronously end the lease
    drop(lease1);

    // in shortish order the key should be acquirable again
    tokio::time::timeout(TEST_WAIT, client.acquire(&lease_key))
        .await
        .expect("could not acquire after drop")
        .expect("failed to acquire");
    let _ = instance.stop().await;
}

#[tokio::test]
async fn acquire_timeout() {
    let lease_table = "test-locker-leases";
    let (db_client, instance) = get_test_db().await;
    create_lease_table(lease_table, &db_client).await;

    let client = dynamodb_lease::Client::builder()
        .table_name(lease_table)
        .build_and_check_db(db_client.clone())
        .await
        .unwrap();
    // use 2 clients to avoid local locking / simulate distributed usage
    let client2 = dynamodb_lease::Client::builder()
        .table_name(lease_table)
        .build_and_check_db(db_client)
        .await
        .unwrap();

    let lease_key = format!("acquire:{}", Uuid::new_v4());

    // acquiring a lease should work
    let lease1 = client
        .acquire_timeout(&lease_key, Duration::from_millis(100))
        .await
        .unwrap();

    // subsequent attempts should fail
    let lease2 = client2
        .acquire_timeout(&lease_key, Duration::from_millis(100))
        .await;
    assert!(lease2.is_err(), "should not acquire while lease1 is alive");

    // dropping should asynchronously end the lease
    drop(lease1);

    // in shortish order the key should be acquirable again
    client2
        .acquire_timeout(&lease_key, TEST_WAIT)
        .await
        .expect("failed to acquire");
    let _ = instance.stop().await;
}

#[tokio::test]
async fn init_should_check_table_exists() {
    let (db_client, instance) = get_test_db().await;

    let err = dynamodb_lease::Client::builder()
        .table_name("test-locker-leases-not-exists")
        .build_and_check_db(db_client)
        .await
        .expect_err("should check table exists");
    assert!(
        err.to_string().to_ascii_lowercase().contains("missing"),
        "{}",
        err
    );
    let _ = instance.stop().await;
}

#[tokio::test]
async fn init_should_check_hash_key() {
    let table_name = "table-with-wrong-key";
    let (db_client, instance) = get_test_db().await;

    let _ = db_client
        .create_table()
        .table_name(table_name)
        .billing_mode(BillingMode::PayPerRequest)
        .attribute_definitions(
            AttributeDefinition::builder()
                .attribute_name("wrong")
                .attribute_type(ScalarAttributeType::S)
                .build()
                .unwrap(),
        )
        .key_schema(
            KeySchemaElement::builder()
                .attribute_name("wrong")
                .key_type(KeyType::Hash)
                .build()
                .unwrap(),
        )
        .send()
        .await;

    let err = dynamodb_lease::Client::builder()
        .table_name(table_name)
        .build_and_check_db(db_client)
        .await
        .expect_err("should check hash 'key'");
    assert!(
        err.to_string().to_ascii_lowercase().contains("key"),
        "{}",
        err
    );
    let _ = instance.stop().await;
}

#[tokio::test]
async fn init_should_check_hash_key_type() {
    let table_name = "table-with-wrong-key-type";
    let (db_client, instance) = get_test_db().await;

    let _ = db_client
        .create_table()
        .table_name(table_name)
        .billing_mode(BillingMode::PayPerRequest)
        .attribute_definitions(
            AttributeDefinition::builder()
                .attribute_name("key")
                .attribute_type(ScalarAttributeType::N)
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

    let err = dynamodb_lease::Client::builder()
        .table_name(table_name)
        .build_and_check_db(db_client)
        .await
        .expect_err("should check hash key type");
    assert!(
        err.to_string().to_ascii_lowercase().contains("type"),
        "{}",
        err
    );
    let _ = instance.stop().await;
}

#[tokio::test]
async fn init_should_check_ttl() {
    let table_name = "table-with-without-ttl";
    let (db_client, instance) = get_test_db().await;

    let _ = db_client
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

    let err = dynamodb_lease::Client::builder()
        .table_name(table_name)
        .build_and_check_db(db_client)
        .await
        .expect_err("should check ttl");
    assert!(
        err.to_string()
            .to_ascii_lowercase()
            .contains("time to live"),
        "{}",
        err
    );
    let _ = instance.stop().await;
}

#[tokio::test]
async fn try_acquire_replaces_expired() {
    let lease_table = "test-locker-leases";
    let (db_client, instance) = get_test_db().await;
    create_lease_table(lease_table, &db_client).await;

    let client = dynamodb_lease::Client::builder()
        .table_name(lease_table)
        .build_and_check_db(db_client.clone())
        .await
        .unwrap();

    let lease_key = format!("try_acquire_replaces_expired:{}", Uuid::new_v4());
    let expired_ts = time::OffsetDateTime::now_utc().unix_timestamp() - 1000;
    let old_lease_v = Uuid::new_v4();

    // Manually insert an expired lease item
    db_client
        .put_item()
        .table_name(lease_table)
        .item(
            "key",
            aws_sdk_dynamodb::types::AttributeValue::S(lease_key.clone()),
        )
        .item(
            "lease_expiry",
            aws_sdk_dynamodb::types::AttributeValue::N(expired_ts.to_string()),
        )
        .item(
            "lease_version",
            aws_sdk_dynamodb::types::AttributeValue::S(old_lease_v.to_string()),
        )
        .send()
        .await
        .expect("Failed to insert expired lease item");

    // Try to acquire the lease - it should succeed by replacing the expired one
    let lease = client.try_acquire(&lease_key).await.unwrap();
    assert!(
        lease.is_some(),
        "Should have acquired the lease by replacing the expired item"
    );

    // Optionally: Verify the lease version changed
    if let Some(acquired_lease) = lease {
        assert_ne!(
            acquired_lease.lease_v().await,
            old_lease_v,
            "Lease version should have been updated"
        );
    }

    let _ = instance.stop().await;
}
