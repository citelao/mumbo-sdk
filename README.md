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

## Development

```bash
# Build
cargo build

# Test
cargo test
```