# Diagnostic examples

These examples analyze local RealmHound `.rhcap` files. Captures and generated
output may contain player names, timestamps, object identifiers, raw packet
payloads, chat, and account or session-related data.

Review and redact captures and output before sharing them publicly.

## `combat_replay`

Replays a capture through the live reassembler, parser and `CombatTracker`, and
prints each reconstructed fight with the damage attributed to every participant:

```
cargo run -p realmhound-core --example combat_replay -- <path-to.rhcap> [mastery] [challenge]
```

The two optional multipliers are what the game applies to the player's own
rolled weapon shots: the Exaltation Mastery factor (`1 + mastery% / 100`) and the
seasonal challenge factor (the Crucible's damage multiplier times the Active
Blood Ritual tier's). Live, the app resolves both from the character cache and
pushes them into the engine; an offline replay has no cache, so pass them here.

A fight that is solo (no other player ever seen) must come out at ~100% of the
boss HP lost. Anything lower means the engine is not modelling one of them --
that is exactly how the missing mastery/Blood Ritual multipliers were found, so
check this whenever damage attribution changes.
