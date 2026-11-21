//! The devices that are remembered, tested on a real connection.
//!
//! Nothing here touches a screen, a camera or a microphone.  The
//! connection is a real one over the loopback: two ends, a handshake,
//! and then the value that lets a machine in without a code.

use skycremote_core::crypto;
use skycremote_core::handshake::{
    controller_handshake, host_handshake, Credential, HostIdentity, TrustedMachine,
};
use skycremote_core::transport::{accept, connect, listen, open, Side};
use std::time::Duration;

/// The identity of the machine that is being controlled.
fn identity() -> HostIdentity {
    let salt = b"the salt of this device".to_vec();
    let verifier = crypto::code_verifier(&salt, "123456");
    HostIdentity {
        device_id: "the-device".into(),
        device_name: "The Device".into(),
        port: 21116,
        salt,
        verifier,
    }
}

/// Open a connection and run a handshake on it.
async fn handshake_with(
    trusted: Vec<TrustedMachine>,
    credential: Credential,
    controller_id: &str,
) -> (
    Result<skycremote_core::handshake::Session, anyhow::Error>,
    Result<skycremote_core::handshake::Session, anyhow::Error>,
) {
    let identity = identity();
    let (server, server_address) = listen(0).await.expect("the port must open");
    let client_endpoint = connect(server_address).await.expect("the port must open");

    let accept_task = tokio::spawn(async move {
        accept(&server, Duration::from_secs(20))
            .await
            .expect("the connection must come in")
    });

    let client = client_endpoint
        .connect(server_address, "skycremote")
        .expect("the connection must start")
        .await
        .expect("the connection must be made");
    let server_connection = accept_task.await.expect("the task must finish");

    let (client_side, server_side) = tokio::join!(
        open(client, Side::Initiator),
        open(server_connection, Side::Responder),
    );
    let (client_sender, mut client_receiver) = client_side.expect("the client must open");
    let (server_sender, mut server_receiver) = server_side.expect("the host must open");

    let (host_task, controller_task) = tokio::join!(
        host_handshake(&server_sender, &mut server_receiver, &identity, &trusted),
        controller_handshake(
            &client_sender,
            &mut client_receiver,
            &identity,
            controller_id,
            credential,
        ),
    );
    (host_task, controller_task)
}

/// A machine that is remembered comes in without the access code.
#[tokio::test]
async fn a_remembered_machine_comes_in_without_the_code() {
    let (salt, verifier) = crypto::trusted_pair();
    let trusted = vec![TrustedMachine {
        controller_id: "the-controller".to_string(),
        salt: salt.clone(),
        verifier,
    }];

    let (host, controller) = handshake_with(
        trusted,
        Credential::Trusted { salt, verifier },
        "the-controller",
    )
    .await;

    assert!(host.is_ok(), "the host must let the machine in");
    assert!(controller.is_ok(), "the machine must be let in");
}

/// A machine that says it is remembered but is not is not let in.
#[tokio::test]
async fn a_machine_that_lies_about_being_remembered_is_refused() {
    let (salt, verifier) = crypto::trusted_pair();

    let (host, controller) = handshake_with(
        Vec::new(),
        Credential::Trusted { salt, verifier },
        "the-controller",
    )
    .await;

    assert!(
        host.is_err(),
        "the host must not let a machine in that it does not know"
    );
    assert!(controller.is_err(), "the machine must not get in either");
}

/// The value of one machine is of no use to another.
#[tokio::test]
async fn the_value_of_one_machine_does_not_open_another() {
    let (salt, verifier) = crypto::trusted_pair();
    let trusted = vec![TrustedMachine {
        controller_id: "another-controller".to_string(),
        salt: salt.clone(),
        verifier,
    }];

    let (host, controller) = handshake_with(
        trusted,
        Credential::Trusted { salt, verifier },
        "the-controller",
    )
    .await;

    assert!(
        host.is_err(),
        "the host must not let in a machine it did not give the value to"
    );
    assert!(controller.is_err(), "the machine must not get in either");
}

/// The access code still works the way it always did.
#[tokio::test]
async fn the_access_code_still_lets_a_machine_in() {
    let (host, controller) = handshake_with(
        Vec::new(),
        Credential::Code("123456".to_string()),
        "the-controller",
    )
    .await;

    assert!(host.is_ok(), "the host must let the machine in");
    assert!(controller.is_ok(), "the machine must be let in");
}

/// Two values that are made are never the same.
#[test]
fn two_values_are_never_the_same() {
    let (_, one) = crypto::trusted_pair();
    let (_, other) = crypto::trusted_pair();
    assert_ne!(one, other, "two values must not be the same");
}
