//! Core data model for the Combat History engine.
//!
//! These types describe a reconstructed boss fight and each participant's
//! contribution. A single client cannot observe every player's damage, so each
//! figure carries a [`DamageProvenance`] tag - we never present unavailable data
//! as a factual zero.

/// How a participant's damage figure was derived.
///
/// A single client only receives `DamagePacket`s for attackers within its render
/// range, and never for its own hits (those flow through the outgoing
/// `EnemyHit`/`PlayerShoot` path). This tag keeps the UI honest about accuracy.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DamageProvenance {
    /// Summed from incoming `DamagePacket` values (server-computed, post-defense)
    /// for another player visible to the local client. Accurate while visible.
    Observed,
    /// Like [`Observed`], but the local player joined the fight after it had
    /// already begun (the boss was below full HP when first seen), so ally
    /// damage dealt before the local client arrived was never received. A lower
    /// bound - the player's real damage was higher.
    PartiallyObserved,
    /// The local player, with every hit resolved to an exact client-computed
    /// damage value (shot simulation). Deterministic and complete.
    SelfComputed,
    /// The local player, but at least one hit could not be resolved to a damage
    /// value (a missed shot correlation). Damage is a lower bound; hit count is
    /// complete.
    SelfPartial,
    /// The local player. Own damage could not be computed at all for this fight
    /// (RNG unseeded mid-map or a weapon asset was unavailable). Hit count only.
    SelfPending,
    /// The local player, in an effectively-solo fully-observed kill. The boss's
    /// HP loss that no packet could attribute (server-authoritative ability /
    /// summon projectiles) is credited here, so the total matches the boss HP
    /// exactly. Only used when the local player was the sole attacker.
    SelfReconciled,
    /// The local player, in an effectively-solo fight, reconciled up to the
    /// server's own echo of the local player's direct hits. The server re-sends
    /// the local player's hits as damage packets with the owner masked, giving an
    /// exact direct-damage total that the shot simulation only approximates. Used
    /// when that masked total exceeds the self-computed figure (including escaped
    /// fights the boss survived, where the HP-gap reconciliation cannot apply).
    SelfServerReconciled,
    /// The local player, where some hits could not be resolved to a damage value
    /// and were estimated from the average of the resolved hits (rather than left
    /// as an unattributed lower bound). An approximation, not an exact total.
    SelfEstimated,
    /// Attacker id could not be resolved to a named player (out of range, minion,
    /// or an environmental/sentinel source).
    Unresolved,
}

impl DamageProvenance {
    /// Stable string form for persistence.
    pub fn as_str(&self) -> &'static str {
        match self {
            DamageProvenance::Observed => "observed",
            DamageProvenance::PartiallyObserved => "partially_observed",
            DamageProvenance::SelfComputed => "self_computed",
            DamageProvenance::SelfPartial => "self_partial",
            DamageProvenance::SelfPending => "self_pending",
            DamageProvenance::SelfReconciled => "self_reconciled",
            DamageProvenance::SelfServerReconciled => "self_server_reconciled",
            DamageProvenance::SelfEstimated => "self_estimated",
            DamageProvenance::Unresolved => "unresolved",
        }
    }

    /// Parse from the persisted string form.
    pub fn from_str(s: &str) -> Self {
        match s {
            "self_computed" => DamageProvenance::SelfComputed,
            "self_partial" => DamageProvenance::SelfPartial,
            "self_pending" => DamageProvenance::SelfPending,
            "self_reconciled" => DamageProvenance::SelfReconciled,
            "self_server_reconciled" => DamageProvenance::SelfServerReconciled,
            "self_estimated" => DamageProvenance::SelfEstimated,
            "unresolved" => DamageProvenance::Unresolved,
            "partially_observed" => DamageProvenance::PartiallyObserved,
            _ => DamageProvenance::Observed,
        }
    }

    /// Whether this figure describes the local player (any self variant).
    pub fn is_self(&self) -> bool {
        matches!(
            self,
            DamageProvenance::SelfComputed
                | DamageProvenance::SelfPartial
                | DamageProvenance::SelfPending
                | DamageProvenance::SelfReconciled
                | DamageProvenance::SelfServerReconciled
                | DamageProvenance::SelfEstimated
        )
    }
}

/// How a participant's *damage-taken* figure was derived.
///
/// Damage taken is only observable for players within render range: a remote
/// player's incoming damage is read exactly from `DamagePacket`s, while the
/// local player's own damage taken is not in `DamagePacket`s and is
/// reconstructed by correlating `PlayerHit` to the enemy projectile's damage.
/// This tag keeps the UI honest about accuracy (the value is `None` when
/// unavailable, never a factual zero).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DamageTakenProvenance {
    /// Summed from incoming `DamagePacket` values (exact, post-defense) for a
    /// remote player visible to the local client.
    Observed,
    /// The local player, reconstructed from the enemy projectile's damage and
    /// the local defense stat. Approximate.
    Estimated,
    /// The local player, but at least one incoming hit could not be valued
    /// (uncorrelated bullet or unknown defense); the figure is a lower bound.
    Partial,
}

impl DamageTakenProvenance {
    /// Stable string form for persistence.
    pub fn as_str(&self) -> &'static str {
        match self {
            DamageTakenProvenance::Observed => "observed",
            DamageTakenProvenance::Estimated => "estimated",
            DamageTakenProvenance::Partial => "partial",
        }
    }

    /// Parse from the persisted string form.
    pub fn from_str(s: &str) -> Self {
        match s {
            "estimated" => DamageTakenProvenance::Estimated,
            "partial" => DamageTakenProvenance::Partial,
            _ => DamageTakenProvenance::Observed,
        }
    }
}

/// Whether a participant was still present when the boss died, or had left the
/// fight (died or escaped the map). Resolved at fight finalize by correlating a
/// departed player's last position with any gravestone that spawned there.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ParticipantEndStatus {
    /// Present when the boss died (also the default for legacy rows recorded
    /// before this was tracked -- no icon is shown either way).
    Present,
    /// Left the fight and a gravestone was correlated to their death location.
    /// Carries the gravestone object type so the UI can draw the tier sprite.
    Died { grave_type: i32 },
    /// Left the fight with no gravestone seen: nexused (or walked out of range).
    Nexused,
}

impl ParticipantEndStatus {
    /// Stable string form for persistence (the grave type persists separately).
    pub fn as_str(&self) -> &'static str {
        match self {
            ParticipantEndStatus::Present => "present",
            ParticipantEndStatus::Died { .. } => "died",
            ParticipantEndStatus::Nexused => "nexused",
        }
    }

    /// Parse from the persisted string form + grave type column.
    pub fn from_str(s: &str, grave_type: i32) -> Self {
        match s {
            "died" => ParticipantEndStatus::Died { grave_type },
            "nexused" => ParticipantEndStatus::Nexused,
            _ => ParticipantEndStatus::Present,
        }
    }

    /// The gravestone object type when this participant died, else 0.
    pub fn grave_type(&self) -> i32 {
        match self {
            ParticipantEndStatus::Died { grave_type } => *grave_type,
            _ => 0,
        }
    }
}

/// A player's active pet, associated to its owner by position proximity. Only captured for newly recorded fights; historical rows have none.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PetInfo {
    /// Pet sprite / skin id (StatType::PetType), renderable via the sprite atlas.
    pub pet_type: i32,
    /// Ability type ids (StatType::Pet*AbilityType, 404-412) in slot order;
    /// zero/empty slots omitted.
    pub abilities: Vec<i32>,
}

/// A single participant's accumulated contribution to a fight.
#[derive(Debug, Clone)]
pub struct FightParticipant {
    /// Player object id in the fight's map instance.
    pub object_id: i32,
    /// Object type (class id, or skin id for skinned players).
    pub object_type: i32,
    /// Skin id (StatType::SkinId); 0 when the player uses their default class
    /// sprite. `object_type` remains the class id for grouping.
    pub skin_id: i32,
    /// Clothing dye/cloth texture (StatType::Texture1); 0 when none.
    pub tex1: u32,
    /// Accessory dye/cloth texture (StatType::Texture2); 0 when none.
    pub tex2: u32,
    /// Resolved player name (empty when unresolved).
    pub name: String,
    /// Equipment snapshot: weapon, ability, armor, ring (Inventory slots 0-3).
    pub equipment: [i32; 4],
    /// Per-slot enchant ids matching `equipment` (weapon, ability, armor, ring),
    /// decoded from the `UniqueDataString` stat; empty when none/unknown.
    pub equipment_enchants: [Vec<u16>; 4],
    /// Total damage attributed to this participant (0 when only `SelfPending`).
    pub damage: i64,
    /// Number of hits attributed to this participant.
    pub hits: u64,
    /// Whether this participant is the local player.
    pub is_local: bool,
    /// Provenance of the damage figure.
    pub provenance: DamageProvenance,
    /// Whether the participant was present at the boss's death, or left (died /
    /// nexused) before it.
    pub end_status: ParticipantEndStatus,
    /// The player's associated pet, when one was matched by
    /// position proximity. Always `None` for fights recorded before pet capture.
    pub pet: Option<PetInfo>,
    /// Total damage this participant took from the fight's enemies (post-defense),
    /// or `None` when no incoming damage was observed/estimable. Rendered as "-"
    /// when `None`; never presented as a factual zero.
    pub damage_taken: Option<i64>,
    /// How `damage_taken` was derived (meaningful only when `damage_taken` is
    /// `Some`).
    pub damage_taken_provenance: DamageTakenProvenance,
    /// Number of incoming hits this participant took ("hits taken", the local
    /// player's "shots tanked"). Recorded for every row from the build that added
    /// it -- a zero included, so "never hit" is distinguishable from the `None` of
    /// rows recorded before that.
    pub damage_taken_hits: Option<u64>,
    /// Damage negated by the local player's defense and other post-defense
    /// mitigation (the "Mell stat"): the difference between the raw
    /// incoming projectile damage and the amount actually taken. Only
    /// reconstructable for the local player, so `None` for remote players (whose
    /// `DamagePacket`s are already post-mitigation) and for fights recorded
    /// before this was tracked. Rendered as "-" when `None`.
    pub damage_blocked: Option<i64>,
    /// Damage this participant dealt to Oryx 3 while he was Guarded (a subset of
    /// `damage`), or `None` for fights recorded before this was tracked. Only
    /// meaningful for O3 fights; `Some(0)` for other bosses.
    pub guarded_damage: Option<i64>,
    /// Hits landed on Oryx 3 while he was Guarded (a subset of `hits`), or `None`
    /// for fights recorded before this was tracked.
    pub guarded_hits: Option<u64>,
    /// Summon/pet-sourced part of `damage` / `hits`. The totals above include it
    /// (the game attributes pet damage to the owning player), so the UI can show
    /// the full figure on the owner's row and the pet's own line beneath it.
    /// Zero for fights recorded before this was tracked.
    pub summon_damage: i64,
    pub summon_hits: u64,
    /// Per-summon/pet breakdown, one entry per entity that landed a hit.
    pub summons: Vec<SummonContribution>,
    /// The local player's own damage split per weapon (one row per weapon, with
    /// the enchants it was fired with), highest damage first. Empty for remote
    /// players, who are observed as whole damage packets and have no shot-level
    /// detail. Always empty for fights recorded before this was tracked.
    pub weapons: Vec<WeaponContribution>,
    /// Damage the server itself attributed to this player in a local damage packet
    /// -- an ability, DoT or proc figure the client did not compute. Only ever
    /// non-zero for the local player, and kept out of `weapons` so a packet figure
    /// is never presented as a weapon's contribution.
    pub packet_damage: i64,
    /// Number of local damage packets behind `packet_damage`.
    pub packet_hits: u64,
}

/// One summon or pet that dealt damage, credited to the player who owns it.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SummonContribution {
    /// Object id of the firing summon / pet entity.
    pub object_id: i32,
    /// Pet sprite / skin id (`StatType::PetType`); 0 for item and ability summons.
    pub pet_type: i32,
    /// True when the entity carried a pet stat, i.e. a pet rather than a summon.
    pub is_pet: bool,
    /// The entity's own object type, so the row can be named and given a sprite
    /// (`Summon Pocket Rat` is `0x4683`, say). 0 when it was never observed in the
    /// object table, and in fights recorded before this was stored.
    pub object_type: i32,
    /// The item that created this summon, when the local player's own loadout and
    /// activation log identify it: the mace in the ability slot, the Kimono in the
    /// armor slot. A creature's catalog entry lists *every* item that can spawn it
    /// -- `Summon Bear` names both `Sovereign Mace` and its `(SB)` twin -- so this
    /// is what names the one row source instead of "from 2 items". 0 when nothing
    /// settles it, and in fights recorded before it was stored.
    pub source_item: i32,
    /// The enchants that item had when it spawned the creature, so two uses of the
    /// same item with different enchants stay separate rows.
    pub source_enchants: Vec<u16>,
    pub damage: i64,
    pub hits: u64,
}

/// One weapon's share of the local player's damage, summed across every loadout
/// that used that weapon during the fight so a mid-fight armor or ring swap does
/// not split one weapon's contribution.
///
/// Only the local player has this: their damage is reconstructed shot by shot
/// from outgoing shoot/hit packets (tagged `Self-computed` in the UI), whereas
/// other players arrive as whole damage packets with no weapon attribution.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WeaponContribution {
    /// Weapon item id (equipment slot 0).
    pub weapon_id: i32,
    /// The weapon's enchant ids at the time of those hits (empty when none).
    pub enchants: Vec<u16>,
    pub damage: i64,
    pub hits: u64,
}

/// A window during which one of the local player's stats or conditions differed
/// from its previous value, e.g. an Attack boost from a consumable or a Damaging
/// condition from an ally's buff.
///
/// Derived purely from the server stat stream the tracker already receives, so
/// it captures every boost the client was told about -- including ones with no
/// local cause. The `source` is a best-effort inference and is labelled as
/// inferred in the UI.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BoostWindow {
    /// Window start, in milliseconds since the fight started.
    pub start_offset_ms: i64,
    /// Window end (the transition back), or the fight's end while still active.
    pub end_offset_ms: i64,
    /// Stat or condition name ("Attack", "Damaging", ...).
    pub stat: String,
    /// Value before the transition.
    pub from: i32,
    /// Value during the window.
    pub to: i32,
    /// Best-effort source label: the item that caused the change (verified against
    /// the item's own declared effects), `<enchant> (<item> proc)` for an
    /// equipment proc, or `Ally buff` / `Enemy debuff` when nothing local explains
    /// it. The wire carries no source id, so this is always an inference.
    pub source: String,
    /// Object id of the item the source names, when one was resolved, so a UI can
    /// draw exactly that item rather than re-resolving its name.
    pub source_item: Option<i32>,
}

/// One activation of the local player's ability-slot item.
///
/// Ability damage is mostly resolved client-side (poison ticks, novas, lightning
/// and so on never reach the wire), so this records the activation and credits it
/// the local damage that *was* packet-borne while its window was open. Abilities
/// whose damage the client resolves locally therefore show activations but no
/// damage, rather than silently disappearing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AbilityActivation {
    /// Ability item id (equipment slot 1).
    pub item_id: i32,
    /// The ability's enchant ids at the time (empty when none).
    pub enchants: Vec<u16>,
    /// Activation time, milliseconds since the fight started.
    pub start_offset_ms: i64,
    /// How long the activation was assumed to be dealing damage (ms), capped so
    /// overlapping activations cannot double-count a packet.
    pub window_ms: i64,
    /// Packet-borne damage credited to this activation.
    pub damage: i64,
    /// Number of packet-borne damage events credited to this activation.
    pub hits: u64,
}

/// A completed boss fight ready to persist and display.
#[derive(Debug, Clone)]
pub struct CompletedFight {
    /// Fight start (epoch millis) - first damage/hit observed on the boss.
    pub started_at: i64,
    /// Fight end (epoch millis) - boss death, map exit, or timeout.
    pub ended_at: i64,
    /// Dungeon / map name the fight took place in.
    pub dungeon: String,
    /// When the local player entered the current dungeon instance (epoch millis),
    /// captured on the map change that loaded it. `None` outside a groupable
    /// dungeon (realm/nexus/hubs) or for fights recorded before this was tracked.
    /// Drives the encounter card's "Total dungeon time" (entry -> final boss death).
    pub dungeon_entered_at: Option<i64>,
    /// Map seed of the fight instance.
    pub map_seed: i32,
    /// Boss object type (resolves to a name/sprite via assets).
    pub boss_object_type: i32,
    /// Resolved boss name.
    pub boss_name: String,
    /// Boss maximum HP.
    pub boss_max_hp: i32,
    /// Boss HP when the fight was first observed (may be < max if seen mid-fight).
    pub boss_start_hp: i32,
    /// Local player's object id in this fight (0 if unknown).
    pub local_object_id: i32,
    /// Local player's character id (char_id) for this fight (0 if unknown).
    ///
    /// Object ids are ephemeral per map instance; this stable char id lets the UI
    /// filter a specific character's fights. In RotMG all of an account's
    /// characters share one IGN, so name alone cannot distinguish them.
    pub local_char_id: i32,
    /// Whether the boss was observed dying.
    pub killed: bool,
    /// Whether the boss's HP was observed reaching zero (a true full-HP-removal
    /// kill, as opposed to a low-HP despawn or completion marker). Persisted so a
    /// future migration can safely reconcile solo HP gaps on stored fights.
    pub reached_zero: bool,
    /// Whether the local player joined after the fight had already begun (boss
    /// below full HP on first sight). Persisted alongside `reached_zero` as solo
    /// reconciliation evidence.
    pub joined_late: bool,
    /// Curated encounter id when this fight is one phase of a multi-phase
    /// encounter (see `assets::encounter_for_boss_type`); `None` for standalone
    /// fights, which behave exactly as before.
    pub encounter_id: Option<String>,
    /// Opaque per-map-instance run id, minted on map change. All member phases of
    /// one encounter run share this id so the UI can group them; never derived
    /// from the RNG map seed. `None` for standalone fights.
    pub encounter_run_id: Option<String>,
    /// Number of "close calls" the local player had during this fight: each
    /// time their HP dropped below 20% of max (deaths included). Summed across
    /// a run's phases for display. 0 for fights recorded before this was tracked.
    pub local_close_calls: i32,
    /// For an aggregated aux summary fight (e.g. Spectral Key, Overseer
    /// Eyesmall), the number of distinct member instances seen in the world this
    /// run. `None` for ordinary boss fights. Drives the "xN" count on the row.
    pub aux_member_count: Option<i32>,
    /// Moonlight Village spirits collected during this fight (0 for every other
    /// boss, and for fights recorded before spirit tracking). Each spirit is one
    /// `MV Total Counter` object released at the end of a dance/Umi phase; the
    /// run total drives the dungeon's loot tier.
    pub spirits: i32,
    /// Total time the boss was vulnerable and taking damage, in ms. `0` means
    /// unknown (records written before this was tracked), so consumers fall back
    /// to the whole fight duration.
    pub boss_uptime_ms: i64,
    /// Whether the run was played in Moonlight Village's Leisurely Mode (the
    /// group consumed a Tofu Delicacy), which shortens the phases and reduces
    /// the loot. False for every other dungeon.
    pub leisurely: bool,
    /// Whether the local player took Moonlight Village's Challenge Mode (they
    /// attacked the Challenge Gate, which pet-stasises them for the run). False
    /// for every other dungeon.
    pub petless: bool,
    /// Whether this Spectral Penitentiary fight was hard mode: a mini-boss is
    /// hard mode when it shouted its objective-cleared taunt, and Soulwarden
    /// Murcian when both of the run's mini-bosses were. The dungeon is hard mode
    /// exactly when Murcian's row is; false for every other dungeon.
    pub spectral_hm: bool,
    /// Whether this Shatters fight happened in hard mode, i.e. the group
    /// destroyed the Stone Idol (or The Source) that unlocks the renamed
    /// bosses. Only meaningful for The Shatters' main bosses; false otherwise.
    pub shatters_hm: bool,
    /// The local player's stat/condition boost windows during this fight,
    /// in time order. Empty for fights recorded before this was tracked.
    pub boosts: Vec<BoostWindow>,
    /// The local player's ability activations during this fight, in time order,
    /// each with whatever packet-borne damage it could be credited. Empty for
    /// fights recorded before this was tracked.
    pub abilities: Vec<AbilityActivation>,
    /// Per-participant contributions, sorted by damage descending on finalize.
    pub participants: Vec<FightParticipant>,
}

impl CompletedFight {
    /// Fight duration in milliseconds (never negative).
    pub fn duration_ms(&self) -> i64 {
        (self.ended_at - self.started_at).max(0)
    }

    /// Sum of all attributed (observed) damage across participants.
    pub fn total_damage(&self) -> i64 {
        self.participants.iter().map(|p| p.damage).sum()
    }

    /// Number of distinct participants that dealt damage or hits.
    pub fn participant_count(&self) -> usize {
        self.participants.len()
    }
}

/// Identity of a selectable Combat History row in the UI: either a single
/// standalone fight (by database row id) or a grouped multi-phase encounter (by
/// its opaque run id). A numeric row id cannot identify a group because a group's
/// membership grows as phases finalize.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum FightSelection {
    /// A standalone fight, keyed by its `fights.id`.
    Single(i64),
    /// A grouped encounter, keyed by its `encounter_run_id`.
    Encounter(String),
}
