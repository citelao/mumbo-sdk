# CONTRIBUTING

We'd love your help!

## Inner loop

```bash
# Build
cargo build

# Test
cargo test
```

### Testing against a real server

`tests/live_server.rs` checks behavior that depends on what a Mumble server
actually does. Those tests are ignored by default, since they need a server:

```bash
podman compose -f dev/mumble-server/compose.yaml up -d  # or `docker compose`
cargo test --test live_server -- --ignored
podman compose -f dev/mumble-server/compose.yaml down
```

To run it alongside another server on 64738 (e.g. Mumbo's), pick another port
for both: `MUMBLE_PORT=64739` before each command. `MUMBLE_SERVER` and
`MUMBLE_PASSWORD` point the tests at a different server.

## Submitting code

- Use [Conventional Commits](https://www.conventionalcommits.org/en/v1.0.0/), e.g. `fix(protocol): prevent Toasters from Toast()ing Bread that has Jam`
- Use the pull request template.