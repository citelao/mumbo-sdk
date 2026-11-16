# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

## [0.1.0] - 2026-11-16

Initial release, split out of [citelao/mumbo](https://github.com/citelao/mumbo).

### Added

- Mumble TCP codec and typed client/server message enums generated from the Mumble protobufs.
- Async client over TLS (tokio + rustls) handling the handshake, pings, and message routing.
- Voice packets tunnelled over TCP, with speaking-state tracking for others and yourself.
- Server state tracking (users, channels, self-mute, speaking) published as events.
- Channel chat messages and direct messages.
