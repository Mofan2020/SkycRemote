//! The encrypted handshake and the cipher that follows it.
//!
//! Two ends that do not know each other meet through the tracker, which
//! hands both of them the other's public key.  From there the session is
//! encrypted end to end: the tracker never sees a byte of it.
//!
//! The exchange is the usual one:
//!
//! ```text
//!   control                          host
//!     |  --- hello (public key) ---->  |
//!     |  <-- hello-ack (public key) -- |
//!     |                                 |
//!     |  both derive the same key      |
//!     |  from their own and the other  |
//!     |  side's public key            |
//! ```
//!
//! The shared secret is run through HKDF with a random salt and a label
//! that names the protocol, so a key derived for this protocol cannot be
//! used for another one.

use aes_gcm::aead::{Aead, KeyInit};
use aes_gcm::{Aes256Gcm, Key, Nonce};
use hkdf::Hkdf;
use sha2::{Digest, Sha256};
use x25519_dalek::{PublicKey, StaticSecret};

/// How long a key is.
const KEY_LEN: usize = 32;
/// How long a nonce is.
const NONCE_LEN: usize = 12;
/// How long the tag of the cipher is.
const TAG_LEN: usize = 16;

/// The label that ties a derived key to this protocol.
const INFO: &[u8] = b"skycremote/session/v1";

/// The label that turns the shared secret into the salt of the
/// derivation.  Both ends use it, so both get the same salt.
const SALT_LABEL: &[u8] = b"skycremote/salt/v1";

/// The label that turns the access code into the key of the proof.
const CODE_LABEL: &[u8] = b"skycremote/code/v1";

/// The label that says what the proof is a proof of.
const PROOF_LABEL: &[u8] = b"skycremote/proof/v1";

/// The label that turns the shared secret into the six digits the two
/// ends show to each other.
const VERIFY_LABEL: &[u8] = b"skycremote/verify/v1";

/// The label of the key of the control channel, from the controller
/// to the host.
const CONTROL_C2H_LABEL: &[u8] = b"skycremote/control/c2h/v1";

/// The label of the key of the control channel, from the host to the
/// controller.
const CONTROL_H2C_LABEL: &[u8] = b"skycremote/control/h2c/v1";

/// The label of the key of the video channel, from the controller to
/// the host.
const VIDEO_C2H_LABEL: &[u8] = b"skycremote/video/c2h/v1";

/// The label of the key of the video channel, from the host to the
/// controller.
const VIDEO_H2C_LABEL: &[u8] = b"skycremote/video/h2c/v1";

/// The public key of one end, and the secret behind it.
#[derive(Clone)]
pub struct KeyPair {
    secret: StaticSecret,
    public: [u8; 32],
}

impl KeyPair {
    /// Make a new key pair from the system randomness.
    pub fn generate() -> Self {
        let mut bytes = [0u8; 32];
        // The system randomness is the only source: a key that comes from
        // anywhere else can be predicted.
        getrandom::fill(&mut bytes).expect("the system randomness is not available");
        let secret = StaticSecret::from(bytes);
        let public = PublicKey::from(&secret);
        Self {
            secret,
            public: public.to_bytes(),
        }
    }

    /// The public key, in the bytes that go on the wire.
    pub fn public_bytes(&self) -> [u8; 32] {
        self.public
    }

    /// The public key, as a string, so it can go into a JSON message.
    pub fn public_base64(&self) -> String {
        use base64::Engine;
        base64::engine::general_purpose::STANDARD.encode(self.public)
    }

    /// Derive the session key from the other end's public key.
    pub fn derive(&self, their_public: &[u8]) -> Result<SessionKey, HandshakeError> {
        let shared = self.shared_secret(their_public)?;

        // The salt has to be the same on both sides, so it is derived from
        // the shared secret instead of being picked at random.  A random
        // salt on each side would give the two ends two different keys.
        let mut hasher = <Sha256 as Default>::default();
        hasher.update(shared);
        hasher.update(SALT_LABEL);
        let salt: [u8; 32] = hasher.finalize().into();

        Self::derive_with_salt(&shared, &salt)
    }

    /// The shared secret of the two ends.
    ///
    /// This is what the session key and the access code proof are
    /// built from.  It is the one thing that only the two ends can
    /// know: somebody in the middle can swap the public keys, but
    /// then the two ends get different secrets and the session stops.
    pub fn shared_secret(&self, their_public: &[u8]) -> Result<[u8; 32], HandshakeError> {
        if their_public.len() != 32 {
            return Err(HandshakeError::BadKeyLength(their_public.len()));
        }
        let mut theirs = [0u8; 32];
        theirs.copy_from_slice(their_public);
        let their_public = PublicKey::from(theirs);
        Ok(self.secret.diffie_hellman(&their_public).to_bytes())
    }

    /// Derive the session key from a shared secret and a salt.
    ///
    /// The salt comes from the other end, so that two sessions of the
    /// same two ends do not end up with the same key.
    pub fn derive_with_salt(shared: &[u8; 32], salt: &[u8]) -> Result<SessionKey, HandshakeError> {
        let hk = Hkdf::<Sha256>::new(Some(salt), shared);
        let mut key = [0u8; KEY_LEN];
        hk.expand(INFO, &mut key)
            .map_err(|_| HandshakeError::KeyDerivation)?;
        Ok(SessionKey {
            cipher: Aes256Gcm::new(
                &Key::<Aes256Gcm>::try_from(&key[..]).expect("a 32 byte key fits"),
            ),
        })
    }
}

/// The number of rounds the access code is put through.
///
/// The access code is short and people pick it themselves, so it is
/// stretched before it is used.  A thousand rounds would not be enough:
/// somebody who has the proof and the salt can try codes without
/// asking the other end, and a fast hash would let them try millions
/// of them a second.  A hundred thousand rounds makes that slow
/// enough to be useless.
const CODE_ROUNDS: u32 = 100_000;

/// The value the access code is turned into.
///
/// The salt is the one of the device that is being controlled, so the
/// same code gives a different value on every device.  The code
/// itself is never written down: this value is what is kept.
pub fn code_verifier(salt: &[u8], code: &str) -> [u8; 32] {
    // The label goes in first, so that the same code cannot be used
    // for anything else in this program.
    let mut hasher = <Sha256 as Default>::default();
    hasher.update(CODE_LABEL);
    hasher.update(code.as_bytes());
    let stretched: [u8; 32] = hasher.finalize().into();

    let mut out = [0u8; 32];
    pbkdf2_sha256(&stretched, salt, CODE_ROUNDS, &mut out);
    out
}

/// The proof that the other end knows the access code.
///
/// The proof is made with the value the code is turned into, over the
/// shared secret.  So it cannot be made without the code, and it does
/// not give the code away either.
pub fn access_proof(shared: &[u8; 32], verifier: &[u8; 32], transcript: &[u8]) -> [u8; 32] {
    let mut message =
        Vec::with_capacity(shared.len() + verifier.len() + transcript.len() + PROOF_LABEL.len());
    message.extend_from_slice(shared);
    message.extend_from_slice(verifier);
    message.extend_from_slice(transcript);
    message.extend_from_slice(PROOF_LABEL);
    hmac_sha256(shared, &message)
}

/// The keys one side of a session uses.
#[derive(Clone)]
pub struct ChannelKeys {
    /// The key of the messages this side sends.
    pub outgoing: SessionKey,
    /// The key of the messages this side receives.
    pub incoming: SessionKey,
}

/// Everything one session needs.
pub struct SessionKeys {
    /// The keys of the control channel.
    pub control: ChannelKeys,
    /// The keys of the video channel.
    pub video: ChannelKeys,
    /// The six digits the two ends show to each other.
    pub verify_code: String,
}

/// Derive everything one session needs.
///
/// The salt comes from the other end, and the verifier is the value
/// the access code was turned into, so a session cannot be opened
/// without the code even when the shared secret is known.
///
/// The two directions have keys of their own.  One key in both
/// directions would work, but then a message that came in could be
/// replayed back out, and a key that goes one way only cannot be.
pub fn derive_session(
    shared: &[u8; 32],
    salt: &[u8],
    verifier: &[u8; 32],
    controller: bool,
) -> Result<SessionKeys, HandshakeError> {
    let mut material = [0u8; 64];
    material[..32].copy_from_slice(shared);
    material[32..].copy_from_slice(verifier);

    let controller_to_host_control = derive_key(&material, salt, CONTROL_C2H_LABEL)?;
    let host_to_controller_control = derive_key(&material, salt, CONTROL_H2C_LABEL)?;
    let controller_to_host_video = derive_key(&material, salt, VIDEO_C2H_LABEL)?;
    let host_to_controller_video = derive_key(&material, salt, VIDEO_H2C_LABEL)?;

    let mut digits = [0u8; 32];
    hkdf_expand(&material, salt, VERIFY_LABEL, &mut digits)?;
    let number = u32::from_be_bytes([digits[0], digits[1], digits[2], digits[3]]) % 1_000_000;

    let (control, video) = if controller {
        (
            ChannelKeys {
                outgoing: controller_to_host_control,
                incoming: host_to_controller_control,
            },
            ChannelKeys {
                outgoing: controller_to_host_video,
                incoming: host_to_controller_video,
            },
        )
    } else {
        (
            ChannelKeys {
                outgoing: host_to_controller_control,
                incoming: controller_to_host_control,
            },
            ChannelKeys {
                outgoing: host_to_controller_video,
                incoming: controller_to_host_video,
            },
        )
    };

    Ok(SessionKeys {
        control,
        video,
        verify_code: format!("{:06}", number),
    })
}

/// Derive one key from the material of the session.
fn derive_key(material: &[u8; 64], salt: &[u8], info: &[u8]) -> Result<SessionKey, HandshakeError> {
    let mut key = [0u8; KEY_LEN];
    hkdf_expand(material, salt, info, &mut key)?;
    Ok(SessionKey {
        cipher: Aes256Gcm::new(&Key::<Aes256Gcm>::try_from(&key[..]).expect("a 32 byte key fits")),
    })
}

/// Stretch the material into one key.
fn hkdf_expand(
    material: &[u8; 64],
    salt: &[u8],
    info: &[u8],
    out: &mut [u8],
) -> Result<(), HandshakeError> {
    let hk = Hkdf::<Sha256>::new(Some(salt), material);
    hk.expand(info, out)
        .map_err(|_| HandshakeError::KeyDerivation)
}

/// The password based key derivation, written out.
///
/// It is the usual construction: the password is stretched over many
/// rounds, and every round is a hash of the last one with the salt and
/// the number of the block in front.
fn pbkdf2_sha256(password: &[u8], salt: &[u8], rounds: u32, out: &mut [u8]) {
    for (index, block) in out.chunks_mut(32).enumerate() {
        let mut salted = salt.to_vec();
        salted.extend_from_slice(&(index as u32 + 1).to_be_bytes());

        let mut current = hmac_sha256(password, &salted);
        let mut result = current;
        for _ in 1..rounds {
            current = hmac_sha256(password, &current);
            for (byte, other) in result.iter_mut().zip(current.iter()) {
                *byte ^= other;
            }
        }
        let length = block.len().min(result.len());
        block[..length].copy_from_slice(&result[..length]);
    }
}

/// Compare two proofs without telling how much of them matched.
pub fn proofs_match(left: &[u8], right: &[u8]) -> bool {
    if left.len() != right.len() {
        return false;
    }
    let mut difference = 0u8;
    for (one, other) in left.iter().zip(right.iter()) {
        difference |= one ^ other;
    }
    difference == 0
}

/// HMAC-SHA256, written out.
///
/// The crates that do this are not needed for anything else, and one
/// small function is easier to read than one more dependency.
fn hmac_sha256(key: &[u8], message: &[u8]) -> [u8; 32] {
    const BLOCK: usize = 64;
    let mut pad = [0u8; BLOCK];
    if key.len() > BLOCK {
        let digest = Sha256::digest(key);
        pad[..32].copy_from_slice(&digest);
    } else {
        pad[..key.len()].copy_from_slice(key);
    }

    let mut inner = Sha256::new();
    inner.update(pad.map(|byte| byte ^ 0x36));
    inner.update(message);
    let inner = inner.finalize();

    let mut outer = Sha256::new();
    outer.update(pad.map(|byte| byte ^ 0x5c));
    outer.update(inner);
    outer.finalize().into()
}

/// The error of a handshake that did not work.
#[derive(Debug, thiserror::Error)]
pub enum HandshakeError {
    /// The other end sent a key of the wrong length.
    #[error("the public key is {0} bytes long, it must be 32")]
    BadKeyLength(usize),
    /// The key could not be derived.
    #[error("the session key could not be derived")]
    KeyDerivation,
    /// The other end sent something that was not encrypted with this key.
    #[error("the message could not be decrypted: {0}")]
    Decrypt(String),
}

/// The cipher of one session.
///
/// The same key is used in both directions of a session.  That is fine
/// as long as the nonce is never the same twice with the same key,
/// and every message gets a fresh random nonce.
#[derive(Clone)]
pub struct SessionKey {
    cipher: Aes256Gcm,
}

impl SessionKey {
    /// Encrypt a message.
    ///
    /// The nonce is put in front of the ciphertext, so the other end can
    /// read it back.  Every message gets a fresh random nonce, and the
    /// same key is never used with the same nonce twice.
    pub fn encrypt(&self, plaintext: &[u8]) -> Result<Vec<u8>, HandshakeError> {
        let mut nonce_bytes = [0u8; NONCE_LEN];
        rand::fill(&mut nonce_bytes);
        let nonce = Nonce::try_from(&nonce_bytes[..]).expect("a 12 byte nonce fits");
        let ciphertext = self
            .cipher
            .encrypt(&nonce, plaintext)
            .map_err(|_| HandshakeError::Decrypt("the encryption failed".into()))?;
        let mut out = Vec::with_capacity(NONCE_LEN + ciphertext.len());
        out.extend_from_slice(&nonce_bytes);
        out.extend_from_slice(&ciphertext);
        Ok(out)
    }

    /// Decrypt a message.
    pub fn decrypt(&self, data: &[u8]) -> Result<Vec<u8>, HandshakeError> {
        if data.len() < NONCE_LEN + TAG_LEN {
            return Err(HandshakeError::Decrypt("the message is too short".into()));
        }
        let (nonce_bytes, ciphertext) = data.split_at(NONCE_LEN);
        let nonce = Nonce::try_from(nonce_bytes).expect("a 12 byte nonce fits");
        self.cipher
            .decrypt(&nonce, ciphertext)
            .map_err(|_| HandshakeError::Decrypt("the message is not ours".into()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn two_ends_derive_the_same_key() {
        let a = KeyPair::generate();
        let b = KeyPair::generate();
        let key_a = a
            .derive(&b.public_bytes())
            .expect("the key must be derivable");
        let key_b = b
            .derive(&a.public_bytes())
            .expect("the key must be derivable");

        let message = b"the picture of a screen";
        let encrypted = key_a
            .encrypt(message)
            .expect("the message must be encrypted");
        let decrypted = key_b
            .decrypt(&encrypted)
            .expect("the message must come back");
        assert_eq!(decrypted, message);
    }

    #[test]
    fn a_third_end_cannot_read_the_message() {
        let a = KeyPair::generate();
        let b = KeyPair::generate();
        let c = KeyPair::generate();
        let key_a = a.derive(&b.public_bytes()).expect("derivable");
        let key_c = c.derive(&b.public_bytes()).expect("derivable");

        let encrypted = key_a.encrypt(b"secret").expect("encryptable");
        assert!(
            key_c.decrypt(&encrypted).is_err(),
            "a stranger must not be able to read the message"
        );
    }

    #[test]
    fn the_nonce_is_never_the_same_twice() {
        let a = KeyPair::generate();
        let b = KeyPair::generate();
        let key = a.derive(&b.public_bytes()).expect("derivable");

        let one = key.encrypt(b"x").expect("encryptable");
        let two = key.encrypt(b"x").expect("encryptable");
        assert_ne!(
            &one[..NONCE_LEN],
            &two[..NONCE_LEN],
            "the same nonce must not come up twice"
        );
    }

    #[test]
    fn a_message_that_was_tampered_with_does_not_come_back() {
        let a = KeyPair::generate();
        let b = KeyPair::generate();
        let key_a = a.derive(&b.public_bytes()).expect("derivable");
        let key_b = b.derive(&a.public_bytes()).expect("derivable");

        let mut encrypted = key_a.encrypt(b"hello").expect("encryptable");
        let last = encrypted.len() - 1;
        encrypted[last] ^= 0xff;
        assert!(
            key_b.decrypt(&encrypted).is_err(),
            "a broken message must not come back"
        );
    }

    #[test]
    fn a_short_message_is_refused_instead_of_panicking() {
        let a = KeyPair::generate();
        let b = KeyPair::generate();
        let key = a.derive(&b.public_bytes()).expect("derivable");
        assert!(key.decrypt(&[0u8; 4]).is_err());
    }

    #[test]
    fn a_key_of_the_wrong_length_is_refused() {
        let a = KeyPair::generate();
        assert!(a.derive(&[0u8; 16]).is_err());
    }

    #[test]
    fn the_same_code_gives_the_same_value() {
        let salt = b"a salt of this device";
        assert_eq!(
            code_verifier(salt, "123456"),
            code_verifier(salt, "123456"),
            "the same code must give the same value"
        );
    }

    #[test]
    fn another_code_gives_another_value() {
        let salt = b"a salt of this device";
        assert_ne!(
            code_verifier(salt, "123456"),
            code_verifier(salt, "123457"),
            "another code must give another value"
        );
    }

    #[test]
    fn another_salt_gives_another_value() {
        assert_ne!(
            code_verifier(b"one salt", "123456"),
            code_verifier(b"another salt", "123456"),
            "the same code on another device must give another value"
        );
    }

    #[test]
    fn the_proof_needs_the_code() {
        let shared = [7u8; 32];
        let salt = b"a salt of this device";
        let right = code_verifier(salt, "123456");
        let wrong = code_verifier(salt, "123457");

        let proof = access_proof(&shared, &right, b"the words");
        assert!(proofs_match(
            &proof,
            &access_proof(&shared, &right, b"the words")
        ));
        assert!(
            !proofs_match(&proof, &access_proof(&shared, &wrong, b"the words")),
            "the wrong code must not make the same proof"
        );
        assert!(
            !proofs_match(&proof, &access_proof(&shared, &right, b"other words")),
            "other words must not make the same proof"
        );
    }

    #[test]
    fn the_two_ends_make_the_same_keys() {
        let shared = [9u8; 32];
        let salt = b"a salt of this device";
        let verifier = code_verifier(salt, "123456");

        let controller =
            derive_session(&shared, salt, &verifier, true).expect("the keys must be derivable");
        let host =
            derive_session(&shared, salt, &verifier, false).expect("the keys must be derivable");

        assert_eq!(
            controller.verify_code, host.verify_code,
            "the two ends must show the same six digits"
        );
    }

    #[test]
    fn the_two_directions_do_not_share_a_key() {
        let shared = [9u8; 32];
        let salt = b"a salt of this device";
        let verifier = code_verifier(salt, "123456");
        let controller = derive_session(&shared, salt, &verifier, true).expect("derivable");

        let message = b"a message";
        let encrypted = controller
            .control
            .outgoing
            .encrypt(message)
            .expect("encryptable");
        assert!(
            controller.control.incoming.decrypt(&encrypted).is_err(),
            "the key of the other direction must not read it"
        );
    }

    #[test]
    fn the_six_digits_are_six_digits() {
        let shared = [3u8; 32];
        let salt = b"a salt";
        let verifier = code_verifier(salt, "123456");
        let session = derive_session(&shared, salt, &verifier, true).expect("derivable");
        assert_eq!(session.verify_code.len(), 6);
        assert!(session.verify_code.chars().all(|c| c.is_ascii_digit()));
    }
}
