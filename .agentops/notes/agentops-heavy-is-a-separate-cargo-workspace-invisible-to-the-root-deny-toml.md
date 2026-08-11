---
title: "agentops-heavy is a separate Cargo workspace, invisible to the root deny.toml"
type: gotcha
---

Assumed deny.toml's reqwest ban applied to agentops-heavy's crates too and planned a reqwest-to-ureq swap for agentops-github-app before checking. agentops-heavy/Cargo.toml declares its own [workspace] block with its own Cargo.lock and is not listed in the root Cargo.toml's members -- cargo deny check run from the repo root has zero visibility into it. agentops-heavy's own workspace.dependencies already pins reqwest with rustls-tls (not native-tls), so no swap was needed; main's agentops-github-app ported verbatim. The real gap this surfaced: agentops-heavy has no deny.toml of its own at all, so it currently has zero supply-chain policy enforcement -- worth adding before any real release, not blocking a single crate port.
