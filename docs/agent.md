# The Agent

The Agent is derisk's window for building software and changing the
desktop with an agent: `derisk-agent` (`crates/derisk-agent` in derisk),
installed on the PC image by `nixos/modules/agent.nix`. Each chat runs
[Claude Code](https://claude.com/claude-code) in the project's folder, and
the window shows what it does as it goes.

- **Projects, chats, routines and artifacts** are in the left column. A
  project holds instructions, folders and MCP servers, like a Claude
  project. A routine is a prompt that runs on a schedule, as a systemd
  timer of the person's own. Artifacts are the apps and files the agent
  made.
- **Live previews** are on the right. When asked for an app, the agent
  writes a Rust app that draws with egui and builds it, and the preview
  runs it beside the chat: sandboxed as WebAssembly (`wasm32-wasip2`, with
  stdio and nothing else) or natively when the app needs files, network or
  processes.
- **The desktop's own tools.** Every chat gets two MCP servers: the
  Agent's own (`derisk-agent mcp`: previews, `nix_build`, `pm_build`, the
  forge's pull requests) and derisk's (`derisk mcp`), which reads and
  arranges the desktop.
- **Built-in skills**: customizing the desktop by prompting, writing new
  skills, and deleting unused online accounts. The Agent hands them to
  Claude Code as a plugin (`--plugin-dir`) on every turn, so they never land
  in `~/.claude`, and a person's own skills work beside them.

derisk's command palette asks it: typing a request that matches nothing
offers "Ask the agent: …", which opens the Agent on a new chat with the
request typed in, or hands it to the window already open. Nothing is sent
until Send is pressed. The palette opens `derisk-agent:ask?prompt=…` links,
which the Agent's desktop entry claims.

## Where it ships

The Agent is on the PC image, which is the desktop, laptop and tablet image
alike. The Halium image ([Phones](halium.md)) goes without: its window is
three columns at least 900 logical pixels wide, which does not fit a screen
under 600 on its short side.

Claude Code itself is not in the image: nixpkgs marks it unfree. The Agent
runs `claude` from `PATH`, or the program `DERISK_AGENT_CLAUDE` names, and
says so in the chat when there is neither ([What is not done](not-done.md)).

## How it is built

`nixos/pkgs/derisk.nix` builds it as `derisk.agent`, a cargo build of its
own from the same `components/derisk` checkout and `cargoHash` as derisk.
It is separate because its previews link wasmtime with WASI, which derisk's
own wasmtime, the one running the palette's plugins, goes without on
purpose, and cargo would give both the same features in one build.

## Sonne, and why it left

The Agent was Sonne's, the project's fork of [Zed](https://zed.dev), which
the PC image shipped as its code editor and as what text and source files
opened in. Sonne added about half a gigabyte to `/usr`, whose update slot is
a fixed 8G, for an editor that the Text Editor and the Agent's own code view
cover. Its agent window, skills, project setup and previews moved into
derisk, and Sonne left the image; it is not coming back. derisk-gpui still
builds against Sonne's fork of GPUI as a library, which is a separate
matter.
