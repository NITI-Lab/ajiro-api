# Records docs/demo/demo.gif with VHS (https://github.com/charmbracelet/vhs).
# Run from the repository root:
#   (cd examples/hono-todo && npm ci)
#   vhs docs/demo/demo.tape
# Needs `ajiro-api` on PATH (cargo install --path . or brew install niti-lab/tap/ajiro-api)
# and port 3000 free.

Output docs/demo/demo.gif
Require ajiro-api
Require npm

Set Shell "bash"
Set FontSize 15
Set Width 1100
Set Height 640
Set Padding 20
Set TypingSpeed 40ms
Set Theme "Catppuccin Mocha"

# Copy the example (with its tests) to a scratch dir and start the server.
Hide
Type "export REPO=$PWD D=$(mktemp -d) && cp -r examples/hono-todo/. $D && cd $D && echo DEMO_PASSWORD=demo-password > .env" Enter
Type "(npm start >/dev/null 2>&1 &) && sleep 3 && export PS1='$ ' && clear" Enter
Sleep 4s
Show

# 1. Routes -> one YAML file per endpoint, tests pass.
Type "ajiro-api scan" Sleep 300ms Enter
Sleep 2s
Type "ajiro-api test | tail -4" Sleep 300ms Enter
Sleep 3s

# 2. "The agent tidies up the routes": /stats now comes after /:id.
Hide
Type "sed -i '25,26d' src/routes/todos.ts && echo 'todoRoutes.get(\"/stats\", (c) => c.json(todoStats()));' >> src/routes/todos.ts" Enter
Type "pkill -f 'tsx src/index.ts'; (npm start >/dev/null 2>&1 &) && sleep 3 && clear" Enter
Sleep 4s
Show
Type "# the agent moved GET /todos/stats below GET /todos/:id ..." Enter
Sleep 1s
Type "ajiro-api scan && ajiro-api test" Sleep 300ms Enter
Sleep 6s

# 3. Fix and re-run.
Hide
Type "cp $REPO/examples/hono-todo/src/routes/todos.ts src/routes/todos.ts" Enter
Type "pkill -f 'tsx src/index.ts'; (npm start >/dev/null 2>&1 &) && sleep 3 && clear" Enter
Sleep 4s
Show
Type "# ...reads the failure, restores the route order, re-runs" Enter
Sleep 1s
Type "ajiro-api scan && ajiro-api test | tail -2" Sleep 300ms Enter
Sleep 4s

Hide
Type "pkill -f 'tsx src/index.ts'; cd $REPO && rm -rf $D" Enter
