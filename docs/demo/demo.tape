# Records docs/demo/demo.gif with VHS (https://github.com/charmbracelet/vhs).
# Run from the repository root:
#   (cd examples/hono-todo && npm ci)
#   vhs docs/demo/demo.tape
# Needs `ajiro-api` on PATH (cargo install --path . or brew install niti-lab/tap/ajiro-api),
# curl, and port 3000 free.

Output docs/demo/demo.gif
Require ajiro-api
Require npm
Require curl

Set Shell "bash"
Set FontSize 15
Set Width 1100
Set Height 640
Set Padding 20
Set TypingSpeed 40ms
Set Theme "Catppuccin Mocha"

# Copy the example (with its tests) to a scratch dir and start the server.
# node_modules is linked, not copied: copying it is slow on network/WSL drives.
Hide
Type `export REPO=$PWD D=$(mktemp -d) && cp -r examples/hono-todo/{src,api,ajiro-api.yaml,package.json,tsconfig.json} $D/ && ln -s $REPO/examples/hono-todo/node_modules $D/node_modules && cd $D && echo DEMO_PASSWORD=demo-password > .env` Enter
Wait
Type `stop() { local p=$(ss -ltnpH 'sport = :3000' | grep -o 'pid=[0-9]*' | head -1 | cut -d= -f2); [ -n "$p" ] && kill $p; sleep 0.5; }` Enter
Wait
Type `start() { (npm start >/dev/null 2>&1 &); until curl -s localhost:3000/health >/dev/null; do sleep 0.3; done; }` Enter
Wait
Type "start && clear" Enter
Wait@30s
Show

# 1. Routes -> one YAML file per endpoint, tests pass.
Type "ajiro-api scan" Sleep 300ms Enter
Wait
Sleep 2s
Type "ajiro-api test | tail -3" Sleep 300ms Enter
Wait
Sleep 3s

# 2. "The agent tidies up the routes": /stats now comes after /:id.
Hide
Type `sed -i '25,26d' src/routes/todos.ts && echo 'todoRoutes.get("/stats", (c) => c.json(todoStats()));' >> src/routes/todos.ts` Enter
Wait
Type "stop && start && clear" Enter
Wait@30s
Show
Type "# the agent moved GET /todos/stats below GET /todos/:id ..." Enter
Wait
Sleep 500ms
Type "ajiro-api scan && ajiro-api test | grep -v '^✓'" Sleep 300ms Enter
Wait
Sleep 7s

# 3. Fix and re-run.
Hide
Type "cp $REPO/examples/hono-todo/src/routes/todos.ts src/routes/todos.ts && stop && start && clear" Enter
Wait@30s
Show
Type "# ...reads the failure, restores the route order, re-runs" Enter
Wait
Sleep 500ms
Type "ajiro-api scan && ajiro-api test | tail -2" Sleep 300ms Enter
Wait
Sleep 4s

Hide
Type "stop; cd $REPO && rm -rf $D" Enter
Wait
