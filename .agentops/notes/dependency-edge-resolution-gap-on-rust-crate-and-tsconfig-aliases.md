---
title: "dependency-edge resolution gap on Rust crate:: and tsconfig aliases"
type: gotcha
---

The codebrain-foundation pass's dependency-edge resolver produced zero edges on real code in this repo: Rust's crate::-prefixed paths and Next.js tsconfig path aliases (@/...) weren't resolved to concrete file targets, so every intra-repo import silently failed to become a graph edge. Found via live scanning of this actual repo (both Rust and the Next.js apps/web frontend), not caught by unit tests using simple relative imports.
