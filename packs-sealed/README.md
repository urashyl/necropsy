# Sealed packs

Unreleased dailies, encrypted. Each `.sealed` file is one pack, sealed with `pack-tool seal` from a `packs-src/` directory that is never committed. `manifest.toml` lists them; `wrap-keys/` holds the public keys they are sealed to. Each pack's key is published at `/api/packs/<id>/key` once its date is over in New York, and opens that pack only.

See [docs/sealing.md](../docs/sealing.md).
