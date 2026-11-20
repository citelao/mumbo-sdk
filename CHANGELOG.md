# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

## [0.2.0](https://github.com/citelao/mumbo-sdk/compare/v0.1.0...v0.2.0) - 2026-11-20

### Added

- *(connection)* send and receive voice over encrypted UDP, falling back to TCP ([#3](https://github.com/citelao/mumbo-sdk/pull/3))

### Fixed

- *(state)* stop showing muted users as speaking; test against a real server ([#8](https://github.com/citelao/mumbo-sdk/pull/8))
- *(connection)* log and report unknown message types instead of failing ([#2](https://github.com/citelao/mumbo-sdk/pull/2))

### Other

- *(license)* Update license to allow App Store publishing ([#16](https://github.com/citelao/mumbo-sdk/pull/16))
- add devcontainer for containerized local dev ([#9](https://github.com/citelao/mumbo-sdk/pull/9))
- contributing

## [0.1.0] - 2026-11-16

Initial release, split out of [citelao/mumbo](https://github.com/citelao/mumbo).

### Added

- Mumble TCP codec and typed client/server message enums generated from the Mumble protobufs.
- Async client over TLS (tokio + rustls) handling the handshake, pings, and message routing.
- Voice packets tunnelled over TCP, with speaking-state tracking for others and yourself.
- Server state tracking (users, channels, self-mute, speaking) published as events.
- Channel chat messages and direct messages.
