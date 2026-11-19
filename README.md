# mumbo-sdk

**mumbo-sdk** is a shared library for building Mumble-compatible clients in Rust, primarily used by [haydenmc/blurt](https://github.com/haydenmc/blurt) and [citelao/mumbo](https://github.com/citelao/mumbo).

In heavy development :).

## Usage

In your project's `Cargo.toml`:

```toml
[dependencies]
mumbo-sdk = { git = "https://github.com/citelao/mumbo-sdk", branch = "main" }
```

This will pin whatever's on `main`: update it with `cargo update -p mumbo-sdk`.

For using a local copy/overlay, in your project's `.cargo/config.toml` (you should `.gitignore` this!):

```toml
[patch."https://github.com/citelao/mumbo-sdk"]
mumbo-sdk = { path = "../path/to/mumbo-sdk" }
```

## Requirements

Needs a `tokio` runtime.

## Contributing

See [CONTRIBUTING.md](./CONTRIBUTING.md).

## License

mumbo-sdk is licensed under [AGPL-3.0-or-later](./LICENSE).

Parts of it are derived from [Mumble](https://github.com/mumble-voip/mumble) and remain under Mumble's BSD 3-Clause license, reproduced in [LICENSE-MUMBLE](./LICENSE-MUMBLE):

- `src/crypt.rs`, a port of Mumble's OCB2-AES128 crypt state and its tests.
- `src/proto/`, Mumble's protobuf definitions.

If you distribute an app built on mumbo-sdk in binary form, include that notice in your own third-party notices.