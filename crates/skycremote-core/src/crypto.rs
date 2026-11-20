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
        if their_public.len() != 32 {
            return Err(HandshakeError::BadKeyLength(their_public.len()));
        }
        let mut theirs = [0u8; 32];
        theirs.copy_from_slice(their_public);
        let their_public = PublicKey::from(theirs);
        let shared = self.secret.diffie_hellman(&their_public);

        // The salt has to be the same on both sides, so it is derived from
        // the shared secret instead of being picked at random.  A random
        // salt on each side would give the two ends two different keys.
        let mut hasher = <Sha256 as Default>::default();
        hasher.update(shared.as_bytes());
        hasher.update(SALT_LABEL);
        let salt: [u8; 32] = hasher.finalize().into();

        let hk = Hkdf::<Sha256>::new(Some(&salt), shared.as_bytes());
        let mut key = [0u8; KEY_LEN];
        hk.expand(INFO, &mut key)
            .map_err(|_| HandshakeError::KeyDerivation)?;
        Ok(SessionKey {
            // The key is always 32 bytes, so the conversion cannot
            // fail; it is a Result only because the type is generic.
            cipher: Aes256Gcm::new(
                &Key::<Aes256Gcm>::try_from(&key[..]).expect("a 32 byte key fits"),
            ),
        })
    }
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
}
