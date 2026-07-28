Harvest all `ponytail:` comment markers from the codebase into a debt ledger.

STEPS:

1. Search codebase for `ponytail:` in all source files:
   ```
   grep -rn "ponytail:" --include="*.ts" --include="*.py" --include="*.js" --include="*.go" .
   ```

2. For each match, output:
   ```
   | File:Line | Ceiling | Upgrade Path |
   |-----------|---------|--------------|
   ```
   Parse the comment as: `ponytail: <ceiling>, <upgrade path>`

3. Group by severity:
   - **Performance ceiling** — will matter at scale
   - **Quality ceiling** — technical debt
   - **Feature ceiling** — missing capability

4. End with:
   ```
   Total ponytail markers: N
   Highest priority: <top 3 by impact>
   ```
