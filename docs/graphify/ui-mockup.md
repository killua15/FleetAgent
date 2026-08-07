# Graph groups — interaction design

Companion to `GRAPHIFY_TRD.md` §5.8. Built with the real tokens from
`src/styles/tokens.css` and the 22 bridges measured by `spike.sh` against
Hangar + `hangar-mobile` — no invented figures.

**Interactive mockup**: https://claude.ai/code/artifact/7abcf25a-1122-4695-9715-d988c24f5847

## Why Hangar draws its own graph

graphify's `graph.html` pulls `vis-network` from `unpkg.com`. It renders blank
under the app's CSP, needs the network, and ships one dark theme that is not
ours. Its full view stays available as a secondary action that opens the file in
the system browser, labelled as the external tool it is.

## The seam map

Two rails, one repo each; the files that carry the contract; one curve per
bridge. A force graph of 4 480 nodes answers no question a group actually has —
this answers the only one: *where do these two repos touch, and which file sits
on each side?*

Hubs fall out of the drawing: `models.rs` and `pull_request.dart` each carry six
bridges, so they are where a careless edit breaks the other side.

## Surfaces

| Surface | Shows |
| --- | --- |
| Settings → Graph groups | Groups, members, and bridges — the three-column skeleton of `SharedMemorySettings.tsx` |
| AgentPanel → Graph tab | The seam map, filtered to files the session touched |
| Create-subsession dialog | Collision warning naming the shared type (Phase 4) |
| The grid | Nothing — an index and its age are not a coarse signal |

## Two details that carry weight

**Freshness shows per member, not only per group.** §5.5 makes a group worth its
worst member, but a warning that only says "stale" doesn't say what to rebuild.

**The bridge count always shows, zero included.** A group that bought nothing has
to be visible in the list, not discovered by opening the graph.
