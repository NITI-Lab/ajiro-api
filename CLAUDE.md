# CLAUDE.md

See [AGENTS.md](AGENTS.md) for architecture, commands and conventions.

Quick reminders:

- Run `cargo fmt --all`, `cargo clippy --all-targets -- -D warnings` and `cargo test` before committing.
- `src/report.rs` is a public JSON contract. Do not rename or remove fields.
- `scan` only rewrites the top-level `source:` block; it must never touch `tests`, `mock` or comments.
- Keep `schema/*.schema.json`, `docs/yaml-reference.md`, `skills/ajiro-api/reference.md` and `docs/agents/AGENTS.snippet.md` in sync with `src/model.rs`.
- Requirements (Japanese): `docs/requirements.md`.

@AGENTS.md
