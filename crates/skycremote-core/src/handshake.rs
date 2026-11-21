//! The handshake that opens a session.
//!
//! The channel is already up when the handshake starts, but a channel
//! on its own says nothing: the certificate of the host is one it made
//! for itself, so anybody could have made one like it.  The handshake
//! is what turns the channel into a session between two machines that
//! know each other's access code.
//!
//! It goes like this:
//!
//! ```text
//!   controller                          host
//!     |  --- hello (public key) ------>  |
//!     |  <-- hello-ack (public key) --  |
//!     |                                 |
//!     |  both turn the access code into |
//!     |  the same value                 |
//!     |                                 |
//!     |  --- auth (proof) ------------>  |
//!     |  <-- auth-result --------------  |
//! ```
//!
//! The proof says "I know the access code" without saying what it is.
//! It is made over the shared secret of the two ends, so it cannot be
//! made by somebody who is only watching, and it cannot be made with
//! the wrong code.
//!
//! The host does not keep the access code, only the value the code is
//! turned into.  The controller types the code and turns it into the
//! same value.  When the two values are the same, the two ends are
//! really talking to each other.
//!
//! The six digits the two ends show each other come from the same
//! material.  When they are the same on both screens, nobody is in the
//! middle; when they are not, somebody is.

use anyhow::{anyhow, Context, Result};

use crate::crypto::{self, KeyPair};
use crate::protocol::{Auth, AuthResult, Hello, HelloAck, Message};
use crate::transport::{Receiver, Sender};

/// What the host needs to know about itself for the handshake.
#[derive(Debug, Clone)]
pub struct HostIdentity {
    /// The id of this device.
    pub device_id: String,
    /// The name to show for this device.
    pub device_name: String,
    /// The port the host listens on.
    pub port: u16,
    /// The salt of this device.  It is different on every device, so
    /// that the same access code means something different on each of
    /// them.
    pub salt: Vec<u8>,
    /// What the access code of this device is turned into.
    pub verifier: [u8; 32],
}

/// A machine this device trusts.
///
/// The value is what the machine proves itself with.  It is not a code
/// anybody can guess: it is 32 bytes of randomness that this device
/// made when it decided to trust that machine, and it was handed over
/// on the encrypted line of the session at that moment.
///
/// The id of the machine is only a name for the entry.  Anybody can say
/// any name, so the name alone lets nobody in: the value is what does
/// that.
#[derive(Debug, Clone)]
pub struct TrustedMachine {
    /// The id of the machine that is trusted.
    pub controller_id: String,
    /// The salt that goes with the value.
    ///
    /// It is different for every trusted machine, so that the same
    /// value means something different on each of them.
    pub salt: Vec<u8>,
    /// The value the machine proves itself with.
    pub verifier: [u8; 32],
}

/// How a controlling machine proves that it may come in.
#[derive(Debug, Clone)]
pub enum Credential {
    /// The access code of the device, typed by the person.
    ///
    /// It is turned into the value the device kept, the same way it
    /// always was.
    Code(String),
    /// A value the device handed over when it trusted this machine.
    ///
    /// With this the machine comes in without anybody typing anything.
    Trusted {
        /// The salt that goes with the value.
        salt: Vec<u8>,
        /// The value itself.
        verifier: [u8; 32],
    },
}

/// What the handshake agreed on.
#[derive(Debug, Clone)]
pub struct Session {
    /// The six digits the two ends show to each other.
    pub verify_code: String,
    /// The id of the machine that is doing the controlling.
    ///
    /// It is empty on the controlling side: there is nobody there.
    pub controller_id: String,
    /// The name the controlling machine gives itself.
    ///
    /// It is empty on the controlling side.
    pub controller_name: String,
}

/// The controller opens a session.
///
/// It sends its public key, waits for the host's, checks that the host
/// knows the access code, proves that it does too, and puts the keys
/// of the session on the channel.
pub async fn controller_handshake(
    sender: &Sender,
    receiver: &mut Receiver,
    identity: &HostIdentity,
    controller_id: &str,
    credential: Credential,
) -> Result<Session> {
    let keys = KeyPair::generate();
    // Whether this end comes in with a code or with a value it was
    // given when it was trusted.  The other end has to do the same,
    // and it says which way it went in the answer.
    let trusted = matches!(credential, Credential::Trusted { .. });
    let hello = Message::Hello(Hello {
        version: crate::PROTOCOL_VERSION,
        device_id: identity.device_id.clone(),
        device_name: identity.device_name.clone(),
        public_key: keys.public_bytes().to_vec(),
        port: identity.port,
        controller_id: controller_id.to_string(),
        trusted,
    });
    send(sender, &hello).await?;

    // The host answers with its public key and the salt it used.
    let reply = receive(receiver).await?;
    let Message::HelloAck(ack) = reply else {
        return Err(anyhow!("the other end did not answer with a hello"));
    };
    if ack.version != crate::PROTOCOL_VERSION {
        return Err(anyhow!(
            "the other end speaks another version of the protocol ({} against {})",
            ack.version,
            crate::PROTOCOL_VERSION
        ));
    }
    if !ack.allowed {
        return Err(anyhow!("{}", ack.reason));
    }

    // The other end has to have taken the same way in.  If it did not,
    // the value this end holds is not the one the other end is
    // checking, and the session would stop a moment later anyway.
    if ack.trusted != trusted {
        return Err(anyhow!(
            "the other end did not take the way in that was offered"
        ));
    }

    // The shared secret only the two ends know.  Somebody in the
    // middle can put their own key in place of the host's, but then
    // the secret is not the same on both sides and the proof fails.
    let shared = keys
        .shared_secret(&ack.public_key)
        .map_err(|err| anyhow!("the other end sent a bad key: {err}"))?;

    // The value that proves this end may come in.
    //
    // With a code, the value is the code turned into something, with
    // the salt that came back.  With a value that was given when this
    // machine was trusted, the value is the one that was given: the
    // salt has to be the same as the one it was given with, or the
    // value means something else here and there.
    let verifier = match &credential {
        Credential::Code(code) => crypto::code_verifier(&ack.salt, code),
        Credential::Trusted { salt, verifier } => {
            if ack.salt != *salt {
                return Err(anyhow!("the other end changed the value of this machine"));
            }
            *verifier
        }
    };

    // The host's proof.  If it does not match, then either the code is
    // wrong or somebody in the middle is pretending to be the host.
    let host_transcript = transcript(&keys.public_bytes(), &ack.public_key, &ack.salt, b"host");
    let expected_host = crypto::access_proof(&shared, &verifier, &host_transcript);
    if !crypto::proofs_match(&ack.proof, &expected_host) {
        return Err(anyhow!("访问口令不对，或者中间有人"));
    }

    // The proof of this end.
    let controller_transcript = transcript(
        &keys.public_bytes(),
        &ack.public_key,
        &ack.salt,
        b"controller",
    );
    let proof = crypto::access_proof(&shared, &verifier, &controller_transcript);
    send(
        sender,
        &Message::Auth(Auth {
            proof: proof.to_vec(),
            trusted,
        }),
    )
    .await?;

    // The host has checked the proof by now.
    let answer = receive(receiver).await?;
    let Message::AuthResult(result) = answer else {
        return Err(anyhow!("the other end did not answer the access code"));
    };
    if !result.allowed {
        return Err(anyhow!("{}", result.reason));
    }

    // The keys of the session.  From here on everything is encrypted.
    let session = crypto::derive_session(&shared, &ack.salt, &verifier, true)?;
    sender.set_ciphers(
        session.control.outgoing.clone(),
        session.video.outgoing.clone(),
    );
    receiver.set_ciphers(
        session.control.incoming.clone(),
        session.video.incoming.clone(),
    );

    Ok(Session {
        verify_code: session.verify_code,
        controller_id: String::new(),
        controller_name: String::new(),
    })
}

/// The host answers a session.
///
/// It answers the hello with its own public key and the salt of the
/// device, waits for the proof that the access code is known, and puts
/// the keys of the session on the channel.
pub async fn host_handshake(
    sender: &Sender,
    receiver: &mut Receiver,
    identity: &HostIdentity,
    trusted: &[TrustedMachine],
) -> Result<Session> {
    let keys = KeyPair::generate();

    let hello = receive(receiver).await?;
    let Message::Hello(hello) = hello else {
        return Err(anyhow!("the other end did not say hello"));
    };
    if hello.version != crate::PROTOCOL_VERSION {
        return Err(anyhow!(
            "the other end speaks another version of the protocol ({} against {})",
            hello.version,
            crate::PROTOCOL_VERSION
        ));
    }

    // The shared secret of the two ends.
    let shared = keys
        .shared_secret(&hello.public_key)
        .map_err(|err| anyhow!("the other end sent a bad key: {err}"))?;

    // A machine this device trusts comes in with the value it was given
    // when it was trusted.  The entry is looked up by the id of that
    // machine, and the value and the salt come out of it.  The id is
    // only a name: the value is what lets the machine in, so anybody
    // who says the name of a trusted machine without the value gets
    // nowhere.
    //
    // A machine that is not trusted uses the access code of this
    // device, and the salt of this device.
    let (salt, verifier) = if hello.trusted {
        match trusted
            .iter()
            .find(|machine| machine.controller_id == hello.controller_id)
        {
            Some(machine) => (machine.salt.clone(), machine.verifier),
            None => {
                // It says it is trusted but it is not.  The session
                // does not go on, and the other end is told why so
                // that it can ask for the access code instead.
                send(
                    sender,
                    &Message::HelloAck(HelloAck {
                        version: crate::PROTOCOL_VERSION,
                        public_key: keys.public_bytes().to_vec(),
                        allowed: false,
                        reason: "这台电脑没有被信任".to_string(),
                        salt: Vec::new(),
                        proof: Vec::new(),
                        verify_code: String::new(),
                        trusted: true,
                    }),
                )
                .await?;
                return Err(anyhow!("这台电脑没有被信任"));
            }
        }
    } else {
        (identity.salt.clone(), identity.verifier)
    };

    // The host's own proof.  The controller checks it before it sends
    // its own, so that a machine that does not know the code is found
    // out before anything is sent to it.
    let host_transcript = transcript(&hello.public_key, &keys.public_bytes(), &salt, b"host");
    let proof = crypto::access_proof(&shared, &verifier, &host_transcript);

    send(
        sender,
        &Message::HelloAck(HelloAck {
            version: crate::PROTOCOL_VERSION,
            public_key: keys.public_bytes().to_vec(),
            allowed: true,
            reason: String::new(),
            salt: salt.clone(),
            proof: proof.to_vec(),
            verify_code: String::new(),
            trusted: hello.trusted,
        }),
    )
    .await?;

    // The proof of the controller.
    let answer = receive(receiver).await?;
    let Message::Auth(auth) = answer else {
        return Err(anyhow!("the other end did not send the access code"));
    };
    // The way in has to be the same on both sides.
    if auth.trusted != hello.trusted {
        send(
            sender,
            &Message::AuthResult(AuthResult {
                allowed: false,
                reason: "两边说的不是一回事".to_string(),
            }),
        )
        .await?;
        return Err(anyhow!("两边说的不是一回事"));
    }
    let controller_transcript = transcript(
        &hello.public_key,
        &keys.public_bytes(),
        &salt,
        b"controller",
    );
    let expected = crypto::access_proof(&shared, &verifier, &controller_transcript);
    if !crypto::proofs_match(&auth.proof, &expected) {
        // The code is wrong, or somebody in the middle is pretending.
        // Either way this session does not go on.
        send(
            sender,
            &Message::AuthResult(AuthResult {
                allowed: false,
                reason: "访问口令不对".to_string(),
            }),
        )
        .await?;
        return Err(anyhow!("访问口令不对"));
    }

    send(
        sender,
        &Message::AuthResult(AuthResult {
            allowed: true,
            reason: String::new(),
        }),
    )
    .await?;

    let session = crypto::derive_session(&shared, &salt, &verifier, false)?;
    sender.set_ciphers(
        session.control.outgoing.clone(),
        session.video.outgoing.clone(),
    );
    receiver.set_ciphers(
        session.control.incoming.clone(),
        session.video.incoming.clone(),
    );

    Ok(Session {
        verify_code: session.verify_code,
        controller_id: hello.controller_id,
        controller_name: hello.device_name,
    })
}

/// The bytes the proof is made over.
///
/// The transcript says who is talking to whom and with which salt.
/// Without it, a proof made for one session could be used in another.
fn transcript(first: &[u8], second: &[u8], salt: &[u8], role: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(64 + salt.len() + role.len());
    out.extend_from_slice(first);
    out.extend_from_slice(second);
    out.extend_from_slice(salt);
    out.extend_from_slice(role);
    out
}

/// Send one message.
async fn send(sender: &Sender, message: &Message) -> Result<()> {
    let text = serde_json::to_string(message).context("the message could not be written")?;
    sender
        .send_control(bytes::Bytes::from(text))
        .await
        .context("the message could not be sent")
}

/// Wait for one message.
async fn receive(receiver: &mut Receiver) -> Result<Message> {
    loop {
        match receiver.recv().await {
            crate::transport::Incoming::Control(data) => {
                let text = std::str::from_utf8(&data).context("the message was not readable")?;
                return serde_json::from_str(text).context("the message was not understood");
            }
            // A picture that arrives during the handshake is not
            // wanted; the host only starts sending once the handshake
            // is done.
            crate::transport::Incoming::Video(_) => continue,
            crate::transport::Incoming::Closed => return Err(anyhow!("the other end hung up")),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The two ends must end up with the same keys, or nothing that
    /// is sent can be read on the other side.
    #[test]
    fn the_two_ends_end_up_with_the_same_keys() {
        let salt = b"the salt of this device".to_vec();
        let verifier = crypto::code_verifier(&salt, "123456");

        let controller_keys = KeyPair::generate();
        let host_keys = KeyPair::generate();
        let shared_controller = controller_keys
            .shared_secret(&host_keys.public_bytes())
            .expect("derivable");
        let shared_host = host_keys
            .shared_secret(&controller_keys.public_bytes())
            .expect("derivable");
        assert_eq!(
            shared_controller, shared_host,
            "the two ends must have the same shared secret"
        );

        let controller =
            crypto::derive_session(&shared_controller, &salt, &verifier, true).expect("derivable");
        let host =
            crypto::derive_session(&shared_host, &salt, &verifier, false).expect("derivable");

        let message = b"a message";
        let encrypted = controller
            .control
            .outgoing
            .encrypt(message)
            .expect("encryptable");
        let plain = host
            .control
            .incoming
            .decrypt(&encrypted)
            .expect("the host must be able to read it");
        assert_eq!(plain, message);
    }

    #[test]
    fn the_transcript_says_who_is_talking() {
        let one = [1u8; 32];
        let two = [2u8; 32];
        let salt = [3u8; 32];

        let controller = transcript(&one, &two, &salt, b"controller");
        let host = transcript(&one, &two, &salt, b"host");
        assert_ne!(
            controller, host,
            "the two sides must not make the same proof"
        );

        let other = transcript(&one, &two, &[4u8; 32], b"controller");
        assert_ne!(
            controller, other,
            "another salt must not make the same proof"
        );
    }

    /// Two ends that know the code get a session, and the six digits
    /// are the same on both sides.
    #[tokio::test]
    async fn two_ends_with_the_same_code_get_a_session() {
        let salt = b"the salt of this device".to_vec();
        let verifier = crypto::code_verifier(&salt, "123456");
        let identity = HostIdentity {
            device_id: "the-device".into(),
            device_name: "The Device".into(),
            port: 21116,
            salt: salt.clone(),
            verifier,
        };

        let (server, server_address) = crate::transport::listen(0)
            .await
            .expect("the port must open");
        let client_endpoint = crate::transport::connect(server_address)
            .await
            .expect("the port must open");

        let accept_task = tokio::spawn(async move {
            crate::transport::accept(&server, std::time::Duration::from_secs(10))
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
            crate::transport::open(client, crate::transport::Side::Initiator),
            crate::transport::open(server_connection, crate::transport::Side::Responder),
        );
        let (client_sender, mut client_receiver) = client_side.expect("the client must open");
        let (server_sender, mut server_receiver) = server_side.expect("the host must open");

        let (host_task, controller_task) = tokio::join!(
            host_handshake(&server_sender, &mut server_receiver, &identity, &[]),
            controller_handshake(
                &client_sender,
                &mut client_receiver,
                &identity,
                "the-controller",
                Credential::Code("123456".to_string()),
            ),
        );

        let host = host_task.expect("the host must get in");
        let controller = controller_task.expect("the controller must get in");
        assert_eq!(
            host.verify_code, controller.verify_code,
            "the two ends must show the same six digits"
        );
    }

    /// A controller that does not know the code is not let in.
    #[tokio::test]
    async fn a_controller_with_the_wrong_code_is_refused() {
        let salt = b"the salt of this device".to_vec();
        let verifier = crypto::code_verifier(&salt, "123456");
        let identity = HostIdentity {
            device_id: "the-device".into(),
            device_name: "The Device".into(),
            port: 21116,
            salt: salt.clone(),
            verifier,
        };

        let (server, server_address) = crate::transport::listen(0)
            .await
            .expect("the port must open");
        let client_endpoint = crate::transport::connect(server_address)
            .await
            .expect("the port must open");

        let accept_task = tokio::spawn(async move {
            crate::transport::accept(&server, std::time::Duration::from_secs(10))
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
            crate::transport::open(client, crate::transport::Side::Initiator),
            crate::transport::open(server_connection, crate::transport::Side::Responder),
        );
        let (client_sender, mut client_receiver) = client_side.expect("the client must open");
        let (server_sender, mut server_receiver) = server_side.expect("the host must open");

        let (host_task, controller_task) = tokio::join!(
            host_handshake(&server_sender, &mut server_receiver, &identity, &[]),
            controller_handshake(
                &client_sender,
                &mut client_receiver,
                &identity,
                "the-controller",
                Credential::Code("654321".to_string()),
            ),
        );

        assert!(
            controller_task.is_err(),
            "the controller must not get in with the wrong code"
        );
        assert!(
            host_task.is_err(),
            "the host must not let the wrong code in"
        );
    }
}
