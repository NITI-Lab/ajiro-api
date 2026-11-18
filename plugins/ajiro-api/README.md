# ajiro-api plugin (Claude plugin directory bundle)

This folder is the Claude plugin directory submission of [ajiro-api](https://github.com/NITI-Lab/ajiro-api). It contains only the `ajiro-api` skill; the skill runs the CLI through `npx`.

For the skill plus the MCP server, add this repository as a marketplace instead:

```
/plugin marketplace add niti-lab/ajiro-api
/plugin install ajiro-api@ajiro-api
```

Do not edit the files here by hand. `skills/` and `.claude-plugin/icon.png` are copies of the ones at the repository root, and CI fails if they drift.
