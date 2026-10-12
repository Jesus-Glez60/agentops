# agentops-cli

Thin installer for the [agentops](https://github.com/Jesus-Glez60/agentops) CLI. Running it via `npx` downloads the prebuilt `agentops` binary for your platform (macOS, Linux, or Windows; x64 or arm64) into `~/.agentops/bin` and runs it. There's no separate install step and no Rust toolchain required. Nothing runs at `npm install` time; the download happens on first invocation.

Connect a repo to a hosted AgentOps server (opens a browser to approve the device and mints a personal API key for you):

```sh
cd your-project
npx agentops-cli connect --remote https://your-agentops-server --agents claude,cursor
```

Or run fully local, with no server:

```sh
npx agentops-cli install    # scan the repo, generate AGENTS.md
npx agentops-cli connect    # register the local MCP server for your coding tool
```

Every argument is passed straight through to the real `agentops` binary. See the [main README](https://github.com/Jesus-Glez60/agentops#readme) for the full command reference.

Set `AGENTOPS_INSTALL_DIR` to change where the binary is cached (default `~/.agentops`), or `AGENTOPS_CLI_DOWNLOAD_BASE_URL` to point at a different release mirror.
