# agentops-cli

Thin installer for the [agentops](https://github.com/Jesus-Glez60/agentops) CLI. Running this via `npx` downloads the prebuilt `agentops` binary for your platform (macOS, Linux, or Windows; x64 or arm64) into `~/.agentops/bin` and runs it — no separate install step, no Rust toolchain required.

```sh
npx agentops-cli connect --remote https://your-agentops-server --api-key "$AGENTOPS_API_KEY" --agents claude,cursor
```

Every argument is passed straight through to the real `agentops` binary — see the [main README](https://github.com/Jesus-Glez60/agentops#readme) for the full command reference.

Set `AGENTOPS_INSTALL_DIR` to change where the binary is cached (default `~/.agentops`), or `AGENTOPS_CLI_DOWNLOAD_BASE_URL` to point at a different release mirror.
