# Teams plan (deferred: implement after the individual product is solid)

**Status: NOT scheduled.** Pushin is individuals-first for now. This is the design sketch so the teams
build can start cold later without re-deriving the architecture. It maps to the roadmap's Stage 4
(multiplayer). Do **not** start this until Stage 1–3 (reliability, ingestion, intelligence) have landed.

## Why teams is the monetization stage: and the tension
Every paid competitor (Motion, Reclaim, Akiflow) monetizes *teams*: shared availability, meeting polls,
round-robin booking, shared projects. It's the revenue stage. **But it's in tension with the local-first,
on-device, privacy-first wedge**, so the guiding rule is: **shared data is opt-in per entity, and personal
data never leaves the device without an explicit share.** Teams is an *additive, optional* layer, never a
cloud rewrite.

## What we can build on
- **`sync` (Iroh P2P mesh + changeset log, LWW)**: memory `device-sync`, migration 0015. Today it syncs
  **one user's own devices** (full-DB mirror, last-writer-wins), paired by invite, iroh pinned 0.90.
- **`booking` free-slots**: the scheduler already computes availability; the booking page already exposes
  slots to outsiders via a hardened local HTTP server.
- **`people` / CRM + `entity_index`**: identity + relationship scaffolding already exists.

## The core architectural gap (read before designing)
The current mesh is **single-user, full-mirror LWW**: it merges your whole DB across *your* devices. Teams
is **multi-user + permissioned**: you do NOT LWW-merge a teammate's entire calendar into your DB. Teams
therefore needs a **new sharing layer**, distinct from device-sync:
- **Identity per person** (not per device): extend pairing to cross-user identity + keys.
- **Scoped, permissioned changesets**: sync only explicitly shared entities (a shared project, a
  free-busy view), with an ACL, not the full DB.
- **Different conflict semantics**: LWW is fine for personal; shared tasks need ownership/assignment
  semantics (who's assigned, who can edit).
- **Relay for offline members**: Iroh relay already in use; a shared entity may need store-and-forward so
  an offline teammate still converges. Keep it content-encrypted.

## Phased build (smallest, most on-brand first)
- **T1: Shared free/busy (read-only availability exchange).** Publish *only* your free slots (times, no
  event details) to paired teammates over the mesh. Reuses booking free-slots + the mesh transport.
  Smallest, most privacy-preserving, immediately useful. **Start here.**
- **T2: Find-a-time / meeting poll.** Intersect several members' shared free-busy → propose common slots;
  confirm → each member's scheduler books it locally. Pure computation on T1's data.
- **T3: Shared projects + task assignment.** The real multi-user step: identity + ACL + scoped sync of a
  shared project's tasks; assign to a person; ownership-based conflict resolution. Migration for
  share/ACL tables.
- **T4: Team booking page (round-robin).** Extend the public booking page to a team offering: route an
  external booking to whichever member is free. Builds on booking_server + T1 free-busy.
- **T5: Permissioned shared calendar / presence.** Opt-in view of a teammate's events (per-entity
  permission), light presence. The richest and most privacy-sensitive, so it comes last.

## Cross-cutting concerns
- **Auth/identity:** today is invite-pairing for one user's devices; teams needs verifiable cross-user
  identity. Design the key model up front (T1) even though T1 only needs read-only sharing.
- **Privacy invariant:** default-private; every share is explicit and revocable; encrypted in transit and
  at any relay. This invariant is the product's whole reason to exist. Never regress it for convenience.
- **Monetization:** teams = the paid tier. Keep the individual product fully functional without it.
- **Mobile intersection:** the `sync/infer` inference-bridge (a modelless phone borrowing a peer's model)
  overlaps this transport and has an open idle-unload gap (Stage 0 note); reconcile when the mobile track
  and teams both exist.

## Explicitly out of scope for the first teams cut
Real-time collaborative editing of the same note, org-wide admin/SSO, and a central server. If any of
those become required, revisit whether they can be done P2P before adding cloud infrastructure.
