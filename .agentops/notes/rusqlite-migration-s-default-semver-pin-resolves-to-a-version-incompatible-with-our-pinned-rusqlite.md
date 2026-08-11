---
title: "rusqlite_migration's default semver pin resolves to a version incompatible with our pinned rusqlite"
type: gotcha
---

Added rusqlite_migration = "1" to the workspace, matching the crate's docs examples. It resolved to 1.0.1, which fails to compile: unresolved import rusqlite::NO_PARAMS, an item removed from newer rusqlite versions. Bumping to "1.3" resolved to 1.3.0, which compiles on its own but pulls in its own transitive rusqlite ^0.32.1 dependency requiring libsqlite3-sys ^0.30.1 -- a hard conflict with our workspace's rusqlite = "0.40" (requiring a newer libsqlite3-sys), since sqlite3 is a native 'links' library and cargo refuses to link two different versions in one binary. Only rusqlite_migration "2" (resolved 2.6.0) has a rusqlite dependency range wide enough to share our existing rusqlite 0.40 pin cleanly. Caught immediately via a real cargo build failure, not assumed -- worth remembering next time a small utility crate is added: check its own transitive version pin against workspace pins before assuming a bare major-version spec is safe, especially for anything with a native 'links' dependency like libsqlite3-sys.
