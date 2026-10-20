# Milestone 3a recall evaluation

Scores from `agentops recall-eval --remote --repeat 5` (`agentops-cli/src/recall_eval.rs`, see [05-retrieval.md](../05-retrieval.md#measuring-recall)) against the hosted graph. These are numbers only. The 20 cases live in `.context/recall-eval.jsonl`, which is git-ignored because prompts can reveal private memories, so no prompt or memory text appears here.

- **Cases:**
  - m2-1 to m2-5 are milestone 2's five prompts (`m2-report.md` §5).
  - c06 to c20 were drafted from the graph and approved by the author on 2026-10-19. Seven are gotchas, four decisions and four notes. Nine of the fifteen share no distinctive word with their memory.
  - All cases run in plan mode, seed limit 8.
- **A hit** means a target, or an equivalent memory (cosine ≥ `EQUIVALENT_MIN_SCORE` 0.88 with a target), appears in the seed the hook injected.
- **Latency** is the hook round trip from the author's machine to the server, over 100 runs.

## Scores

| Point | Commit | Hit rate | m2 cases | MRR | p50 / p95 ms | Digest in seed |
| --- | --- | --- | --- | --- | --- | --- |
| Baseline | `bc0d370` | 10/20 (50%), 10 exact, 0 equivalent | 3/5 | 0.239 | 105 / 141 | 20/20 |
| Re-baseline with deep ranks (0b) | `e5e165f` | 9/20 (45%), 9 exact, 0 equivalent | 2/5 | 0.233 | 95 / 124 | 20/20 |
| 1. Retitled imports (46 renamed on the server) | `e976a03` | 11/20 (55%), 11 exact, 0 equivalent | 3/5 | 0.245 | 99 / 128 | 20/20 |
| 2. No doc sections in seeds | `6399325` | 13/20 (65%), 13 exact, 0 equivalent | 3/5 | 0.415 | 68 / 99 | 0/20 |
| 3a. Hop inherits full cue count | `9db16a7` | 9/20 (45%), 9 exact, 0 equivalent | – | 0.377 | 74 / 116 | 0/20 |
| 3b. + cap of 2 per source | `0fd3880` | 11/20 (55%), 11 exact, 0 equivalent | – | 0.399 | 60 / 84 | 0/20 |
| 3c. + one cue fewer | `4699cdb` | 11/20 (55%), 11 exact, 0 equivalent | – | 0.396 | 62 / 93 | 0/20 |
| 3. Reverted to one cue (kept: cosine score, cap, determinism) | `c68d349` | 13/20 (65%), 13 exact, 0 equivalent | – | 0.414 | 65 / 85 | 0/20 |
| 4. Recall by meaning on the server | `1aab7ff` | 14/20 (70%), 14 exact, 0 equivalent | 4/5 | 0.446 | 102 / 127 | 0/20 |
| 5. Threshold 0.82 + prune (run on the server) | `197d892` | 15/20 (75%), 15 exact, 0 equivalent | 4/5 | 0.472 | 101 / 125 | 0/20 |

Target: at least 16/20, at least 4/5 m2 cases, and p95 within 148 + 150 ms.

## Baseline per case

| Case | Rank | Hit | Digest sections in seed |
| --- | --- | --- | --- |
| m2-1 | 1 | exact | 3 |
| m2-2 | 8 | exact | 3 |
| m2-3 | – | miss | 3 |
| m2-4 | 2 | exact | 3 |
| m2-5 | – | miss | 3 |
| c06 | – | miss | 3 |
| c07 | – | miss | 3 |
| c08 | 8 | exact | 3 |
| c09 | – | miss | 3 |
| c10 | 5 | exact | 3 |
| c11 | – | miss | 3 |
| c12 | – | miss | 3 |
| c13 | – | miss | 3 |
| c14 | – | miss | 3 |
| c15 | 4 | exact | 3 |
| c16 | 1 | exact | 3 |
| c17 | 4 | exact | 3 |
| c18 | 3 | exact | 3 |
| c19 | 1 | exact | 3 |
| c20 | – | miss | 3 |

## Re-baseline per case (0b)

Retrieval is unchanged from the baseline. The graph has gained notes since then, which moved m2-2 from rank 8 to rank 9, just past the cut. **Misses:** 8 noise, 3 vocabulary, 0 ranking. "Deep" is the target's rank in the 50-deep ranking (see [05-retrieval.md](../05-retrieval.md#measuring-recall)).

| Case | Rank | Hit | Deep | Miss |
| --- | --- | --- | --- | --- |
| m2-1 | 1 | exact | 1 |  |
| m2-2 | – | miss | 9 | noise |
| m2-3 | – | miss | – | vocabulary |
| m2-4 | 2 | exact | 2 |  |
| m2-5 | – | miss | – | vocabulary |
| c06 | – | miss | 16 | noise |
| c07 | – | miss | 21 | noise |
| c08 | 8 | exact | 8 |  |
| c09 | – | miss | 13 | noise |
| c10 | 5 | exact | 5 |  |
| c11 | – | miss | 26 | noise |
| c12 | – | miss | 13 | noise |
| c13 | – | miss | 9 | noise |
| c14 | – | miss | – | vocabulary |
| c15 | 4 | exact | 4 |  |
| c16 | 1 | exact | 1 |  |
| c17 | 4 | exact | 4 |  |
| c18 | 3 | exact | 3 |  |
| c19 | 1 | exact | 1 |  |
| c20 | – | miss | 10 | noise |

## Item 5 notes

The server ran `agentops-server link-similar <repo> --prune` on 2026-10-20 over 365 memories:
- 709 pairs below 0.82 were pruned.
- 94 floor pairs (at or above 0.75) were kept or added.
- The follow-up link pass created 72 edges.
- Memories with no edge of any kind: 0 of 365 (0.0%) before and 0 (0.0%) after, against the 5% limit.
- Candidate-pair cosine distribution: 17 at 0.90–0.95, 115 at 0.85–0.90, 640 at 0.80–0.85, 1,296 at 0.75–0.80.
- The dense timeout fired 0 times in 100 runs.

## Item 4 notes

The dense timeout fired 0 times in 100 runs. That count comes from the PM2 error log inside the app container, because the server's output doesn't reach `docker logs`. m2-5, a vocabulary miss in every earlier run, is now found by meaning at rank 2. c12 dropped out (deep rank 16), and m2-5 and c11 came in.

The measurement was delayed by an outage. The server's disk filled with the CI cargo cache (`agentops-ci-target`, 54.5 GB), and Postgres crash-looped from about 05:22 to 15:57 UTC on 2026-10-20. Hook writes in that window were lost. The fix was removing the cache volume, approved by the author.

## Item 3: what was tried

The spec has a `SimilarTo` neighbour inherit its source's strength. Two forms were measured, and both regressed against item 2's 13/20:
- **Full cue count (3a):** neighbours of one strongly matched hub filled 5 to 7 of the 8 slots.
- **Cap of 2 per source, then one cue fewer (3b, 3c):** these cut the flood, but neighbours of a 3-cue match still pushed out 2-cue direct hits that were the answer.

With the author's approval, the shipped form (3) counts a neighbour as one cue again, ranked after direct matches with as many. It keeps the cosine-scaled score, the cap and the deterministic order. The "m2 cases" column is filled only where it was counted.

## Notes on the baseline

- **m2-2 hits here, though milestone 2 recorded a miss.** The target is in slot 8 of 8, tied on two matched words with the items above it, so it sits on the edge of the cut (**unverified** why it moved since 2026-10-19). The spec expected the equivalence rule to settle this case. It doesn't: the cosine between decision 54022 and target 104430 is **0.868**, below 0.88 (`GET /repos/{id}/nodes/104430/similarity?to=54022`).
- **Digest sections take 3 of 8 slots in all 20 cases** (F53). Item 2 removes them.
- **c18's seed varied across repeats.** Slot 8 alternated between two `SimilarTo` neighbours of the same note, because `recall_seed` scores a neighbour from whichever source its `HashMap` walk reaches first. Recorded as a gotcha. Item 3's "keep the higher score" rule fixes it. The case's hit (rank 3) didn't change.
- Latency p95 141 ms matches milestone 2's 148 ms. Same path, same server.
