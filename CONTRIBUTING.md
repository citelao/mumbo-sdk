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

We include a compose file for simple local testing:

```bash
# Start the server
podman compose -f dev/mumble-server/compose.yaml up -d  # or `docker compose`

# Connect to `localhost:64738` with password `test`

# Logs
podman compose -f dev/mumble-server/compose.yaml logs -f

# Stop the server
podman compose -f dev/mumble-server/compose.yaml down
```

When running the real server, we also have some integration tests:

```bash
# Run integration tests against a live server
cargo test --test live_server -- --ignored
```

To use custom servers, ports, or passwords, use `MUMBLE_SERVER`, `MUMBLE_PORT`, and `MUMBLE_PASSWORD`:

```bash
MUMBLE_PORT=65000 MUMBLE_PASSWORD=foo podman compose -f dev/mumble-server/compose.yaml up -d
MUMBLE_PORT=65000 MUMBLE_PASSWORD=foo cargo test --test live_server -- --ignored
```

## Submitting code

- Use [Conventional Commits](https://www.conventionalcommits.org/en/v1.0.0/), e.g. `fix(protocol): prevent Toasters from Toast()ing Bread that has Jam`
- Use the pull request template.