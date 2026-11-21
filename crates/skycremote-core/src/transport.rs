//! The channel the two ends talk over.
//!
//! The traffic is split in two:
//!
//! * the **control channel** carries the messages of the protocol: the
//!   hello, the key events, the mouse events, the state of the session.
//!   It is reliable and ordered, because losing a key press is worse
//!   than waiting a little.
//! * the **video channel** carries the pictures.  It is not reliable:
//!   a picture that was late is worthless, and waiting for it would
//!   make the picture stutter.
//!
//! QUIC gives both of them.  The control traffic goes over a reliable
//! stream, the video goes over unreliable datagrams, which are exactly
//! "send it once, do not tell me if it arrived".
//!
//! QUIC also brings its own encryption.  That protects the traffic from
//! anyone who can watch the wire, but it does not say *who* is on the
//! other side -- a self signed certificate proves nothing.  The identity
//! of the other end comes from the handshake of
//! [`crate::crypto`], which runs over this channel and is what makes a
//! session safe against somebody in the middle.

use anyhow::{anyhow, Context, Result};
use bytes::{Bytes, BytesMut};
use parking_lot::RwLock;
use quinn::{Connection, Endpoint, RecvStream, SendStream};
use std::net::SocketAddr;
use std::sync::Arc;
use tokio::sync::mpsc;

use rustls::pki_types::{CertificateDer, PrivateKeyDer, PrivatePkcs8KeyDer, ServerName, UnixTime};
use rustls::{ClientConfig, DigitallySignedStruct, SignatureScheme};

/// How big one picture chunk may be before it is split.
const CHUNK_SIZE: usize = 16 * 1024;

/// The mark that says a frame is encrypted.
///
/// The handshake goes over the channel in the clear, because the key
/// that would encrypt it is only known after the handshake.  Every
/// frame says whether it is encrypted, so the other end always knows
/// what it is looking at.
const FLAG_PLAIN: u8 = 0;
/// The mark that says a frame is encrypted.
const FLAG_ENCRYPTED: u8 = 1;

/// A key that can be put on later.
///
/// The handshake happens after the channel is open, so the key cannot
/// be there from the start.  It is put on afterwards, and every copy
/// of the sender sees it, because they all share this.
#[derive(Clone, Default)]
pub struct CipherSlot(Arc<RwLock<Option<Arc<crate::crypto::SessionKey>>>>);

impl CipherSlot {
    /// Put a key on.
    pub fn set(&self, key: crate::crypto::SessionKey) {
        *self.0.write() = Some(Arc::new(key));
    }

    /// The key that is on, if there is one.
    pub fn get(&self) -> Option<Arc<crate::crypto::SessionKey>> {
        self.0.read().clone()
    }
}

/// Which side of the session this is.
///
/// The two sides are not the same: one of them opens the control
/// stream and the other one accepts it.  A bidirectional stream in
/// QUIC has one opener and one acceptor, and both ends have to agree
/// about who is which.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Side {
    /// The side that starts the connection and opens the stream.
    Initiator,
    /// The side that waits and accepts the stream.
    Responder,
}

/// The side of the channel that reads.
pub struct Receiver {
    /// The reliable stream the control messages come in.
    control: RecvStream,
    /// The datagrams the video comes in.
    video: mpsc::UnboundedReceiver<Bytes>,
    /// The connection, kept so the caller can ask about it.
    connection: Connection,
    /// The key of the control channel, once it has been agreed on.
    control_cipher: CipherSlot,
    /// The key of the video channel, once it has been agreed on.
    video_cipher: CipherSlot,
}

/// The side of the channel that writes.
///
/// The two ends each have one stream for reading and one for writing,
/// so both can talk at the same time without waiting for the other.
///
/// It can be copied.  The stream underneath cannot be, so it sits
/// behind a lock and the copies take turns writing.  That matters
/// because the two halves of a session write at the same time: one
/// sends the pictures, the other answers the other end.
#[derive(Clone)]
pub struct Sender {
    /// The reliable stream the control messages go out.
    control: Arc<tokio::sync::Mutex<SendStream>>,
    /// The connection, kept so the caller can ask about it.
    connection: Connection,
    /// The key of the control channel, once it has been agreed on.
    control_cipher: CipherSlot,
    /// The key of the video channel, once it has been agreed on.
    video_cipher: CipherSlot,
}

/// What comes out of the receiver.
#[derive(Debug)]
pub enum Incoming {
    /// A control message.
    Control(Bytes),
    /// A piece of a picture.
    Video(Bytes),
    /// The other end closed the connection.
    Closed,
}

impl Receiver {
    /// Wait for the next thing that arrives.
    pub async fn recv(&mut self) -> Incoming {
        // The video is checked first: a picture that is already waiting
        // must not be held back by the control stream.
        if let Ok(chunk) = self.video.try_recv() {
            return Incoming::Video(chunk);
        }
        tokio::select! {
            biased;
            chunk = self.video.recv() => match chunk {
                Some(chunk) => Incoming::Video(chunk),
                None => Incoming::Closed,
            },
            data = read_one_message(&mut self.control, &self.control_cipher) => match data {
                Ok(Some(data)) => Incoming::Control(data),
                Ok(None) => Incoming::Closed,
                Err(_) => Incoming::Closed,
            },
        }
    }

    /// The address of the other end.
    pub fn peer_address(&self) -> SocketAddr {
        self.connection.remote_address()
    }

    /// How long the trip to the other end takes, in microseconds.
    pub fn latency_us(&self) -> u64 {
        self.connection.rtt().as_micros() as u64
    }

    /// Whether the connection is still open.
    pub fn is_open(&self) -> bool {
        self.connection.close_reason().is_none()
    }

    /// Put the keys of the session on the channel.
    ///
    /// After this, everything that comes in is encrypted.
    pub fn set_ciphers(
        &self,
        control: crate::crypto::SessionKey,
        video: crate::crypto::SessionKey,
    ) {
        self.control_cipher.set(control);
        self.video_cipher.set(video);
    }
}

impl Sender {
    /// Send a control message.  It arrives, or it does not.
    ///
    /// The stream sends the data on its own; there is nothing to flush.
    ///
    /// Once the keys of the session are known, the message is
    /// encrypted before it goes out.  A flag in front of the message
    /// says whether it is, so that the other end always knows.
    pub async fn send_control(&self, data: Bytes) -> Result<()> {
        let (flag, body) = match self.control_cipher.get() {
            Some(key) => (
                FLAG_ENCRYPTED,
                key.encrypt(&data)
                    .map_err(|err| anyhow!("the message could not be encrypted: {err}"))?,
            ),
            None => (FLAG_PLAIN, data.to_vec()),
        };

        // Every message is sent with its length in front of it, because
        // a stream does not keep the borders between two messages: read
        // too much and two messages arrive as one.  The length counts
        // the mark as well, so the reader knows how many bytes are
        // coming before it reads any of them.
        let length = ((body.len() + 1) as u32).to_be_bytes();
        let mut control = self.control.lock().await;
        control
            .write_all(&length)
            .await
            .context("the control stream failed")?;
        control
            .write_all(&[flag])
            .await
            .context("the control stream failed")?;
        control
            .write_all(&body)
            .await
            .context("the control stream failed")?;
        Ok(())
    }

    /// Send a piece of a picture.  It arrives, or it does not, and
    /// nobody waits for the answer.
    ///
    /// The whole datagram is encrypted once the keys are known, so
    /// that the picture cannot be read on the way.
    pub fn send_video(&self, data: Bytes) -> Result<()> {
        let body = match self.video_cipher.get() {
            Some(key) => Bytes::from(
                key.encrypt(&data)
                    .map_err(|err| anyhow!("the picture could not be encrypted: {err}"))?,
            ),
            None => data,
        };
        self.connection
            .send_datagram(body)
            .map_err(|_| anyhow!("the video channel is closed"))
    }

    /// The address of the other end.
    pub fn peer_address(&self) -> SocketAddr {
        self.connection.remote_address()
    }

    /// How long the trip to the other end takes, in microseconds.
    pub fn latency_us(&self) -> u64 {
        self.connection.rtt().as_micros() as u64
    }

    /// Say goodbye and close the connection.
    pub fn close(&self) {
        self.connection.close(0u32.into(), b"bye");
    }

    /// Put the keys of the session on the channel.
    ///
    /// After this, everything that goes out is encrypted.
    pub fn set_ciphers(
        &self,
        control: crate::crypto::SessionKey,
        video: crate::crypto::SessionKey,
    ) {
        self.control_cipher.set(control);
        self.video_cipher.set(video);
    }
}

/// A certificate verifier that accepts anything.
///
/// The certificate of the host is self signed and proves nothing about
/// who the host is.  Who the host is comes from the handshake of
/// [`crate::crypto`]: both ends exchange public keys over this channel
/// and derive the same session key, and only somebody who is really
/// there can do that.  So the certificate is only there to make TLS
/// work, and it is not a security boundary.
#[derive(Debug)]
struct AcceptAnyCertificate;

impl rustls::client::danger::ServerCertVerifier for AcceptAnyCertificate {
    fn verify_server_cert(
        &self,
        _end_entity: &CertificateDer<'_>,
        _intermediates: &[CertificateDer<'_>],
        _server_name: &ServerName<'_>,
        _ocsp_response: &[u8],
        _now: UnixTime,
    ) -> Result<rustls::client::danger::ServerCertVerified, rustls::Error> {
        Ok(rustls::client::danger::ServerCertVerified::assertion())
    }

    fn verify_tls12_signature(
        &self,
        _message: &[u8],
        _cert: &CertificateDer<'_>,
        _dss: &DigitallySignedStruct,
    ) -> Result<rustls::client::danger::HandshakeSignatureValid, rustls::Error> {
        Ok(rustls::client::danger::HandshakeSignatureValid::assertion())
    }

    fn verify_tls13_signature(
        &self,
        _message: &[u8],
        _cert: &CertificateDer<'_>,
        _dss: &DigitallySignedStruct,
    ) -> Result<rustls::client::danger::HandshakeSignatureValid, rustls::Error> {
        Ok(rustls::client::danger::HandshakeSignatureValid::assertion())
    }

    fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
        vec![
            SignatureScheme::ECDSA_NISTP256_SHA256,
            SignatureScheme::ECDSA_NISTP384_SHA384,
            SignatureScheme::ED25519,
            SignatureScheme::RSA_PSS_SHA256,
            SignatureScheme::RSA_PSS_SHA384,
            SignatureScheme::RSA_PSS_SHA512,
        ]
    }
}

/// Open a port that waits for the other end to come in.
///
/// The port is bound on every interface, so the other end can reach it
/// from the outside once the hole is punched.  The address that is
/// returned is the loopback one, because that is the only address that
/// can be connected to from the same machine; from the outside the
/// address comes from the tracker.
pub async fn listen(port: u16) -> Result<(Endpoint, SocketAddr)> {
    let keys = self_signed_keys()?;
    let certificate = CertificateDer::from(keys.0);
    let private_key = PrivateKeyDer::Pkcs8(PrivatePkcs8KeyDer::from(keys.1));

    let mut server_config = rustls::ServerConfig::builder_with_provider(Arc::new(
        rustls::crypto::ring::default_provider(),
    ))
    .with_protocol_versions(&[&rustls::version::TLS13])
    .context("TLS 1.3 is not available")?
    .with_no_client_auth()
    .with_single_cert(vec![certificate], private_key)
    .context("the certificate could not be used")?;

    // QUIC needs the early data to be either off or wide open.
    server_config.max_early_data_size = u32::MAX;

    let quic_config = quinn::crypto::rustls::QuicServerConfig::try_from(Arc::new(server_config))
        .context("the server configuration could not be used")?;
    let mut config = quinn::ServerConfig::with_crypto(Arc::new(quic_config));

    let mut transport = quinn::TransportConfig::default();
    // The picture is the point of the whole thing, so the space for
    // datagrams is made big.
    // One two-way stream carries the control messages.
    transport.max_concurrent_uni_streams(0u32.into());
    transport.max_concurrent_bidi_streams(1u32.into());
    transport.max_idle_timeout(Some(
        std::time::Duration::from_secs(30)
            .try_into()
            .expect("thirty seconds fits"),
    ));
    // A picture that is late is worth nothing, so it is thrown away
    // instead of being resent.  That is what the datagrams do.
    transport.datagram_receive_buffer_size(Some(16 * 1024 * 1024));
    transport.datagram_send_buffer_size(16 * 1024 * 1024);
    // The control channel is the one stream that matters.
    transport.stream_receive_window(quinn::VarInt::from_u32(1024 * 1024));
    transport.send_window(16 * 1024 * 1024);
    config.transport_config(Arc::new(transport));

    let endpoint = Endpoint::server(config, SocketAddr::from(([0, 0, 0, 0], port)))?;
    let bound = endpoint.local_addr()?;
    // The bound address is on every interface; the loopback one is
    // given back because that is the only one that can be connected to
    // from the same machine.
    let address = SocketAddr::from(([127, 0, 0, 1], bound.port()));
    Ok((endpoint, address))
}

/// Open a port to reach the other end.
/// `address` is where the other end is.  The socket this end uses is not
/// tied to it -- the local address is picked by the system -- but the
/// caller keeps it here because it is the thing they are connecting to,
/// and a future version may need to bind a particular interface to reach
/// it.
pub async fn connect(_address: SocketAddr) -> Result<Endpoint> {
    let mut client_config =
        ClientConfig::builder_with_provider(Arc::new(rustls::crypto::ring::default_provider()))
            .with_protocol_versions(&[&rustls::version::TLS13])
            .context("TLS 1.3 is not available")?
            .dangerous()
            .with_custom_certificate_verifier(Arc::new(AcceptAnyCertificate))
            .with_no_client_auth();

    client_config.enable_early_data = true;

    let quic_config = quinn::crypto::rustls::QuicClientConfig::try_from(Arc::new(client_config))
        .context("the client configuration could not be used")?;
    let mut config = quinn::ClientConfig::new(Arc::new(quic_config));

    let mut transport = quinn::TransportConfig::default();
    transport.max_concurrent_uni_streams(0u32.into());
    transport.max_concurrent_bidi_streams(1u32.into());
    transport.max_idle_timeout(Some(
        std::time::Duration::from_secs(30)
            .try_into()
            .expect("thirty seconds fits"),
    ));
    config.transport_config(Arc::new(transport));

    let mut endpoint = Endpoint::client(SocketAddr::from(([0, 0, 0, 0], 0)))?;
    endpoint.set_default_client_config(config);
    Ok(endpoint)
}

/// Open a port to the other end, punching through the NAT on the way.
///
/// The socket that punches the hole is the socket the session then runs
/// on.  A NAT that has let one packet through will let the rest of the
/// flow through as well, but only if it is the same port number, so
/// the hole and the session have to share one socket.
pub async fn connect_direct(tracker: &str, device_id: &str) -> Result<(Sender, Receiver)> {
    let peer = crate::hole::find_peer(tracker, device_id)
        .await
        .context("the other end could not be found")?;
    let socket = std::net::UdpSocket::bind((std::net::Ipv4Addr::UNSPECIFIED, 0))
        .context("the socket for the direct connection could not be opened")?;
    let hole =
        crate::hole::punch(socket, peer).context("the direct connection could not be made")?;

    let client_config = client_config()?;
    let mut endpoint = client_endpoint(hole.socket)?;
    endpoint.set_default_client_config(client_config);
    let connecting = endpoint
        .connect(hole.peer, "skycremote")
        .context("the direct connection could not be started")?;
    let connection = connecting
        .await
        .context("the direct connection could not be made")?;
    open(connection, Side::Initiator).await
}

/// Wait for the other end on a port that was punched open.
///
/// The other end has to know where to send, so this registers with the
/// tracker first and only then waits.
pub async fn listen_direct(
    tracker: &str,
    device_id: &str,
    device_name: &str,
    port: u16,
    timeout: std::time::Duration,
) -> Result<(Sender, Receiver)> {
    listen_direct_cancellable(
        tracker,
        device_id,
        device_name,
        port,
        timeout,
        &tokio_util::sync::CancellationToken::new(),
    )
    .await
}

/// The same wait, but it can be called off.
///
/// The window starts and stops the share while the program runs, so
/// the wait for the other end has to be interruptible: a person who
/// closes the share must not wait out the whole timeout.
pub async fn listen_direct_cancellable(
    tracker: &str,
    device_id: &str,
    device_name: &str,
    port: u16,
    timeout: std::time::Duration,
    cancel: &tokio_util::sync::CancellationToken,
) -> Result<(Sender, Receiver)> {
    let socket = std::net::UdpSocket::bind((std::net::Ipv4Addr::UNSPECIFIED, port))
        .context("the port could not be opened")?;
    register(tracker, device_id, device_name, port).await?;
    let hole = crate::hole::punch_wait_until(socket, std::time::Instant::now() + timeout, cancel)
        .context("the other end did not arrive")?;

    let server_config = server_config()?;
    let endpoint = server_endpoint(hole.socket, server_config)?;
    let connection = accept_cancellable(&endpoint, timeout, cancel).await?;
    open(connection, Side::Responder).await
}

/// Tell the tracker which device this is and where it listens.
async fn register(tracker: &str, device_id: &str, device_name: &str, port: u16) -> Result<()> {
    let body = serde_json::json!({
        "id": device_id,
        "name": device_name,
        "port": port,
    });
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(10))
        .build()
        .context("the tracker could not be reached")?;
    let reply = client
        .post(format!("{tracker}/register"))
        .json(&body)
        .send()
        .await
        .context("the tracker could not be reached")?
        .json::<serde_json::Value>()
        .await
        .context("the tracker did not answer")?;
    if reply.get("ok").and_then(|v| v.as_bool()) != Some(true) {
        anyhow::bail!("the tracker did not accept the registration");
    }
    Ok(())
}

/// The TLS settings of the side that connects.
fn client_config() -> Result<quinn::ClientConfig> {
    let mut client_config =
        ClientConfig::builder_with_provider(Arc::new(rustls::crypto::ring::default_provider()))
            .with_protocol_versions(&[&rustls::version::TLS13])
            .context("TLS 1.3 is not available")?
            .dangerous()
            .with_custom_certificate_verifier(Arc::new(AcceptAnyCertificate))
            .with_no_client_auth();

    client_config.enable_early_data = true;

    let quic_config = quinn::crypto::rustls::QuicClientConfig::try_from(Arc::new(client_config))
        .context("the client configuration could not be used")?;
    let mut config = quinn::ClientConfig::new(Arc::new(quic_config));

    let mut transport = quinn::TransportConfig::default();
    transport.max_concurrent_uni_streams(0u32.into());
    transport.max_concurrent_bidi_streams(1u32.into());
    transport.max_idle_timeout(Some(
        std::time::Duration::from_secs(30)
            .try_into()
            .expect("thirty seconds fits"),
    ));
    config.transport_config(Arc::new(transport));
    Ok(config)
}

/// The TLS settings of the side that waits.
///
/// The same self-signed certificate as the ordinary listener uses, so
/// that a session looks the same to the other end no matter how it was
/// set up.
fn server_config() -> Result<quinn::ServerConfig> {
    let keys = self_signed_keys()?;
    let certificate = CertificateDer::from(keys.0);
    let private_key = PrivateKeyDer::Pkcs8(PrivatePkcs8KeyDer::from(keys.1));

    let mut server_config = rustls::ServerConfig::builder_with_provider(Arc::new(
        rustls::crypto::ring::default_provider(),
    ))
    .with_protocol_versions(&[&rustls::version::TLS13])
    .context("TLS 1.3 is not available")?
    .with_no_client_auth()
    .with_single_cert(vec![certificate], private_key)
    .context("the certificate could not be used")?;

    server_config.max_early_data_size = u32::MAX;

    let quic_config = quinn::crypto::rustls::QuicServerConfig::try_from(Arc::new(server_config))
        .context("the server configuration could not be used")?;
    let mut config = quinn::ServerConfig::with_crypto(Arc::new(quic_config));

    let mut transport = quinn::TransportConfig::default();
    transport.max_concurrent_uni_streams(0u32.into());
    transport.max_concurrent_bidi_streams(1u32.into());
    transport.max_idle_timeout(Some(
        std::time::Duration::from_secs(30)
            .try_into()
            .expect("thirty seconds fits"),
    ));
    transport.datagram_receive_buffer_size(Some(16 * 1024 * 1024));
    transport.datagram_send_buffer_size(16 * 1024 * 1024);
    transport.stream_receive_window(quinn::VarInt::from_u32(1024 * 1024));
    transport.send_window(16 * 1024 * 1024);
    config.transport_config(Arc::new(transport));
    Ok(config)
}

/// A QUIC endpoint on a socket that already exists.
fn client_endpoint(socket: std::net::UdpSocket) -> Result<Endpoint> {
    let wrapped = quinn::Runtime::wrap_udp_socket(&quinn::TokioRuntime, socket)
        .context("the socket could not be handed over")?;
    let mut endpoint = Endpoint::new_with_abstract_socket(
        quinn::EndpointConfig::default(),
        None,
        wrapped,
        Arc::new(quinn::TokioRuntime),
    )
    .context("the endpoint could not be built")?;
    endpoint.set_default_client_config(client_config()?);
    Ok(endpoint)
}

/// A QUIC endpoint that waits for the other end.
fn server_endpoint(socket: std::net::UdpSocket, config: quinn::ServerConfig) -> Result<Endpoint> {
    let wrapped = quinn::Runtime::wrap_udp_socket(&quinn::TokioRuntime, socket)
        .context("the socket could not be handed over")?;
    let endpoint = Endpoint::new_with_abstract_socket(
        quinn::EndpointConfig::default(),
        Some(config),
        wrapped,
        Arc::new(quinn::TokioRuntime),
    )
    .context("the endpoint could not be built")?;
    Ok(endpoint)
}

/// Wait for the other end to come in, with a deadline.
///
/// The deadline matters: without it a machine that waits for a
/// connection that will never come would wait forever, and the user
/// would be stuck with a window that does nothing.
pub async fn accept(endpoint: &Endpoint, timeout: std::time::Duration) -> Result<Connection> {
    accept_cancellable(
        endpoint,
        timeout,
        &tokio_util::sync::CancellationToken::new(),
    )
    .await
}

/// The same wait, but it can be called off.
///
/// The wait for the other end is the longest wait in the program, so
/// it is the one that has to answer to a stop.
pub async fn accept_cancellable(
    endpoint: &Endpoint,
    timeout: std::time::Duration,
    cancel: &tokio_util::sync::CancellationToken,
) -> Result<Connection> {
    let wait = async {
        let incoming = tokio::time::timeout(timeout, endpoint.accept())
            .await
            .context("nobody came in")?
            .context("the endpoint is closed")?;
        incoming
            .accept()
            .context("the connection could not be made")?
            .await
            .context("the connection could not be made")
    };
    tokio::select! {
        biased;
        _ = cancel.cancelled() => Err(anyhow!("the wait was called off")),
        result = wait => result,
    }
}

/// Open the two channels over a connection.
///
/// The control channel is the first stream; the video goes over
/// datagrams, which need no stream of their own.
pub async fn open(connection: Connection, side: Side) -> Result<(Sender, Receiver)> {
    // The keys of the two directions are kept apart.  The two ends of
    // a session use different keys to send and to receive, so the
    // sending half and the receiving half must not share a slot:
    // otherwise one would put its key over the other's.
    let control_out = CipherSlot::default();
    let video_out = CipherSlot::default();
    let control_in = CipherSlot::default();
    let video_in = CipherSlot::default();
    // One two-way stream carries the control messages in both
    // directions.  The side that starts the connection opens it and the
    // other side accepts it; both then write on that one stream, which
    // is what a two-way stream is for.  Opening a stream on both ends
    // would make two separate streams that never meet.
    let (control_send, control_recv) = match side {
        Side::Initiator => {
            let (mut send, recv) = connection.open_bi().await?;
            // QUIC does not tell the other end that a stream has been
            // opened until something is sent on it.  So an empty
            // message is sent here to say "the stream is ready";
            // without it the other end waits for a stream that it has
            // not been told about, and the two ends wait for each
            // other.
            send.write_all(&0u32.to_be_bytes()).await?;
            (send, recv)
        }
        Side::Responder => {
            let (send, mut recv) = connection.accept_bi().await?;
            // The empty message that says the stream is ready is taken
            // off here, so that the first message the caller reads is a
            // real one.
            let mut ready = [0u8; 4];
            recv.read_exact(&mut ready).await?;
            if u32::from_be_bytes(ready) != 0 {
                anyhow::bail!("the other end did not open the stream properly");
            }
            (send, recv)
        }
    };

    let (video_send, video_recv) = mpsc::unbounded_channel();

    // The datagrams arrive on the connection, so somebody has to read
    // them and put them into the channel.  The key is read at the
    // moment the datagram arrives, because the handshake puts it on
    // after the channel is open.
    let pump_connection = connection.clone();
    let pump_cipher = video_in.clone();
    tokio::spawn(async move {
        while let Ok(data) = pump_connection.read_datagram().await {
            let data = match pump_cipher.get() {
                Some(key) => match key.decrypt(&data) {
                    Ok(plain) => Bytes::from(plain),
                    // A datagram that cannot be read is thrown away:
                    // the picture it belongs to is incomplete anyway.
                    Err(_) => continue,
                },
                None => data,
            };
            if video_send.send(data).is_err() {
                break;
            }
        }
    });

    let sender = Sender {
        control: Arc::new(tokio::sync::Mutex::new(control_send)),
        connection: connection.clone(),
        control_cipher: control_out.clone(),
        video_cipher: video_out.clone(),
    };
    let receiver = Receiver {
        control: control_recv,
        video: video_recv,
        connection,
        control_cipher: control_in.clone(),
        video_cipher: video_in.clone(),
    };
    Ok((sender, receiver))
}

/// Read one message off the control stream.
///
/// The length comes first, so two messages that arrive together are
/// still two messages and not one long one.
async fn read_one_message(stream: &mut RecvStream, cipher: &CipherSlot) -> Result<Option<Bytes>> {
    let mut length_bytes = [0u8; 4];
    match stream.read_exact(&mut length_bytes).await {
        Ok(_) => {}
        Err(quinn::ReadExactError::FinishedEarly(0)) => return Ok(None),
        Err(err) => return Err(anyhow!("the control stream failed: {err}")),
    }
    let length = u32::from_be_bytes(length_bytes) as usize;
    if length == 0 {
        return Err(anyhow!("the message had no mark in it"));
    }

    let mut data = vec![0u8; length];
    stream
        .read_exact(&mut data)
        .await
        .map_err(|err| anyhow!("the control stream failed: {err}"))?;
    let flag = data[0];
    let data = data[1..].to_vec();

    // The flag says whether the message is encrypted.  A message that
    // says it is encrypted but arrives before the keys are known is
    // refused: something is wrong with the order of the handshake.
    let data = match flag {
        FLAG_PLAIN => Bytes::from(data),
        FLAG_ENCRYPTED => {
            let Some(key) = cipher.get() else {
                return Err(anyhow!("an encrypted message arrived before the keys"));
            };
            let plain = key
                .decrypt(&data)
                .map_err(|err| anyhow!("the message could not be read: {err}"))?;
            Bytes::from(plain)
        }
        other => return Err(anyhow!("the message had an unknown mark: {other}")),
    };
    Ok(Some(data))
}

/// Split a picture into chunks that fit into a datagram.
pub fn split_video(frame: &[u8]) -> Vec<Bytes> {
    if frame.len() <= CHUNK_SIZE {
        return vec![Bytes::copy_from_slice(frame)];
    }
    frame
        .chunks(CHUNK_SIZE)
        .map(Bytes::copy_from_slice)
        .collect()
}

/// Put chunks back together.
pub fn join_video(chunks: &[Bytes]) -> Bytes {
    let mut out = BytesMut::with_capacity(chunks.iter().map(|c| c.len()).sum());
    for chunk in chunks {
        out.extend_from_slice(chunk);
    }
    out.freeze()
}

/// Make a certificate and a key for the host.
///
/// The certificate is made fresh every time the host starts, because
/// it is not what makes the session safe.
fn self_signed_keys() -> Result<(Vec<u8>, Vec<u8>)> {
    let certified = rcgen::generate_simple_self_signed(vec!["skycremote".to_string()])
        .context("a self signed certificate could not be made")?;
    let certificate = certified.cert.der().to_vec();
    let private_key = certified.signing_key.serialize_der();
    Ok((certificate, private_key))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_small_picture_stays_in_one_chunk() {
        let frame = vec![7u8; 1000];
        let chunks = split_video(&frame);
        assert_eq!(chunks.len(), 1);
        assert_eq!(chunks[0].as_ref(), &frame[..]);
    }

    #[test]
    fn a_big_picture_is_split_and_comes_back_whole() {
        let frame: Vec<u8> = (0..100_000).map(|i| (i % 251) as u8).collect();
        let chunks = split_video(&frame);
        assert!(chunks.len() > 1, "a big picture must be split");
        for chunk in &chunks {
            assert!(chunk.len() <= CHUNK_SIZE);
        }
        let joined = join_video(&chunks);
        assert_eq!(joined.as_ref(), &frame[..]);
    }

    #[test]
    fn the_split_does_not_lose_a_single_byte() {
        // A frame that is exactly on the border is the case that breaks
        // an off-by-one.
        let frame = vec![1u8; CHUNK_SIZE * 2];
        let chunks = split_video(&frame);
        assert_eq!(chunks.len(), 2);
        assert_eq!(join_video(&chunks).len(), frame.len());
    }

    #[tokio::test]
    async fn two_ends_can_talk_over_the_channel() {
        let (server, server_address) = listen(0).await.expect("the port must open");
        let client_endpoint = connect(server_address).await.expect("the port must open");

        let accept_task = tokio::spawn(async move {
            accept(&server, std::time::Duration::from_secs(10))
                .await
                .expect("the connection must come in")
        });

        let client = client_endpoint
            .connect(server_address, "skycremote")
            .expect("the connection must start")
            .await
            .expect("the connection must be made");

        let server_connection = accept_task.await.expect("the task must finish");

        // The two ends open their side of the stream at the same time.
        // Doing one after the other would deadlock: the side that opens
        // waits for the other end to be ready, and the other end is
        // still waiting for its turn.
        let (client_side, server_side) = tokio::join!(
            open(client, Side::Initiator),
            open(server_connection, Side::Responder),
        );
        let (client_sender, mut client_receiver) =
            client_side.expect("the client channels must open");
        let (server_sender, mut server_receiver) =
            server_side.expect("the server channels must open");

        // A control message must arrive, in order, whole.
        client_sender
            .send_control(Bytes::from_static(b"first"))
            .await
            .expect("the message must go out");
        client_sender
            .send_control(Bytes::from_static(b"second"))
            .await
            .expect("the message must go out");

        match server_receiver.recv().await {
            Incoming::Control(data) => assert_eq!(data.as_ref(), b"first"),
            other => panic!("expected a control message, got {other:?}"),
        }
        match server_receiver.recv().await {
            Incoming::Control(data) => assert_eq!(data.as_ref(), b"second"),
            other => panic!("expected a control message, got {other:?}"),
        }

        // A picture must arrive as well.
        server_sender
            .send_video(Bytes::from_static(b"a picture"))
            .expect("the picture must go out");
        match client_receiver.recv().await {
            Incoming::Video(data) => assert_eq!(data.as_ref(), b"a picture"),
            other => panic!("expected a picture, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn raw_quinn_streams_work() {
        // The same thing without `open()`, straight through quinn.  It
        // is the control group: when `open()` stops working, this one
        // says whether the fault is in the setup or in `open()`.
        //
        // It also pins down a thing that is easy to forget: quinn does
        // not tell the other end that a stream has been opened until
        // something is sent on it.  That is why `open()` sends an empty
        // message first, and why this test writes before it reads.
        let (server, server_address) = listen(0).await.expect("the port must open");
        let client_endpoint = connect(server_address).await.expect("the port must open");

        let accept_task = tokio::spawn(async move {
            accept(&server, std::time::Duration::from_secs(10))
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
            async {
                let (mut s, mut r) = client.open_bi().await?;
                s.write_all(b"ping").await?;
                let mut buf = [0u8; 16];
                let n = r.read(&mut buf).await.unwrap_or(None).unwrap_or(0);
                Ok::<_, anyhow::Error>((n, String::from_utf8_lossy(&buf[..n]).to_string()))
            },
            async {
                let (mut s, mut r) = server_connection.accept_bi().await?;
                let mut buf = [0u8; 16];
                let n = r.read(&mut buf).await.unwrap_or(None).unwrap_or(0);
                s.write_all(b"pong").await?;
                Ok::<_, anyhow::Error>((n, String::from_utf8_lossy(&buf[..n]).to_string()))
            },
        );
        let (cn, ct) = client_side.expect("the client side must work");
        let (sn, st) = server_side.expect("the server side must work");
        assert_eq!(st, "ping", "the server must get what the client sent");
        assert_eq!(ct, "pong", "the client must get what the server sent");
        assert_eq!(cn, 4);
        assert_eq!(sn, 4);
    }

    #[tokio::test]
    async fn a_connection_that_never_comes_gives_up_instead_of_waiting_forever() {
        let (server, _) = listen(0).await.expect("the port must open");
        let result = accept(&server, std::time::Duration::from_millis(200)).await;
        assert!(result.is_err(), "the wait must end");
    }
}
