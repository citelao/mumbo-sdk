//! Integration tests against a real Mumble server.
//!
//! They need a server so they're `#[ignore]`d by default. Start one with
//! `podman compose -f dev/mumble-server/compose.yaml up -d` (or `docker compose`),
//! then run `cargo test --test live_server -- --ignored`.
//!
//! Tests run in parallel against the one server, all in its root channel, so
//! each must ignore what other tests' users do: e.g. match on its own users'
//! sessions and on unique message text.
//!
//! Environment vars:
//!
//! - `MUMBLE_SERVER` (default: `localhost`)
//! - `MUMBLE_PORT` (default `64738`)
//! - `MUMBLE_PASSWORD` (default: `test`)

use mumbo_sdk::ServerToClientMessage;
use mumbo_sdk::client::{Connection, ConnectionOptions, SelfMuteStatus, connect};
use mumbo_sdk::raw::permission_denied::DenyType;
use mumbo_sdk::state::{ConnectionStatus, MumbleState, TextMessage};
use std::ops::{Deref, DerefMut};
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::Duration;
use tokio::sync::broadcast::error::RecvError;
use tokio::time::timeout;

/// Long enough for a slow CI runner, short enough that a missing message
/// fails fast.
const TIMEOUT: Duration = Duration::from_secs(5);

/// A `Connection` that disconnects when dropped, even if the test panics.
struct TestConnection(Connection);

impl Deref for TestConnection {
    type Target = Connection;
    fn deref(&self) -> &Connection {
        &self.0
    }
}

impl DerefMut for TestConnection {
    fn deref_mut(&mut self) -> &mut Connection {
        &mut self.0
    }
}

impl Drop for TestConnection {
    fn drop(&mut self) {
        self.0.client.disconnect();
    }
}

/// Joins the server as a new user. `role` only makes the name readable in the
/// server's logs.
async fn join(role: &str) -> TestConnection {
    // Unique names: a duplicate name kicks the earlier session.
    static NEXT: AtomicU32 = AtomicU32::new(0);
    let username = format!(
        "sdk-test-{role}-{}-{}",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    );
    // Defaults match dev/mumble-server/compose.yaml.
    let options = ConnectionOptions {
        server: std::env::var("MUMBLE_SERVER").unwrap_or("localhost".into()),
        port: Some(
            std::env::var("MUMBLE_PORT")
                .map(|p| p.parse().expect("MUMBLE_PORT must be a port number"))
                .unwrap_or(64738),
        ),
        password: Some(std::env::var("MUMBLE_PASSWORD").unwrap_or("test".into())),
        username: Some(username.clone()),
    };
    TestConnection(
        connect(options)
            .await
            .unwrap_or_else(|e| panic!("{username} couldn't connect: {e}")),
    )
}

fn my_session(connection: &Connection) -> u32 {
    let state = connection.client.state();
    let state = state.lock().unwrap();
    state
        .current_user_session
        .expect("connected, so we have a session")
}

/// Waits until `check` finds what it's looking for in `connection`'s state,
/// re-checking each time the state changes.
async fn until<T>(
    connection: &mut Connection,
    waiting_for: &str,
    mut check: impl FnMut(&MumbleState) -> Option<T>,
) -> T {
    let state = connection.client.state();
    let found = timeout(TIMEOUT, async {
        loop {
            if let Some(found) = check(&state.lock().unwrap()) {
                return found;
            }
            connection
                .events
                .recv()
                .await
                .expect("the connection closed");
        }
    })
    .await;
    found.unwrap_or_else(|_| panic!("timed out waiting for {waiting_for}"))
}

/// Waits for a message from the server matching `check`, e.g. one the state
/// doesn't keep, like `PermissionDenied`.
async fn until_message<T>(
    connection: &mut Connection,
    waiting_for: &str,
    mut check: impl FnMut(ServerToClientMessage) -> Option<T>,
) -> T {
    let found = timeout(TIMEOUT, async {
        loop {
            match connection.debug.recv().await {
                Ok(message) => {
                    if let Some(found) = check(message) {
                        return found;
                    }
                }
                // Audio from other tests can fill the buffer; what we want is
                // still to come, since we only drop what's already arrived.
                Err(RecvError::Lagged(_)) => {}
                Err(RecvError::Closed) => panic!("the connection closed"),
            }
        }
    })
    .await;
    found.unwrap_or_else(|_| panic!("timed out waiting for {waiting_for}"))
}

/// Text no other test (or run) will send.
fn unique_text(label: &str) -> String {
    static NEXT: AtomicU32 = AtomicU32::new(0);
    format!(
        "{label} {}-{}",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    )
}

fn received(state: &MumbleState, from: u32, content: &str) -> bool {
    state.chat_messages().iter().any(|m| {
        matches!(m, TextMessage::ChatTextMessage(m) if m.actor == Some(from) && m.content == content)
    })
}

#[tokio::test]
#[ignore = "needs a live Mumble server; see the top of this file"]
async fn connecting_syncs_the_server_state() {
    let mut me = join("syncer").await;

    let state = me.client.state();
    {
        let state = state.lock().unwrap();
        assert_eq!(state.connection_status(), &ConnectionStatus::Connected);
        let user = state.current_user().expect("we're in the user list");
        assert!(user.name.starts_with("sdk-test-syncer-"), "{user:?}");
        assert!(user.is_me);
        assert!(
            state.channels.contains_key(&user.channel_id),
            "we're in a channel we know about: {state:?}"
        );
        assert!(state.channels.contains_key(&0), "the root channel exists");
    }

    // The server sends its config just after the handshake.
    until(&mut me, "the server's message limits", |s| {
        s.message_length.zip(s.image_message_length)
    })
    .await;
}

#[tokio::test]
#[ignore = "needs a live Mumble server; see the top of this file"]
async fn users_see_each_other_join_and_leave() {
    let mut stayer = join("stayer").await;
    let leaver = join("leaver").await;
    let leaver_session = my_session(&leaver);

    until(&mut stayer, "the other user to join", |s| {
        s.users.get(&leaver_session).map(|_| ())
    })
    .await;

    // Dropping disconnects.
    drop(leaver);

    until(&mut stayer, "the other user to leave", |s| {
        (!s.users.contains_key(&leaver_session)).then_some(())
    })
    .await;
}

#[tokio::test]
#[ignore = "needs a live Mumble server; see the top of this file"]
async fn channel_messages_reach_other_users() {
    let sender = join("sender").await;
    let mut listener = join("listener").await;
    let sender_session = my_session(&sender);

    let text = unique_text("hello, channel");
    sender
        .client
        .send_channel_message(text.clone(), None)
        .unwrap();
    until(&mut listener, "the channel message", |s| {
        received(s, sender_session, &text).then_some(())
    })
    .await;
}

#[tokio::test]
#[ignore = "needs a live Mumble server; see the top of this file"]
async fn direct_messages_reach_only_their_recipient() {
    let sender = join("dm-sender").await;
    let mut recipient = join("dm-recipient").await;
    let mut bystander = join("dm-bystander").await;
    let sender_session = my_session(&sender);

    let secret = unique_text("psst");
    sender
        .client
        .send_user_message(secret.clone(), vec![my_session(&recipient)])
        .unwrap();
    // The server relays one sender's messages in order, so once the bystander
    // has this later one, it would already have the DM if it was sent there.
    let after = unique_text("sent after the DM");
    sender
        .client
        .send_channel_message(after.clone(), None)
        .unwrap();

    until(&mut recipient, "the DM", |s| {
        received(s, sender_session, &secret).then_some(())
    })
    .await;
    until(&mut bystander, "the channel message", |s| {
        received(s, sender_session, &after).then_some(())
    })
    .await;
    let state = bystander.client.state();
    assert!(!received(&state.lock().unwrap(), sender_session, &secret));
}

#[tokio::test]
#[ignore = "needs a live Mumble server; see the top of this file"]
async fn self_mute_is_confirmed_and_seen_by_others() {
    let mut muter = join("muter").await;
    let mut listener = join("mute-listener").await;
    let muter_session = my_session(&muter);

    muter.client.set_self_mute(true).unwrap();
    assert_eq!(muter.client.self_mute_status(), SelfMuteStatus::Muting);
    until(&mut muter, "the server to confirm the mute", |s| {
        s.current_user()?.self_mute.then_some(())
    })
    .await;
    assert_eq!(muter.client.self_mute_status(), SelfMuteStatus::Muted);
    until(&mut listener, "the other user to see the mute", |s| {
        s.users.get(&muter_session)?.self_mute.then_some(())
    })
    .await;

    muter.client.set_self_mute(false).unwrap();
    assert_eq!(muter.client.self_mute_status(), SelfMuteStatus::Unmuting);
    until(&mut muter, "the server to confirm the unmute", |s| {
        (!s.current_user()?.self_mute).then_some(())
    })
    .await;
    assert_eq!(muter.client.self_mute_status(), SelfMuteStatus::Unmuted);
    until(&mut listener, "the other user to see the unmute", |s| {
        (!s.users.get(&muter_session)?.self_mute).then_some(())
    })
    .await;
}

/// Receives the next voice packet `sender` sent to `listener`.
async fn next_packet_from(listener: &mut Connection, sender: u32) -> mumbo_sdk::raw_udp::Audio {
    let packet = timeout(TIMEOUT, async {
        loop {
            let packet = listener.audio.recv().await.expect("the connection closed");
            if packet.sender_session == sender {
                return packet;
            }
        }
    })
    .await;
    packet.expect("timed out waiting for a voice packet")
}

/// The server relays Opus packets without decoding them, so any bytes will do.
fn fake_opus(i: u8) -> Vec<u8> {
    vec![i; 20]
}

#[tokio::test]
#[ignore = "needs a live Mumble server; see the top of this file"]
async fn voice_reaches_other_users_in_the_channel() {
    let talker = join("talker").await;
    let mut listener = join("voice-listener").await;
    let talker_session = my_session(&talker);

    for i in 0..3 {
        talker
            .client
            .send_audio(&fake_opus(i), i.into(), false)
            .unwrap();
    }
    for i in 0..3 {
        let packet = next_packet_from(&mut listener, talker_session).await;
        assert_eq!(packet.opus_data, fake_opus(i));
        assert_eq!(packet.frame_number, u64::from(i));
        assert!(!packet.is_terminator);
    }
    until(&mut listener, "the talker to be speaking", |s| {
        s.users.get(&talker_session)?.is_speaking.then_some(())
    })
    .await;

    talker.client.send_audio(&fake_opus(3), 3, true).unwrap();
    let last = next_packet_from(&mut listener, talker_session).await;
    assert!(last.is_terminator);
    until(&mut listener, "the talker to stop speaking", |s| {
        (!s.users.get(&talker_session)?.is_speaking).then_some(())
    })
    .await;
}

#[tokio::test]
#[ignore = "needs a live Mumble server; see the top of this file"]
async fn muted_users_send_no_voice() {
    let mut talker = join("muted-talker").await;
    let mut listener = join("muted-listener").await;
    let talker_session = my_session(&talker);

    talker.client.set_self_mute(true).unwrap();
    until(&mut talker, "the server to confirm the mute", |s| {
        s.current_user()?.self_mute.then_some(())
    })
    .await;
    talker.client.send_audio(&fake_opus(1), 0, false).unwrap();

    // Packets are relayed in order, so if the muted one had been sent, it
    // would arrive before this.
    talker.client.set_self_mute(false).unwrap();
    // The unmute travels over TCP and voice over UDP, so voice packets sent
    // just after the unmute might still be dropped by the server.
    until(&mut talker, "the server to confirm the unmute", |s| {
        (!s.current_user()?.self_mute).then_some(())
    })
    .await;
    talker.client.send_audio(&fake_opus(2), 2, false).unwrap();
    let next = next_packet_from(&mut listener, talker_session).await;
    assert_eq!(next.opus_data, fake_opus(2));
}

/// Murmur drops even the terminator packet of a muted user. `state.rs` relies
/// on this to decide when to stop showing a muted user as speaking.
#[tokio::test]
#[ignore = "needs a live Mumble server; see the top of this file"]
async fn muted_users_terminator_packets_are_dropped() {
    let mut talker = join("terminator-talker").await;
    let mut listener = join("terminator-listener").await;
    let talker_session = my_session(&talker);

    talker.client.send_audio(&fake_opus(1), 0, false).unwrap();
    let first = next_packet_from(&mut listener, talker_session).await;
    assert_eq!(first.opus_data, fake_opus(1));

    talker.client.set_self_mute(true).unwrap();
    until(&mut talker, "the server to confirm the mute", |s| {
        s.current_user()?.self_mute.then_some(())
    })
    .await;
    talker.client.send_audio(&fake_opus(2), 1, true).unwrap();

    // Packets are relayed in order, so if the terminator had been sent, it
    // would arrive before this.
    talker.client.set_self_mute(false).unwrap();
    until(&mut talker, "the server to confirm the unmute", |s| {
        (!s.current_user()?.self_mute).then_some(())
    })
    .await;
    talker.client.send_audio(&fake_opus(3), 2, false).unwrap();
    let next = next_packet_from(&mut listener, talker_session).await;
    assert_eq!(next.opus_data, fake_opus(3));
}

/// Murmur drops all voice from a muted user, including the packet that ends
/// their burst, so listeners must stop showing them as speaking anyway.
#[tokio::test]
#[ignore = "needs a live Mumble server; see the top of this file"]
async fn muting_mid_burst_stops_listeners_seeing_the_talker_speak() {
    let talker = join("mid-burst-talker").await;
    let mut listener = join("mid-burst-listener").await;
    let talker_session = my_session(&talker);

    talker.client.send_audio(&fake_opus(1), 0, false).unwrap();
    until(&mut listener, "the talker to be speaking", |s| {
        s.users.get(&talker_session)?.is_speaking.then_some(())
    })
    .await;

    talker.client.set_self_mute(true).unwrap();
    talker.client.send_audio(&fake_opus(2), 2, true).unwrap();
    until(&mut listener, "the talker to stop speaking", |s| {
        (!s.users.get(&talker_session)?.is_speaking).then_some(())
    })
    .await;
}

/// Murmur re-parses messages over `message_length` as XML, to measure them
/// without their images, and rejects them if that fails. See
/// `MumbleClient::send_channel_message`.
#[tokio::test]
#[ignore = "needs a live Mumble server; see the top of this file"]
async fn long_messages_must_be_well_formed_xml() {
    let mut sender = join("xml-sender").await;
    let mut listener = join("xml-listener").await;
    let sender_session = my_session(&sender);

    let limit = until(&mut sender, "the server's message limit", |s| {
        s.message_length
    })
    .await;
    // The server doesn't decode the image, so any base64 will do.
    let image = format!("data:image/png;base64,{}", "A".repeat(limit as usize));
    let label = unique_text("an image");

    let malformed = format!(r#"{label}<br><img src="{image}">"#);
    sender.client.send_channel_message(malformed, None).unwrap();
    until_message(
        &mut sender,
        "the malformed message to be denied",
        |m| match m {
            ServerToClientMessage::PermissionDenied(denied) => {
                (denied.r#type() == DenyType::TextTooLong).then_some(())
            }
            _ => None,
        },
    )
    .await;

    let well_formed = format!(r#"{label}<br /><img src="{image}" />"#);
    sender
        .client
        .send_channel_message(well_formed.clone(), None)
        .unwrap();
    until(&mut listener, "the well-formed message", |s| {
        received(s, sender_session, &well_formed).then_some(())
    })
    .await;
}
