## Summary

<!-- What does this change and why? -->

## Checklist

- [ ] `cargo fmt --all`, `cargo clippy --all-targets -- -D warnings` and `cargo test` pass
- [ ] `--json` output (`src/report.rs`) is backward compatible (fields added, none renamed or removed)
- [ ] `scan` still rewrites only the `source:` block
- [ ] Docs updated if `src/model.rs` or `src/cli.rs` changed (`schema/`, `docs/`, `skills/ajiro-api/reference.md`, `docs/agents/AGENTS.snippet.md`, READMEs)
- [ ] `CHANGELOG.md` updated
