//! The Device Name: what a Device calls itself. It defaults to the hostname and is kept across
//! restarts; others learn it when they connect (see `contacts.rs`).

mod support;

use bhayanakshare_core::{Error, MAX_NAME_CHARS};
use support::TestDevice;

#[tokio::test]
async fn a_new_device_is_named_after_its_hostname_and_can_be_renamed() {
    let mut alice = TestDevice::start("alice").await;
    let default = alice.device.device_name().await;
    assert!(!default.is_empty() && default.chars().count() <= MAX_NAME_CHARS, "{default:?}");

    let stored = alice.device.set_device_name("  Alice's desktop \n").await.unwrap();
    assert_eq!(stored, "Alice's desktop");
    assert_eq!(alice.device.device_name().await, "Alice's desktop");

    let long = alice.device.set_device_name(&"x".repeat(500)).await.unwrap();
    assert_eq!(long.chars().count(), MAX_NAME_CHARS);
    assert_eq!(alice.device.device_name().await, long);

    assert!(matches!(alice.device.set_device_name(" \t ").await, Err(Error::EmptyDeviceName)));
    assert_eq!(alice.device.device_name().await, long, "a refused name changes nothing");
    alice.shutdown().await;
}
