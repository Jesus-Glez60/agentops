Audit the whole codebase for over-engineering, ranked by potential line cuts.

STEPS:

1. Walk the source tree (skip node_modules, .git, dist, build, __pycache__, venv).

2. For each file, estimate lines that could be cut by applying the Ponytail ladder.

3. Output a ranked table:
   ```
   | File | Lines | Est. Cut | Top Finding |
   |------|-------|----------|-------------|
   | src/foo.ts | 320 | ~80 | yagni: AbstractFactoryInterface has 1 impl |
   ```

4. After the table, list the top 3 highest-impact fixes with specific line references.

5. End with:
   ```
   Total: ~N lines removable across M files.
   Run /ponytail-debt to see all tagged shortcuts.
   ```
