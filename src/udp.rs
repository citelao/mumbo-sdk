//! Voice over UDP, with automatic fallback to the TCP tunnel.
//!
//! Once the server sends keys in `CryptSetup`, we open a UDP socket and send
//! encrypted pings. Voice goes over UDP once the server answers one, and goes
//! back to the TCP tunnel if the answers stop (e.g. a firewall drops them).
//! This mirrors the official client's behavior.

use std::io;
use std::net::SocketAddr;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use prost::Message;
use tokio::net::UdpSocket;

use crate::crypt::{CryptState, CryptStats, HEADER_LEN};
use crate::{UdpMessageId, raw, raw_udp};

/// How often to ping while UDP works (the official client uses 5 s).
const PING_INTERVAL: Duration = Duration::from_secs(5);
/// How often to ping while waiting for UDP to start working, so voice moves
/// to UDP within about a second of connecting.
const PROBE_INTERVAL: Duration = Duration::from_secs(1);
/// With no ping reply for this long, voice falls back to the TCP tunnel.
const STALE_AFTER: Duration = Duration::from_secs(12);
/// With packets failing to decrypt for this long, ask the server to resync
/// nonces (and don't ask more often than this).
const RESYNC_AFTER: Duration = Duration::from_secs(5);

/// The largest datagram the Mumble protocol allows, encryption header
/// included (`MAX_UDP_PACKET_SIZE` in the official source).
pub(crate) const MAX_DATAGRAM: usize = 1024;
/// The largest message that fits in a datagram once encrypted.
const MAX_PLAIN: usize = MAX_DATAGRAM - HEADER_LEN;

/// Whether `audio` is within the protocol's packet size limit. The limit
/// applies even through the TCP tunnel, so the caller drops voice that is not.
pub(crate) fn fits_datagram(audio: &raw_udp::Audio) -> bool {
    // One more byte, for the message type, has to fit too.
    audio.encoded_len() < MAX_PLAIN
}

/// What a received datagram turned out to be.
#[derive(Debug)]
pub(crate) enum Datagram {
    /// Voice, for the app.
    Audio(raw_udp::Audio),
    /// Decryption keeps failing: send this `CryptSetup` over TCP to resync.
    RequestResync(raw::CryptSetup),
    /// Handled here (a ping reply), or dropped (undecryptable or malformed).
    Nothing,
}

pub(crate) struct UdpVoice {
    /// `None` when there is no UDP path to the server (in tests): keys are
    /// then ignored and voice stays on the TCP tunnel.
    server: Option<SocketAddr>,
    socket: Option<UdpSocket>,
    crypt: Option<CryptState>,
    /// Whether voice should go over UDP rather than the TCP tunnel. Shared
    /// with `MumbleClient`, so the app can show the transport.
    active: Arc<AtomicBool>,
    /// Voice fell back to the tunnel and the server has not been told yet.
    fell_back: bool,
    keyed_at: Option<Instant>,
    /// The last packet that decrypted; without one for a while, we resync.
    last_good: Option<Instant>,
    /// The last ping reply. Only these prove UDP works in both directions:
    /// voice arriving says nothing about whether ours gets through.
    last_pong: Option<Instant>,
    last_ping: Option<Instant>,
    last_resync_request: Option<Instant>,
    rtt: Option<Duration>,
    /// Ping timestamps are microseconds since this instant.
    epoch: Instant,
}

impl UdpVoice {
    pub fn new(server: Option<SocketAddr>, active: Arc<AtomicBool>) -> Self {
        Self {
            server,
            socket: None,
            crypt: None,
            active,
            fell_back: false,
            keyed_at: None,
            last_good: None,
            last_pong: None,
            last_ping: None,
            last_resync_request: None,
            rtt: None,
            epoch: Instant::now(),
        }
    }

    pub fn is_active(&self) -> bool {
        self.active.load(Ordering::Relaxed)
    }

    fn set_active(&mut self, active: bool) {
        // `fell_back` must survive further `set_active(false)` calls (e.g. a
        // failed send just after falling back) until `tick` consumes it, so
        // only reactivating clears it.
        let was_active = self.active.swap(active, Ordering::Relaxed);
        if active {
            self.fell_back = false;
        } else if was_active {
            self.fell_back = true;
        }
    }

    /// Microseconds since `epoch`, as carried in pings.
    fn timestamp(&self, now: Instant) -> u64 {
        u64::try_from((now - self.epoch).as_micros()).unwrap_or(u64::MAX)
    }

    pub fn stats(&self) -> CryptStats {
        self.crypt.as_ref().map(|c| c.stats).unwrap_or_default()
    }

    /// Round-trip time of the last UDP ping.
    pub fn rtt(&self) -> Option<Duration> {
        self.rtt
    }

    /// Receives one datagram, for [`Self::on_datagram`]. Waits forever while
    /// there is no socket (before keys arrive, or without a UDP path).
    pub async fn recv(&self, buf: &mut [u8]) -> io::Result<usize> {
        match &self.socket {
            Some(socket) => socket.recv(buf).await,
            None => std::future::pending().await,
        }
    }

    /// Handles the server's `CryptSetup`, returning a reply to send over TCP.
    pub async fn on_crypt_setup(
        &mut self,
        setup: &raw::CryptSetup,
        now: Instant,
    ) -> Option<raw::CryptSetup> {
        match (&setup.key, &setup.client_nonce, &setup.server_nonce) {
            // Full keys: (re)start encryption.
            (Some(key), Some(client_nonce), Some(server_nonce)) => {
                let server = self.server?;
                let Some(crypt) = CryptState::new(key, client_nonce, server_nonce) else {
                    log::warn!("Ignoring CryptSetup with malformed keys");
                    return None;
                };
                if self.socket.is_none() {
                    match open_socket(server).await {
                        Ok(socket) => self.socket = Some(socket),
                        Err(e) => {
                            log::warn!("UDP voice unavailable: {e}");
                            return None;
                        }
                    }
                }
                self.crypt = Some(crypt);
                self.set_active(false);
                self.keyed_at = Some(now);
                self.last_good = None;
                self.last_pong = None;
                self.send_ping(now).await;
                None
            }
            // Only the server's nonce: it is resyncing us.
            (None, None, Some(server_nonce)) => {
                if let Some(crypt) = &mut self.crypt
                    && !crypt.set_decrypt_iv(server_nonce)
                {
                    log::warn!("Ignoring CryptSetup with a malformed server nonce");
                }
                None
            }
            // Empty: the server wants our nonce.
            (None, None, None) => self.crypt.as_ref().map(|crypt| raw::CryptSetup {
                client_nonce: Some(crypt.encrypt_iv().to_vec()),
                ..Default::default()
            }),
            _ => {
                log::warn!("Ignoring CryptSetup with an unexpected combination of fields");
                None
            }
        }
    }

    /// Periodic upkeep; call it more often than the shorter of
    /// [`PING_INTERVAL`] and [`PROBE_INTERVAL`].
    ///
    /// Falls back to the TCP tunnel once no ping has been answered for
    /// [`STALE_AFTER`]. Sends a ping every [`PING_INTERVAL`] while voice is on
    /// UDP, or every [`PROBE_INTERVAL`] while it is not.
    ///
    /// Returns a ping to send through the TCP tunnel when voice has just
    /// fallen back. The server keeps sending us voice over UDP until something
    /// of ours arrives through the tunnel, and if we are not speaking, nothing
    /// else would.
    pub async fn tick(&mut self, now: Instant) -> Option<raw_udp::Ping> {
        // Nothing to do until the server has sent keys.
        self.crypt.as_ref()?;
        if self.is_active() && self.last_pong.is_none_or(|t| now - t > STALE_AFTER) {
            log::warn!("UDP pings go unanswered; falling back to the TCP tunnel");
            self.set_active(false);
        }
        let interval = if self.is_active() {
            PING_INTERVAL
        } else {
            PROBE_INTERVAL
        };
        if self.last_ping.is_none_or(|t| now - t >= interval) {
            self.send_ping(now).await;
        }
        std::mem::take(&mut self.fell_back).then(|| raw_udp::Ping {
            // Never zero: that would encode to nothing, and the server drops
            // tunnelled packets that have no body.
            timestamp: self.timestamp(now).max(1),
            ..Default::default()
        })
    }

    /// Sends voice over UDP if it is working. Returns `false` if the caller
    /// should send it through the TCP tunnel instead. `audio` must satisfy
    /// [`fits_datagram`].
    pub async fn send_audio(&mut self, audio: &raw_udp::Audio) -> bool {
        if !self.is_active() {
            return false;
        }
        self.send_message(audio).await
    }

    /// Decrypts and decodes one received datagram.
    pub fn on_datagram(&mut self, datagram: &[u8], now: Instant) -> Datagram {
        let Some(crypt) = &mut self.crypt else {
            return Datagram::Nothing;
        };
        if datagram.len() < HEADER_LEN || datagram.len() > MAX_DATAGRAM {
            return Datagram::Nothing;
        }

        let mut plain = [0u8; MAX_PLAIN];
        let plain = &mut plain[..datagram.len() - HEADER_LEN];
        if let Err(e) = crypt.decrypt(datagram, plain) {
            log::debug!("Dropping UDP packet: {e}");
            let since_good = self.last_good.or(self.keyed_at).map(|t| now - t);
            let since_request = self.last_resync_request.map(|t| now - t);
            if since_good.is_some_and(|d| d > RESYNC_AFTER)
                && since_request.is_none_or(|d| d > RESYNC_AFTER)
            {
                log::info!("Requesting a UDP nonce resync");
                self.last_resync_request = Some(now);
                return Datagram::RequestResync(raw::CryptSetup::default());
            }
            return Datagram::Nothing;
        }
        self.last_good = Some(now);

        let Some((&kind, body)) = plain.split_first() else {
            return Datagram::Nothing;
        };
        match kind {
            raw_udp::Audio::TYPE_ID => match raw_udp::Audio::decode(body) {
                Ok(audio) => Datagram::Audio(audio),
                Err(e) => {
                    log::debug!("Dropping malformed UDP voice packet: {e}");
                    Datagram::Nothing
                }
            },
            raw_udp::Ping::TYPE_ID => {
                if let Ok(ping) = raw_udp::Ping::decode(body) {
                    let sent = self.epoch + Duration::from_micros(ping.timestamp);
                    self.rtt = Some(now.saturating_duration_since(sent));
                    self.last_pong = Some(now);
                    if !self.is_active() {
                        log::info!("UDP voice is working");
                        self.set_active(true);
                    }
                }
                Datagram::Nothing
            }
            _ => {
                log::debug!("Dropping UDP packet of unknown type {kind}");
                Datagram::Nothing
            }
        }
    }

    async fn send_ping(&mut self, now: Instant) {
        self.last_ping = Some(now);
        let ping = raw_udp::Ping {
            timestamp: self.timestamp(now),
            ..Default::default()
        };
        self.send_message(&ping).await;
    }

    /// Encrypts and sends one message. Returns whether it was sent.
    async fn send_message<M: UdpMessageId>(&mut self, message: &M) -> bool {
        let (Some(crypt), Some(socket)) = (&mut self.crypt, &self.socket) else {
            return false;
        };
        // A packet is one byte for the message type, then the message.
        let len = 1 + message.encoded_len();
        if len > MAX_PLAIN {
            log::error!("Not sending a {len}-byte message over UDP: too large");
            return false;
        }
        let mut plain = [0u8; MAX_PLAIN];
        plain[0] = M::TYPE_ID;
        let mut body = &mut plain[1..len];
        message
            .encode(&mut body)
            .expect("the buffer holds `encoded_len` bytes");
        let mut sealed = [0u8; MAX_DATAGRAM];
        let sealed = &mut sealed[..len + HEADER_LEN];
        crypt.encrypt(&plain[..len], sealed);
        match socket.send(sealed).await {
            Ok(_) => true,
            Err(e) => {
                // E.g. ECONNREFUSED after an ICMP port-unreachable.
                log::debug!("UDP send failed: {e}");
                self.set_active(false);
                false
            }
        }
    }
}

/// Opens a socket that exchanges datagrams with `server` only.
async fn open_socket(server: SocketAddr) -> io::Result<UdpSocket> {
    let bind: SocketAddr = if server.is_ipv6() {
        "[::]:0".parse().unwrap()
    } else {
        "0.0.0.0:0".parse().unwrap()
    };
    let socket = UdpSocket::bind(bind).await?;
    socket.connect(server).await?;
    log::info!("UDP voice socket {} -> {}", socket.local_addr()?, server);
    Ok(socket)
}

/// The server's side of the encryption, for the tests here and in `client`.
#[cfg(test)]
pub(crate) mod testing {
    use super::*;

    pub const KEY: [u8; 16] = [7; 16];
    pub const CLIENT_NONCE: [u8; 16] = [1; 16];
    pub const SERVER_NONCE: [u8; 16] = [2; 16];

    pub struct ServerCrypt(CryptState);

    impl ServerCrypt {
        /// Keyed to match [`Self::setup`].
        pub fn new() -> Self {
            Self::keyed(&KEY, &SERVER_NONCE)
        }

        pub fn keyed(key: &[u8; 16], server_nonce: &[u8; 16]) -> Self {
            // The server encrypts with the nonce the client decrypts with.
            Self(CryptState::new(key, server_nonce, &CLIENT_NONCE).unwrap())
        }

        /// The keys a server sends the client during the handshake.
        pub fn setup() -> raw::CryptSetup {
            raw::CryptSetup {
                key: Some(KEY.to_vec()),
                client_nonce: Some(CLIENT_NONCE.to_vec()),
                server_nonce: Some(SERVER_NONCE.to_vec()),
            }
        }

        /// Encrypts a message of UDP type `kind` into a datagram.
        pub fn seal(&mut self, kind: u8, message: &impl Message) -> Vec<u8> {
            let mut plain = vec![kind];
            message.encode(&mut plain).unwrap();
            let mut sealed = vec![0; plain.len() + HEADER_LEN];
            self.0.encrypt(&plain, &mut sealed);
            sealed
        }

        /// Decrypts a datagram from the client: its UDP type and body.
        pub fn open(&mut self, datagram: &[u8]) -> (u8, Vec<u8>) {
            let mut plain = vec![0; datagram.len() - HEADER_LEN];
            self.0.decrypt(datagram, &mut plain).unwrap();
            (plain[0], plain[1..].to_vec())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::testing::{CLIENT_NONCE, KEY, SERVER_NONCE, ServerCrypt};
    use super::*;

    fn pong(server: &mut ServerCrypt) -> Vec<u8> {
        server.seal(raw_udp::Ping::TYPE_ID, &raw_udp::Ping::default())
    }

    async fn keyed(now: Instant) -> (UdpVoice, UdpSocket) {
        let server = UdpSocket::bind("127.0.0.1:0").await.unwrap();
        let mut udp = UdpVoice::new(Some(server.local_addr().unwrap()), Arc::default());
        let reply = udp.on_crypt_setup(&ServerCrypt::setup(), now).await;
        assert!(reply.is_none());
        (udp, server)
    }

    #[tokio::test]
    async fn keys_open_a_socket_and_ping_right_away() {
        let (_udp, server) = keyed(Instant::now()).await;

        let mut buf = [0; MAX_DATAGRAM];
        let n = server.recv(&mut buf).await.unwrap();
        let (kind, _) = ServerCrypt::new().open(&buf[..n]);
        assert_eq!(kind, raw_udp::Ping::TYPE_ID);
    }

    #[tokio::test]
    async fn keys_are_ignored_without_a_udp_path() {
        let now = Instant::now();
        let mut udp = UdpVoice::new(None, Arc::default());
        let reply = udp.on_crypt_setup(&ServerCrypt::setup(), now).await;
        assert!(reply.is_none());

        // Not keyed, so there is no nonce to answer a request with either.
        let request = raw::CryptSetup::default();
        assert!(udp.on_crypt_setup(&request, now).await.is_none());
        assert!(!udp.send_audio(&raw_udp::Audio::default()).await);
    }

    #[tokio::test]
    async fn voice_uses_udp_only_after_the_server_answers() {
        let now = Instant::now();
        let (mut udp, _server) = keyed(now).await;
        let audio = raw_udp::Audio::default();
        assert!(!udp.send_audio(&audio).await, "UDP is unproven");

        let reply = pong(&mut ServerCrypt::new());
        assert!(matches!(udp.on_datagram(&reply, now), Datagram::Nothing));
        assert!(udp.is_active());
        assert!(udp.send_audio(&audio).await);
    }

    #[tokio::test]
    async fn the_largest_voice_that_fits_fills_a_datagram_exactly() {
        let now = Instant::now();
        let (mut udp, server) = keyed(now).await;
        udp.on_datagram(&pong(&mut ServerCrypt::new()), now);
        let mut buf = [0; MAX_DATAGRAM + 1];
        server.recv(&mut buf).await.unwrap(); // the first ping

        // 3 bytes of protobuf framing for an `opus_data` this long.
        let mut audio = raw_udp::Audio {
            opus_data: vec![0; MAX_PLAIN - 1 - 3],
            ..Default::default()
        };
        assert!(fits_datagram(&audio));
        assert!(udp.send_audio(&audio).await);
        assert_eq!(server.recv(&mut buf).await.unwrap(), MAX_DATAGRAM);

        audio.opus_data.push(0);
        assert!(!fits_datagram(&audio));
    }

    #[tokio::test]
    async fn received_voice_is_decoded() {
        let now = Instant::now();
        let (mut udp, _server) = keyed(now).await;
        let audio = raw_udp::Audio {
            opus_data: vec![1, 2, 3],
            frame_number: 9,
            ..Default::default()
        };
        let packet = ServerCrypt::new().seal(raw_udp::Audio::TYPE_ID, &audio);

        let Datagram::Audio(got) = udp.on_datagram(&packet, now) else {
            panic!("expected voice");
        };
        assert_eq!(got, audio);
        assert_eq!(udp.stats().good, 1);
    }

    #[tokio::test]
    async fn falls_back_to_tcp_when_udp_goes_quiet() {
        let start = Instant::now();
        let (mut udp, _server) = keyed(start).await;
        udp.on_datagram(&pong(&mut ServerCrypt::new()), start);
        assert!(udp.is_active());

        assert!(udp.tick(start + STALE_AFTER).await.is_none());
        assert!(udp.is_active(), "not stale yet");
        let later = start + STALE_AFTER + Duration::from_millis(1);
        let notice = udp.tick(later).await;
        assert!(!udp.is_active());

        // The server is told once, through the tunnel, with a ping that has a body.
        assert!(!notice.expect("a tunnelled ping").encode_to_vec().is_empty());
        assert!(udp.tick(later).await.is_none());
    }

    /// Voice reaching us does not prove that ours reaches the server.
    #[tokio::test]
    async fn falls_back_when_pings_go_unanswered_even_if_voice_still_arrives() {
        let start = Instant::now();
        let (mut udp, _server) = keyed(start).await;
        let mut server = ServerCrypt::new();
        let voice = raw_udp::Audio::default();

        // Voice alone does not start UDP either; only a ping reply does.
        udp.on_datagram(&server.seal(raw_udp::Audio::TYPE_ID, &voice), start);
        assert!(!udp.is_active());
        udp.on_datagram(&pong(&mut server), start);
        assert!(udp.is_active());

        let later = start + STALE_AFTER + Duration::from_millis(1);
        let packet = server.seal(raw_udp::Audio::TYPE_ID, &voice);
        assert!(matches!(
            udp.on_datagram(&packet, later),
            Datagram::Audio(_)
        ));
        udp.tick(later).await;
        assert!(!udp.is_active());
    }

    #[tokio::test]
    async fn persistent_decrypt_failures_request_a_resync_rate_limited() {
        let start = Instant::now();
        let (mut udp, _server) = keyed(start).await;
        // Encrypted with the wrong key, so it never decrypts.
        let mut wrong = ServerCrypt::keyed(&[9; 16], &SERVER_NONCE);
        let mut bad = || pong(&mut wrong);

        assert!(
            matches!(udp.on_datagram(&bad(), start), Datagram::Nothing),
            "too soon"
        );
        let later = start + RESYNC_AFTER + Duration::from_millis(1);
        assert!(matches!(
            udp.on_datagram(&bad(), later),
            Datagram::RequestResync(_)
        ));
        assert!(
            matches!(udp.on_datagram(&bad(), later), Datagram::Nothing),
            "rate-limited"
        );
    }

    #[tokio::test]
    async fn empty_crypt_setup_is_answered_with_our_nonce() {
        let (mut udp, _server) = keyed(Instant::now()).await;
        let reply = udp
            .on_crypt_setup(&raw::CryptSetup::default(), Instant::now())
            .await
            .unwrap();
        // One packet (the first ping) has advanced our nonce by one.
        let mut expected = CLIENT_NONCE;
        expected[0] += 1;
        assert_eq!(reply.client_nonce.unwrap(), expected);
    }

    #[tokio::test]
    async fn server_nonce_alone_resyncs_decryption() {
        let now = Instant::now();
        let (mut udp, _server) = keyed(now).await;
        let new_nonce = [0x40; 16];
        let setup = raw::CryptSetup {
            server_nonce: Some(new_nonce.to_vec()),
            ..Default::default()
        };
        assert!(udp.on_crypt_setup(&setup, now).await.is_none());
        assert_eq!(udp.stats().resync, 1);

        let mut server = ServerCrypt::keyed(&KEY, &new_nonce);
        udp.on_datagram(&pong(&mut server), now);
        assert!(udp.is_active());
    }
}
