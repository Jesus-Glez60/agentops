---
title: "get_symbol surfaces affecting gotchas and decisions directly"
type: decision
---

get_symbol (the MCP tool for pulling a single code section) originally returned only the symbol's own content, with zero visibility into Gotcha/Decision nodes connected to it via Affects edges — that attachment only ever surfaced in the separately-generated full docgen repo-map doc, not in the tool an agent would actually call while about to touch a specific symbol. Decided to extend get_symbol itself to look up incoming Affects edges and append 'Known gotchas affecting this symbol'/'Decisions affecting this symbol' sections, rather than requiring a full docgen run first — this is Codebrain-2's actual intended payoff (a gotcha resurfacing exactly when relevant), and get_symbol is the higher-traffic, more targeted tool for that moment. Verified live over the real MCP protocol: pulling PostgresGraphStore now returns the nested-Tokio-runtime gotcha attached, and pulling describe_backend returns the misleading-backend-output gotcha attached.
