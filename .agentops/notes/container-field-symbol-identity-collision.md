---
title: "container field symbol-identity collision"
type: gotcha
---

During the codebrain-foundation pass, using a symbol's 'container' field alone as part of its identity key caused a silent data-loss collision: two distinct symbols with the same name in different containers (e.g. two methods both named 'new' on different structs) overwrote each other on rescan instead of being tracked as separate nodes. Found via live testing against this repo's own Rust code, not caught by any unit test.
