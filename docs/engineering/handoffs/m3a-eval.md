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

## Notes on the baseline

- **m2-2 hits here, though milestone 2 recorded a miss.** The target is in slot 8 of 8, tied on two matched words with the items above it, so it sits on the edge of the cut (**unverified** why it moved since 2026-10-19). The spec expected the equivalence rule to settle this case. It doesn't: the cosine between decision 54022 and target 104430 is **0.868**, below 0.88 (`GET /repos/{id}/nodes/104430/similarity?to=54022`).
- **Digest sections take 3 of 8 slots in all 20 cases** (F53). Item 2 removes them.
- **c18's seed varied across repeats.** Slot 8 alternated between two `SimilarTo` neighbours of the same note, because `recall_seed` scores a neighbour from whichever source its `HashMap` walk reaches first. Recorded as a gotcha. Item 3's "keep the higher score" rule fixes it. The case's hit (rank 3) didn't change.
- Latency p95 141 ms matches milestone 2's 148 ms. Same path, same server.
