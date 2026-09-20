//! Fight reconstruction engine.
//!
//! Consumes the same packet-derived events the loot tracker uses and rebuilds
//! boss fights: boundary detection, boss HP trajectory, participant roster with
//! equipment, and per-player damage attribution.
//!
//! Damage attribution follows the two signal paths a single client actually
//! receives:
//! - Other players' damage comes from incoming `DamagePacket`s (`object_id` =
//!   attacker, `damage_amount` = final post-defense value). Accurate for
//!   attackers within render range.
//! - The local player's own damage is NOT in `DamagePacket`s; only the hit count
//!   is known here (via outgoing `EnemyHit`). The precise self-damage value is a
//!   later addition (shot simulation), so the local participant is tagged
//!   `SelfPending`.

use std::collections::HashMap;
use std::collections::HashSet;
use std::collections::VecDeque;

use crate::api::character::parse_enchant_ids;
use crate::assets::get_asset_manager;
use crate::assets::LethalStrikeParams;
use crate::loot::class_ids;
use crate::protocol::data::{ObjectStatusData, StatType};

use super::damage::{self, AttackerStats, Rng, DEFAULT_EXALT_BONUS};
use super::types::{
    CompletedFight, DamageProvenance, DamageTakenProvenance, FightParticipant,
    ParticipantEndStatus, PetInfo,
};

/// MaxHP fallback for boss classification, used only when an enemy is not tag-
/// classified as boss-like (see `AssetManager::is_boss_like`). Catches untagged
/// high-HP setpieces (e.g. Elder Sprite Tree ~16k, whose catalog entry has no
/// boss labels). Regular realm/dungeon trash sits far below this.
const BOSS_MIN_MAXHP: i32 = 10_000;

/// Close an active fight if its boss has had no HP update or damage for this long
/// (the boss despawned out of range or the player left without a death packet).
const FIGHT_TIMEOUT_MS: i64 = 20_000;

/// How long a non-wandering boss that despawned un-killed is kept suspended,
/// keyed by its object id, so re-engaging the same physical boss (e.g. walking
/// out of range in the realm and coming back) resumes the same fight instead of
/// spawning a duplicate. If it is not seen again within this window it finalizes
/// as an escaped fight.
const BOSS_RESUME_WINDOW_MS: i64 = 180_000;

/// Duration of the rogue Lethal Strike buff after exiting sneak (equip.xml
/// `duration="2.4"`), during which shots get the LS bonus + 2 side procs.
const LETHAL_STRIKE_WINDOW_MS: i64 = 2_400;
/// The shot that cancels sneak (and thus opens the window) can be sent just
/// before the Invisible-off stat update is observed. Back-date the window start
/// by this guard so that boundary shot is still credited.
const LETHAL_STRIKE_GUARD_MS: i64 = 300;
/// Lethal Strike side procs spawn this many tiles from the player (equip.xml
/// `minDistance=maxDistance=4.6` on the cloak's `OnPlayerShootActivate`).
const LS_PROC_SPAWN_TILES: f32 = 4.6;
/// Angular offset (radians) of each of the 2 side procs from the shot angle
/// (equip.xml `offsetAngle=+/-9`).
const LS_PROC_OFFSET_RAD: f32 = 0.157_079_63; // 9 degrees
/// Distance (tiles) a side proc travels from its spawn point before expiring.
/// The proc bullet type is not described in equip.xml, so this is a generous
/// upper bound: the point-blank overshoot and +/-9 perpendicular miss are the
/// modes that matter, not the far cutoff.
const LS_PROC_RANGE_TILES: f32 = 20.0;
/// Base enemy collision radius (tiles) before the `CustomHitbox` scale. RotMG
/// enemies use a ~0.5-tile collision circle scaled by the object's hitbox.
const LS_ENEMY_BASE_RADIUS: f32 = 0.5;
/// Extra radius (tiles) folded in for the proc bullet's own size / aim slack.
const LS_PROC_RADIUS: f32 = 0.25;

/// Count how many of the 2 Lethal Strike side procs reach a target circle.
///
/// Each proc spawns [`LS_PROC_SPAWN_TILES`] from `(px, py)` along
/// `angle +/- LS_PROC_OFFSET_RAD` and travels outward; it lands if its
/// trajectory (a segment of length [`LS_PROC_RANGE_TILES`] from the spawn point)
/// passes within `radius` of the target centred at `(tx, ty)`. This naturally
/// rejects the point-blank case (target inside the spawn ring, so the proc
/// spawns past it) and the perpendicular miss (a small target slips between the
/// two +/-9 procs).
fn ls_proc_hits(px: f32, py: f32, angle: f32, tx: f32, ty: f32, radius: f32) -> u32 {
    let mut hits = 0;
    for offset in [LS_PROC_OFFSET_RAD, -LS_PROC_OFFSET_RAD] {
        let a = angle + offset;
        let (ca, sa) = (a.cos(), a.sin());
        let sx = px + LS_PROC_SPAWN_TILES * ca;
        let sy = py + LS_PROC_SPAWN_TILES * sa;
        // Closest point on the segment [0, range] to the target centre.
        let t = ((tx - sx) * ca + (ty - sy) * sa).clamp(0.0, LS_PROC_RANGE_TILES);
        let (cx, cy) = (sx + t * ca, sy + t * sa);
        let (dx, dy) = (tx - cx, ty - cy);
        if dx * dx + dy * dy <= radius * radius {
            hits += 1;
        }
    }
    hits
}
/// Primary condition bit (id 29) for Invisible / cloaked. Its falling edge on
/// the local player opens a Lethal Strike window.
const COND_INVISIBLE: i32 = 0x0000_1000;
/// Primary condition bit (id 29) for Invulnerable: the target takes no damage,
/// so no Lethal Strike bonus is attributed either.
const COND_INVULNERABLE: i32 = 0x0100_0000;
/// Item slot index of the ability item (rogue cloak) in a player's equipment.
const ABILITY_SLOT: usize = 1;
/// Item slot index of the main-hand weapon in a player's equipment.
const WEAPON_SLOT: usize = 0;
/// Item slot index of the armor item in a player's equipment.
const ARMOR_SLOT: usize = 2;
/// Item id of Armor of Nil, whose "Planar Absorption" passive negates 15% of
/// post-defense damage in a 5s window on a 10s cooldown, triggered on being hit.
const ARMOR_OF_NIL_ITEM_ID: i32 = 7381;
/// Armor of Nil active window and cooldown, in milliseconds.
const NIL_ACTIVE_MS: i64 = 5_000;
const NIL_COOLDOWN_MS: i64 = 10_000;
/// Knight shield damage-reduction window after activation, in milliseconds.
const SHIELD_ACTIVE_MS: i64 = 5_000;

/// Oryx the Mad God 3 boss object type. Damage dealt to him while he is in his
/// Guarded (shield) state is "wasted" and risks his Silence counter, so it is
/// tracked separately from total damage.
const O3_BOSS_TYPE: i32 = 45363;
/// Stat 125 (`StatType::Animation`) values observed on O3 while he is guarding.
/// These are empirically-derived constants from packet captures and
/// may need updating after a fresh capture. Guard state is not a condition bit.
const O3_GUARD_ANIM: [i32; 2] = [-935_464_302, -918_686_683];

/// Sentinel attacker id (0xFFFFFF) seen on some `DamagePacket`s - environmental
/// or otherwise unattributable source, not a real player.
const SENTINEL_ATTACKER_ID: i32 = 0x00FF_FFFF;

/// Max fights kept in memory for later UI consumption.
const MAX_RECENT_FIGHTS: usize = 200;

/// Death/nexus detection. A departed participant is marked dead when
/// a gravestone spawned within this many tiles of their last-seen position.
const GRAVE_MATCH_RADIUS: f32 = 3.0;
/// ...and within this time window of their removal (covers a grave that arrives
/// slightly before or after the removal packet in the same/adjacent tick).
const GRAVE_MATCH_WINDOW_MS: i64 = 15_000;
/// A departure only counts for a fight if it happened no earlier than this before
/// the fight's start (guards against correlating a much older, unrelated exit).
const DEPARTURE_GRACE_MS: i64 = 5_000;
/// Recent gravestone spawns / participant departures older than this are pruned.
/// Set well above any realistic single-fight duration so a death early in a long
/// boss fight is still correlated at finalize (buffers are also fully cleared on
/// every map change / disconnect, bounding growth per map instance).
const CORRELATION_MAX_AGE_MS: i64 = 600_000;
/// Hard cap on each correlation buffer, so a long session cannot grow it without
/// bound even if pruning by age lags.
const CORRELATION_MAX_ENTRIES: usize = 256;

/// A gravestone observed spawning, buffered for death correlation.
#[derive(Debug, Clone, Copy)]
struct GraveSpawn {
    grave_type: i32,
    x: f32,
    y: f32,
    time: i64,
}

/// A participant's last-seen position when their object left an active fight.
#[derive(Debug, Clone, Copy)]
struct Departure {
    x: f32,
    y: f32,
    time: i64,
}

/// Per-instance pet association state. A pet follows its owner, so
/// across the map instance we tally how often each pet was the closest object to
/// each player; the top-voted player becomes the pet's owner at finalize. Pet
/// identity persists even after the pet object is removed, so a pet despawning
/// just before a boss dies still shows.
#[derive(Debug, Clone, Default)]
struct PetTrack {
    /// Pet sprite / skin id (StatType::PetType).
    pet_type: i32,
    /// Pet ability type ids in slot order (0 = unused).
    pet_abilities: [i32; 3],
    /// player object id -> number of observations this pet was nearest to them.
    votes: HashMap<i32, u32>,
}

/// Max distance (tiles) between a pet and a player for a proximity vote to count.
const PET_ASSOC_MAX_DIST: f32 = 8.0;

/// Soft cap on buffered enemy projectiles awaiting a local `PlayerHit`. Enemy
/// bullets that never hit the local player are only freed on map change, so the
/// buffer is cleared once it grows past this (a damage-taken estimate tolerates
/// the rare miss after an overflow).
const MAX_PENDING_ENEMY_SHOTS: usize = 8192;

/// Which active fight an incoming (damage-taken) hit is credited to.
#[derive(Debug, Clone, Copy)]
enum TakenDest {
    /// A boss fight keyed by boss object id.
    Boss(i32),
    /// An aggregated aux fight keyed by its category.
    Aux(&'static str),
}

/// A participant's display identity snapshotted at credit time, so a player who
/// only ever took damage (and despawns before finalize) still resolves.
#[derive(Debug, Clone, Default)]
struct IdentitySnapshot {
    name: Option<String>,
    object_type: i32,
    skin_id: i32,
    tex1: u32,
    tex2: u32,
}

/// Apply a non-empty identity snapshot onto a participant accumulator (each field
/// only overwrites when the snapshot carries a resolved value).
fn apply_identity_snapshot(accum: &mut ParticipantAccum, snap: &IdentitySnapshot) {
    if snap.name.is_some() {
        accum.last_name = snap.name.clone();
    }
    if snap.object_type != 0 {
        accum.last_object_type = snap.object_type;
    }
    if snap.skin_id != 0 {
        accum.last_skin_id = snap.skin_id;
    }
    if snap.tex1 != 0 {
        accum.last_tex1 = snap.tex1;
    }
    if snap.tex2 != 0 {
        accum.last_tex2 = snap.tex2;
    }
}

/// A tracked world object (player, boss, or other enemy) with the stats the
/// combat engine cares about.
#[derive(Debug, Clone)]
struct TrackedObject {
    object_type: i32,
    name: Option<String>,
    max_hp: i32,
    hp: i32,
    /// Skin id (StatType::SkinId); 0 = default class sprite.
    skin_id: i32,
    /// Clothing/accessory dye textures (StatType::Texture1/Texture2); 0 = none.
    tex1: u32,
    tex2: u32,
    /// Inventory slots 0-3 (weapon, ability, armor, ring); -1 when unknown.
    equipment: [i32; 4],
    /// Per-slot enchant ids (weapon, ability, armor, ring) decoded from the
    /// `UniqueDataString` stat; empty when none/unknown.
    equipment_enchants: [Vec<u16>; 4],
    /// Attack stat (id 20); relevant for the local player's damage multiplier.
    attack: i32,
    /// Defense stat (id 21); relevant when this object is a damage target.
    defense: i32,
    /// Primary condition bitmask (id 29).
    condition: i32,
    /// Secondary "new condition" bitmask (id 69).
    condition_new: i32,
    /// Animation stat (id 125). For O3 this carries his Guarded-state signal.
    animation: i32,
    /// `ExaltationBonusDamage` (id 113), per-mille; defaults to x1.0.
    exalt_bonus: i32,
    /// Base Wisdom stat (id 27); used for the local rogue's Lethal Strike bonus.
    /// The server already folds any WisdomBoost (id 52) into this value, so the
    /// boost stat is not summed separately (doing so double-counts it).
    wisdom: i32,
    /// MaxMP (id 3); the server already folds MaxMPBoost (id 47) into it. Used
    /// to scale MAXMP-scaled ability damage (e.g. Command Cornea).
    max_mp: i32,
    /// Dexterity (id 28); the server already folds DexterityBoost (id 53) into
    /// it. Scales DEX-scaled abilities (e.g. Enchantment Orb).
    dexterity: i32,
    /// Whether a base Wisdom stat has been observed (avoids a 0-WIS default
    /// under-applying the Lethal Strike bonus before the first stat arrives).
    wisdom_seen: bool,
    /// Whether an Attack stat has been observed for this object (so the local
    /// player's damage multiplier is not computed from a zero default).
    attack_seen: bool,
    /// Whether a Defense stat has been observed (so the local player's damage
    /// taken is not estimated from a zero default).
    defense_seen: bool,
    /// Last time (epoch millis) this object's stats were updated.
    last_seen: i64,
    /// Last-seen world position (tile coords), used to correlate a departed
    /// player with the gravestone that spawns at their death tile.
    last_x: f32,
    last_y: f32,
    /// Whether this object is a pet (carries a PetType stat); pets follow their
    /// owner and are associated to a player by proximity.
    is_pet: bool,
    /// Pet sprite / skin id (StatType::PetType); 0 when not a pet.
    pet_type: i32,
    /// Pet ability type ids (StatType::Pet{First,Second,Third}AbilityType);
    /// 0 in unused slots.
    pet_abilities: [i32; 3],
}

impl Default for TrackedObject {
    fn default() -> Self {
        Self {
            object_type: 0,
            name: None,
            max_hp: 0,
            hp: 0,
            skin_id: 0,
            tex1: 0,
            tex2: 0,
            equipment: [-1; 4],
            equipment_enchants: Default::default(),
            attack: 0,
            defense: 0,
            condition: 0,
            condition_new: 0,
            animation: 0,
            exalt_bonus: DEFAULT_EXALT_BONUS,
            wisdom: 0,
            max_mp: 0,
            dexterity: 0,
            wisdom_seen: false,
            attack_seen: false,
            defense_seen: false,
            last_seen: 0,
            last_x: 0.0,
            last_y: 0.0,
            is_pet: false,
            pet_type: 0,
            pet_abilities: [0; 3],
        }
    }
}

impl TrackedObject {
    /// Effective wisdom for damage scaling: base Wisdom plus the summed boost
    /// (gear, consumables, dungeon modifiers). Returns `None` until a base
    /// Wisdom stat has been observed.
    fn effective_wis(&self) -> Option<i32> {
        self.wisdom_seen.then_some(self.wisdom)
    }

    /// Effective value of a scaling stat, used for Lethal Strike and other
    /// stat-scaled procs. Returns `None` when the required base stat hasn't
    /// been observed yet. The server already folds each stat's boost component
    /// (ids 46-53) into the base stat (ids 20-28), so the boost is not added
    /// again here (that would double-count it).
    fn effective_scaling_stat(&self, stat: crate::assets::ScalingStat) -> Option<i32> {
        use crate::assets::ScalingStat;
        match stat {
            ScalingStat::Wisdom => self.effective_wis(),
            ScalingStat::Attack => self.attack_seen.then_some(self.attack),
            ScalingStat::Defense => self.defense_seen.then_some(self.defense),
            ScalingStat::Dexterity => Some(self.dexterity),
            ScalingStat::MaxMp => Some(self.max_mp),
            _ => None,
        }
    }

    /// Whether this object is O3 currently in his Guarded (shield) state, in
    /// which damage dealt is "wasted" and risks his Silence counter.
    fn is_o3_guarding(&self) -> bool {
        self.object_type == O3_BOSS_TYPE && O3_GUARD_ANIM.contains(&self.animation)
    }

    fn is_boss(&self) -> bool {
        let am = get_asset_manager();
        let otype = self.object_type as i32;
        // Curated deny-list: known boss summons / adds (Legion Footsoldier/Major)
        // that carry boss-tier HP but no labels, so nothing else demotes them.
        if am.is_curated_non_boss(otype) {
            return false;
        }
        // Curated allow-list: dungeon minibosses the catalog mislabels as MINION
        // (e.g. Sea Dragon, the Deep Sea Abyss gods). They share their label
        // string with their own adds, so only an explicit list can promote them.
        if am.is_curated_boss(otype) {
            return true;
        }
        // A spawned minion / clone / stage-add only counts as its own fight when
        // it also carries a STRONG boss label (BOSS/MINIBOSS/HERO/ENCOUNTER) --
        // e.g. Heroes of Oryx like Red Demon (MINION,HERO,...). A mere QUEST tag
        // (the quest-arrow marker on realm trash such as Urgle the Traptosser)
        // does not qualify, and neither does boss-tier HP (Crystal Prisoner
        // Clone, Mysterious Crystal, Oryx Judge).
        if am.is_minion(otype) {
            return am.is_strong_boss_like(otype);
        }
        // Any boss-like catalog label (including the weaker QUEST marker on
        // non-minion notable enemies) makes it a real fight, even at low HP.
        if am.is_boss_like(otype) {
            return true;
        }
        // HP-only fallback: give the benefit of the doubt to an *uncatalogued*
        // high-HP enemy (no labels at all -- e.g. Elder Sprite Tree, Monstrous
        // Grizzly), but not to one the catalog deliberately classified without a
        // boss label. A labeled-but-not-boss enemy is a boss add, not its own
        // fight (e.g. Deep Sea Beast is ENEMY,STASISIMMUNE,BEAST -- a Thessal add).
        if am
            .object_labels(otype)
            .map_or(false, |l| !l.trim().is_empty())
        {
            return false;
        }
        // The fallback must only fire for actual mobs. Reject only when the
        // catalog *positively* classes this as a non-`Character` object -- walls,
        // O3 columns/skeletons (GameObject), player-spawned objects -- which
        // carry boss-tier HP but are never fights. An unknown class (no catalog
        // entry) keeps the benefit of the doubt.
        if let Some(class) = am.object_class(otype) {
            if !class.is_empty() && class != "Character" {
                return false;
            }
        }
        self.max_hp >= BOSS_MIN_MAXHP
    }

    fn attacker_stats(&self) -> AttackerStats {
        AttackerStats {
            attack: self.attack,
            condition: self.condition,
            exalt_bonus: self.exalt_bonus,
        }
    }
}

/// A local shot awaiting its `EnemyHit` confirmation, keyed by bullet id. The
/// base (pre-defense) damage is rolled at shoot time; defense is applied per hit
/// against the target's live stats (a piercing bullet may hit several targets).
#[derive(Debug, Clone, Copy, Default)]
struct PendingShot {
    base_damage: i32,
    armor_piercing: bool,
    /// When this shot was fired (epoch millis), used to test membership in a
    /// Lethal Strike window at hit time (hits arrive after the window is known).
    shot_time: i64,
    /// Lethal Strike proc snapshot taken at shoot time: the equipped cloak's
    /// params + the player's effective WIS. `None` when the player has no LS
    /// cloak or WIS is unseen. The per-target bonus is computed at hit time.
    lethal_strike: Option<(LethalStrikeParams, i32)>,
    /// Set once the two side procs for this trigger have been credited, so a
    /// piercing bullet that yields several hits does not spawn duplicate procs.
    procs_credited: bool,
    /// True for the secondary projectiles of a multi-projectile weapon shot (any
    /// bullet after the first sharing the trigger's client time). The two Lethal
    /// Strike side procs fire once per shot trigger, not once per projectile, so
    /// only the primary projectile credits them; the per-projectile main-shot
    /// bonus still applies to every projectile.
    extra_projectile: bool,
    /// Shot-time player position and firing angle `(x, y, angle_radians)` from
    /// the PlayerShoot packet, used to reconstruct the Lethal Strike side-proc
    /// trajectories. `None` for shots fired before this was recorded (and in
    /// tests), in which case both procs are credited unconditionally.
    shot_geo: Option<(f32, f32, f32)>,
}

/// Accumulated contribution from one attacker during an active fight.
#[derive(Debug, Clone, Default)]
struct ParticipantAccum {
    damage: i64,
    hits: u64,
    is_local: bool,
    /// Subset of `damage`/`hits` dealt to O3 while he was Guarded.
    /// Included in the totals above; tracked separately for display.
    guarded_damage: i64,
    guarded_hits: u64,
    /// Post-defense damage this participant took from fight enemies. Remote
    /// players: summed exactly from `DamagePacket`s. Local player: reconstructed
    /// from correlated enemy shots (approximate).
    damage_taken: i64,
    /// Number of incoming hits credited to `damage_taken` (a value was resolved).
    damage_taken_hits: u64,
    /// Pre-defense damage the local player's defense and other protections
    /// negated across this fight (the "Mell stat"). Only ever set for
    /// the local player, where the pre-defense projectile damage is known.
    damage_blocked: i64,
    /// Local incoming hits that could not be valued (uncorrelated bullet or
    /// unknown defense). Makes the local taken figure a lower bound (`Partial`).
    unmatched_taken_hits: u64,
    /// Local hits whose bullet could not be correlated to a rolled shot (so no
    /// damage value was added). Used to distinguish `SelfComputed` from
    /// `SelfPartial`.
    unresolved_local_hits: u64,
    /// Latest resolved player name / object type for this attacker, snapshotted
    /// at hit time. Aggregated aux fights span the whole run, by which point the
    /// attacker object may have despawned, so identity can't be resolved lazily.
    last_name: Option<String>,
    last_object_type: i32,
    /// Last-seen skin id / dye textures for this attacker, snapshotted at hit
    /// time so despawned/aggregated participants still render their skin+dyes.
    last_skin_id: i32,
    last_tex1: u32,
    last_tex2: u32,
    /// Damage + hit count grouped by the loadout (gear ids + per-slot enchants)
    /// worn at the moment of each hit. Used to surface the gear the player did
    /// the most damage with -- and its matching enchants -- rather than whatever
    /// they happened to have equipped when the boss died.
    damage_by_equipment: HashMap<([i32; 4], [Vec<u16>; 4]), (i64, u64)>,
}

/// State of an in-progress boss fight, keyed by the boss object id.
#[derive(Debug, Clone)]
struct FightState {
    boss_object_id: i32,
    boss_object_type: i32,
    boss_max_hp: i32,
    boss_start_hp: i32,
    started_at: i64,
    last_activity: i64,
    killed: bool,
    /// Lowest HP the boss was ever observed at during this fight. Used for kill
    /// detection: transition/petrify bosses (e.g. Lair of Draconis dragons) dip
    /// to ~0 then reset to full HP for a new phase under the same object id, so
    /// the HP at removal is misleadingly high. The minimum captures the death.
    min_hp_seen: i64,
    /// Local player's char id when this fight began (0 if unknown then).
    local_char_id: i32,
    /// Count of local-player "close calls" (HP dropped below 20% of max,
    /// deaths included) observed while this fight was active.
    local_close_calls: i32,
    /// attacker object id -> accumulated contribution.
    attackers: HashMap<i32, ParticipantAccum>,
    /// Optional segment label appended to the boss name ("Pre-survival" /
    /// "Post-survival") for bosses split at a heal-back. `None` for normal fights.
    segment_label: Option<&'static str>,
    /// Once set, this name replaces the resolved boss name entirely -- used by
    /// aggregated aux summary fights (e.g. "Marble Core").
    display_name_override: Option<&'static str>,
    /// True once a second-coming split has already happened for this object, so
    /// the heal-back is only acted on once.
    split_done: bool,
    /// True when the local player reached the boss after the fight had already
    /// begun (boss below full HP on first sight), so remote observed damage is
    /// only a partial figure. Never set for heal-back / aux segments.
    joined_late: bool,
}

/// Run-scoped tally of one aux category's member instances (object id -> highest
/// max HP seen). Counting distinct object ids captures every spawned instance
/// regardless of who damaged it, so e.g. all six Spectral Keys are counted.
#[derive(Debug, Clone, Default)]
struct AuxInstances {
    max_hp_by_id: HashMap<i32, i32>,
}

impl AuxInstances {
    fn count(&self) -> i32 {
        self.max_hp_by_id.len() as i32
    }

    fn total_hp(&self) -> i32 {
        self.max_hp_by_id
            .values()
            .map(|&hp| hp as i64)
            .sum::<i64>()
            .min(i32::MAX as i64) as i32
    }
}

/// The combat reconstruction engine.
pub struct CombatTracker {
    /// Current map / dungeon name.
    current_map: String,
    /// Current map seed.
    current_seed: i32,
    /// When the local player entered the current map instance (epoch millis), or
    /// `None` when the current map is not a groupable dungeon. Set on map change,
    /// carried onto every fight finalized in this dungeon so the encounter card
    /// can measure total dungeon time from entry to the final boss death.
    dungeon_entered_at: Option<i64>,
    /// Local player's object id (0 until known).
    local_object_id: i32,
    /// Local player's character id / char_id (0 until known). Stamped onto each
    /// fight when it begins so the UI can filter by character.
    local_char_id: i32,
    /// All tracked objects in the current map instance.
    objects: HashMap<i32, TrackedObject>,
    /// Active fights keyed by boss object id.
    fights: HashMap<i32, FightState>,
    /// Aggregated non-boss damage summaries for the current run, keyed by aux
    /// category (e.g. "marble_core"). Accumulate across the whole run and emit as
    /// synthetic summary fights at run end.
    aux_fights: HashMap<&'static str, FightState>,
    /// Per-aux-category tally of every distinct member instance seen in the world
    /// this run (object id -> max HP observed). Drives the aggregated row's
    /// member count and summed HP, independent of who dealt damage. Cleared per
    /// run and per category when its aux fight finalizes.
    aux_instances: HashMap<&'static str, AuxInstances>,
    /// Suspended fights for curated wandering bosses that despawned un-killed,
    /// keyed by boss object type. Resumed under the boss's new object id when it
    /// re-enters view so a roaming boss is not recorded as several duplicate
    /// fights; drained (finalized as escaped) at run end.
    dormant_fights: HashMap<i32, FightState>,
    /// Suspended fights for ordinary (non-wandering) bosses that despawned
    /// un-killed, keyed by boss object id. Unlike wandering bosses, these keep
    /// their object id when they re-enter view (e.g. walking out of range in the
    /// realm and back), so re-engaging the same id resumes the same fight. Pruned
    /// to escaped once older than `BOSS_RESUME_WINDOW_MS`, or at run end.
    dormant_by_id: HashMap<i32, FightState>,
    /// Fights finalized outside a value-returning call (e.g. an object-id reused
    /// for a different boss type evicts the stale suspended fight during damage
    /// attribution). Flushed to the caller on the next `on_tick` / run end so
    /// they still reach the database.
    deferred_finished: Vec<CompletedFight>,
    /// Recently completed fights (for later UI use).
    recent_fights: Vec<CompletedFight>,
    /// Damage RNG, re-seeded from the map seed on every map change.
    rng: Rng,
    /// Whether [`Self::rng`] has been seeded for the current map. Shots that
    /// arrive before a seed cannot be rolled.
    rng_seeded: bool,
    /// Whether local self-damage can be trusted for the current map. Cleared for
    /// the rest of a map if the RNG is unseeded when a shot arrives or a weapon's
    /// asset data is unavailable (either would desync the sequence).
    self_damage_available: bool,
    /// Rolled local shots awaiting their `EnemyHit`, keyed by bullet id.
    pending_shots: HashMap<i16, PendingShot>,
    /// Client `time` of the most recent local shot trigger (from the outgoing
    /// `PlayerShoot` packet), paired with its weapon id. All bullets of one
    /// multi-projectile shot share the same client time and weapon, so a bullet
    /// whose (client time, weapon id) equals this is a secondary projectile of the
    /// same trigger (used to credit LS side procs once per trigger, not once per
    /// projectile). The weapon id guards against a same-tick shoot-type ability
    /// colliding with a weapon trigger. `None` until the first shot.
    last_shot_trigger: Option<(i32, i32)>,
    /// Active Lethal Strike windows (start_ms, end_ms) for the local player,
    /// opened on the Invisible condition falling edge. Bounded; cleared
    /// on map change. A local weapon shot fired within a window gets the LS
    /// bonus + 2 side procs attributed at hit time.
    lethal_strike_windows: VecDeque<(i64, i64)>,
    /// Summon entity id -> its summoner (owning player) id, learned from
    /// `ServerPlayerShoot`. Covers both local and remote summons; used to
    /// redirect remote summon `DamagePacket`s to the owning player.
    summon_owners: HashMap<i32, i32>,
    /// Pre-defense damage of the local player's own summon shots awaiting their
    /// `EnemyHit`, keyed by (summon entity id, bullet id). Only populated for
    /// summons whose summoner is the local player.
    pending_summon_shots: HashMap<(i32, i16), i32>,
    /// Pre-defense damage of the local player's OWN server-fired bullets (weapon
    /// procs, ability volleys/secondary shots) awaiting their `EnemyHit`, keyed
    /// by bullet id. The value is `(damage, armor_piercing)`. These arrive via
    /// `ServerPlayerShoot` with owner == local and are confirmed by an outgoing
    /// `EnemyHit`; their damage is only in that packet. Kept disjoint from
    /// `pending_shots` (newest registration owns an id). Cleared on map change.
    pending_server_shots: HashMap<i16, (i32, bool)>,
    /// Object ids of every player-class object seen in the current map instance.
    /// Used at finalize to prove a fight was effectively solo (no *other* player
    /// present) before crediting unattributed boss-HP loss to the local player.
    /// Persists for the run (immune to teammate despawns) and clears on map
    /// change.
    seen_player_ids: std::collections::HashSet<i32>,
    /// Pre-defense damage of enemy projectiles awaiting a local `PlayerHit`,
    /// keyed by (enemy owner id, bullet id). The value is `(damage,
    /// armor_piercing)`; armor-piercing shots bypass the local player's defense.
    /// Used to estimate the local player's damage taken. Cleared on map change /
    /// disconnect.
    pending_enemy_shots: HashMap<(i32, i16), (i32, bool)>,
    /// Random per-tracker nonce so run ids are unique across sessions/databases.
    session_nonce: u64,
    /// Incremented on every map change; combined with the nonce to form a run id.
    run_counter: u64,
    /// Opaque run id for the current map instance (see `CompletedFight`). Minted
    /// on map change; stamped onto member fights of a curated encounter.
    current_run_id: Option<String>,
    /// Gravestones observed spawning this run, newest last. Correlated with a
    /// participant's departure position/time to detect deaths.
    recent_graves: VecDeque<GraveSpawn>,
    /// Last-seen position/time of participants whose object left an active fight,
    /// keyed by object id. Resolved at finalize into Died (grave nearby) or
    /// Nexused (no grave).
    departures: HashMap<i32, Departure>,
    /// Authoritative local-player death from `CharacterDied`
    /// (char_id, gravestone_type, time). Overrides presence at finalize because
    /// the local object is still in `objects` when a map-change finalize runs.
    pending_local_grave: Option<(i32, i32, i64)>,
    /// Authoritative remote-player deaths parsed from the server death-broadcast
    /// text ("<name> died at level N, killed by <killer>"). Keyed by the
    /// lowercased player name -> death time. This is the reliable death signal
    /// (position-based grave correlation mis-assigns graves when several players
    /// die in a tight cluster), so it overrides a Nexused verdict at finalize.
    /// Cleared on map change / disconnect like the other correlation buffers.
    remote_deaths: HashMap<String, i64>,
    /// Set while a map change / disconnect force-finalizes active fights, so the
    /// local player -- always still in `objects` from their own view -- is scored
    /// as having left (nexused) any boss fight that was not completed.
    local_departing: bool,
    /// Per-instance pet -> owner proximity votes and pet identity.
    /// Keyed by pet object id; reset on map change / disconnect.
    pet_tracks: HashMap<i32, PetTrack>,
    /// Boss object types marked completed for the current run by a Moonlight
    /// Village completion marker. Bosses here go invulnerable
    /// instead of dying, so any fight of these types is force-scored as a kill at
    /// finalize regardless of its observed HP. Cleared on map change / disconnect.
    completed_boss_types: HashSet<i32>,
    /// Subset of [`completed_boss_types`] whose completed fight has already been
    /// emitted this run. Used to reject a duplicate fight if
    /// lingering fire re-touches the still-present invulnerable boss object after
    /// its first completed record was finalized. Cleared on map change / disconnect.
    completed_recorded_types: HashSet<i32>,
    /// Local player's Knight-shield damage-reduction window: end time (epoch
    /// millis) and its percentage (25%, or 30% for the Kogbold Cower Shield),
    /// opened by a `UseItem` on a shield ability. `until == 0` when inactive.
    shield_dr_until: i64,
    shield_dr_pct: u8,
    /// Local player's Armor of Nil "Planar Absorption" proc timeline: the active window end and the earliest time it may re-proc (10s
    /// cooldown from the last proc). Both epoch millis; `0` when never procced.
    nil_active_until: i64,
    nil_ready_at: i64,
    /// Whether the local player's HP is currently below the close-call
    /// threshold (< 20% of max). Used for edge detection so each dip counts once.
    local_low_hp_active: bool,
    /// Local-player close calls detected since the last drain, for the processor
    /// to fold into the per-character lifetime counter.
    pending_close_calls: u32,
    /// Close calls observed in the current run while no fight was active, carried
    /// onto the next fight started this run so a pre-boss dip still shows on the
    /// run card. Reset on map change.
    run_pending_fight_close_calls: i32,
}

impl Default for CombatTracker {
    fn default() -> Self {
        Self::new()
    }
}

impl CombatTracker {
    /// Create an empty tracker.
    pub fn new() -> Self {
        Self {
            current_map: String::new(),
            current_seed: 0,
            dungeon_entered_at: None,
            local_object_id: 0,
            local_char_id: 0,
            objects: HashMap::new(),
            fights: HashMap::new(),
            aux_fights: HashMap::new(),
            aux_instances: HashMap::new(),
            dormant_fights: HashMap::new(),
            dormant_by_id: HashMap::new(),
            deferred_finished: Vec::new(),
            recent_fights: Vec::new(),
            rng: Rng::new(0),
            rng_seeded: false,
            self_damage_available: false,
            pending_shots: HashMap::new(),
            last_shot_trigger: None,
            lethal_strike_windows: VecDeque::new(),
            summon_owners: HashMap::new(),
            pending_summon_shots: HashMap::new(),
            pending_server_shots: HashMap::new(),
            seen_player_ids: std::collections::HashSet::new(),
            pending_enemy_shots: HashMap::new(),
            session_nonce: rand::random(),
            run_counter: 0,
            current_run_id: None,
            recent_graves: VecDeque::new(),
            departures: HashMap::new(),
            pending_local_grave: None,
            remote_deaths: HashMap::new(),
            local_departing: false,
            pet_tracks: HashMap::new(),
            completed_boss_types: HashSet::new(),
            completed_recorded_types: HashSet::new(),
            shield_dr_until: 0,
            shield_dr_pct: 0,
            nil_active_until: 0,
            nil_ready_at: 0,
            local_low_hp_active: false,
            pending_close_calls: 0,
            run_pending_fight_close_calls: 0,
        }
    }

    /// Recently completed fights held in memory.
    pub fn recent_fights(&self) -> &[CompletedFight] {
        &self.recent_fights
    }

    /// Record the local player's object id and char id (from CreateSuccess).
    ///
    /// Backfills any already-active fight that started before the identity was
    /// known, so a fight begun during map load is still attributed correctly.
    pub fn on_player_loaded(&mut self, object_id: i32, char_id: i32) {
        // A different character loading invalidates any buffered local death,
        // so a prior character's death cannot mark the new one.
        if char_id != 0 && self.local_char_id != 0 && char_id != self.local_char_id {
            self.pending_local_grave = None;
            // A different character starts with its own low-HP edge state so the
            // prior character's death does not suppress the new one's first dip.
            self.local_low_hp_active = false;
        }
        self.local_object_id = object_id;
        self.local_char_id = char_id;
        if char_id != 0 {
            for fight in self.fights.values_mut() {
                if fight.local_char_id == 0 {
                    fight.local_char_id = char_id;
                }
            }
        }
    }

    /// Drain the lifetime close-call counter accumulated since the last call.
    /// The caller persists these against the local character.
    pub fn take_pending_close_calls(&mut self) -> u32 {
        std::mem::take(&mut self.pending_close_calls)
    }

    /// Handle a map change. Finalizes any active fights (players left the
    /// instance) and resets per-instance tracking.
    pub fn on_map_change(
        &mut self,
        map_name: &str,
        map_seed: i32,
        time_ms: i64,
    ) -> Vec<CompletedFight> {
        // The local player is leaving the current map: any fight still open here
        // ends because they left it (nexus / teleport), not because they finished
        // it. Flag the departure so finalize scores the local player as nexused
        // on any boss that was not completed.
        self.local_departing = true;
        let finished = self.finalize_all(time_ms);
        self.local_departing = false;
        self.current_map = map_name.to_string();
        self.current_seed = map_seed;
        // Stamp dungeon entry time so fights finalized in this instance can report
        // total dungeon time from entry to the final boss. Set after finalizing
        // the departing map's fights so those keep the old map's entry time.
        self.dungeon_entered_at =
            is_groupable_dungeon(&normalize_dungeon(map_name)).then_some(time_ms);
        // Mint a fresh opaque run id for the new map instance. A reconnect thus
        // splits an encounter into separate runs (safer than merging unrelated
        // runs that happen to share a recycled map seed).
        self.run_counter += 1;
        self.current_run_id = Some(format!(
            "{:016x}-{:x}",
            self.session_nonce, self.run_counter
        ));
        self.objects.clear();
        self.fights.clear();
        self.aux_instances.clear();
        // Re-seed the damage RNG for the new instance and re-enable self-damage.
        self.rng = Rng::new(map_seed);
        self.rng_seeded = true;
        self.self_damage_available = true;
        self.pending_shots.clear();
        self.last_shot_trigger = None;
        self.lethal_strike_windows.clear();
        self.summon_owners.clear();
        self.pending_summon_shots.clear();
        self.pending_server_shots.clear();
        self.seen_player_ids.clear();
        self.pending_enemy_shots.clear();
        self.recent_graves.clear();
        self.departures.clear();
        self.pending_local_grave = None;
        self.remote_deaths.clear();
        self.pet_tracks.clear();
        self.completed_boss_types.clear();
        self.completed_recorded_types.clear();
        self.reset_local_mitigation_windows();
        // A new instance starts a fresh close-call carry buffer; the low-HP edge
        // state resets so the first dip in the new map always counts.
        self.local_low_hp_active = false;
        self.run_pending_fight_close_calls = 0;
        finished
    }

    /// Handle disconnect - finalize all active fights.
    pub fn on_disconnect(&mut self, time_ms: i64) -> Vec<CompletedFight> {
        // A disconnect ends any open fight because the local player left it, not
        // because they finished it -- same departure semantics as a map change.
        self.local_departing = true;
        let finished = self.finalize_all(time_ms);
        self.local_departing = false;
        self.objects.clear();
        self.fights.clear();
        self.aux_instances.clear();
        // Leaving the world clears the current dungeon; the next map change stamps
        // a fresh entry time if it loads a dungeon.
        self.dungeon_entered_at = None;
        // The RNG state does not survive a disconnect; a new map seed is required.
        self.rng_seeded = false;
        self.self_damage_available = false;
        self.pending_shots.clear();
        self.last_shot_trigger = None;
        self.lethal_strike_windows.clear();
        self.summon_owners.clear();
        self.pending_summon_shots.clear();
        self.pending_server_shots.clear();
        self.seen_player_ids.clear();
        self.pending_enemy_shots.clear();
        // A different character may log in next; identity is re-set on CreateSuccess.
        self.local_char_id = 0;
        self.recent_graves.clear();
        self.departures.clear();
        self.pending_local_grave = None;
        self.remote_deaths.clear();
        self.pet_tracks.clear();
        self.completed_boss_types.clear();
        self.completed_recorded_types.clear();
        self.reset_local_mitigation_windows();
        self.local_low_hp_active = false;
        self.run_pending_fight_close_calls = 0;
        finished
    }

    /// Record a freshly spawned object (from an `Update` packet), which carries
    /// its object type.
    pub fn on_object_spawn(
        &mut self,
        object_id: i32,
        object_type: i32,
        status: &ObjectStatusData,
        time_ms: i64,
    ) {
        // A respawn of a previously-departed object cancels its pending departure
        // (it did not leave the fight after all).
        self.departures.remove(&object_id);
        // Moonlight Village completion marker: an invulnerable-finish boss's
        // progress bar filled and its loot dropped. Consolidate all
        // live and suspended fights of each cleared boss type into a single
        // finalized kill so a summoner run that re-detected the boss under many
        // object ids yields exactly one record instead of one per id.
        if let Some(targets) = get_asset_manager().completion_marker_targets(object_type) {
            for &boss_type in targets {
                self.completed_boss_types.insert(boss_type);
                if let Some(cf) = self.consolidate_completed_boss(boss_type, time_ms) {
                    self.deferred_finished.push(cf);
                }
            }
        }
        // Buffer gravestone spawns for death correlation at finalize.
        if get_asset_manager().is_gravestone(object_type) {
            self.recent_graves.push_back(GraveSpawn {
                grave_type: object_type,
                x: status.pos.x,
                y: status.pos.y,
                time: time_ms,
            });
            self.prune_correlation_buffers(time_ms);
        }
        let entry = self.objects.entry(object_id).or_default();
        entry.object_type = object_type;
        entry.equipment = [-1; 4];
        // Note any player-class object for the effectively-solo test used by
        // finalize's HP-gap reconciliation. Recorded for every player id so a
        // teammate who later despawns still counts as "another player present".
        if class_ids::is_player_class(object_type) {
            self.seen_player_ids.insert(object_id);
        }
        // Reset pet state: a recycled object ID must not inherit a prior pet.
        entry.is_pet = false;
        entry.pet_type = 0;
        entry.pet_abilities = [0; 3];
        self.pet_tracks.remove(&object_id);
        apply_stats(entry, status, time_ms);
        if entry.is_pet {
            self.record_pet_observation(object_id);
        }
        let spawn_max_hp = self.objects.get(&object_id).map(|o| o.max_hp).unwrap_or(0);
        self.note_aux_instance(object_id, object_type, spawn_max_hp);
    }

    /// Update an existing object's stats (from a `NewTick` status, which does not
    /// carry the object type). Returns any fight segment finalized as a side
    /// effect (a Marble Colossus survival-phase split emits the pre-survival
    /// segment here).
    pub fn on_object_status(
        &mut self,
        object_id: i32,
        status: &ObjectStatusData,
        time_ms: i64,
    ) -> Vec<CompletedFight> {
        // Capture the local player's condition before the update so the Lethal
        // Strike window can be opened on the Invisible falling edge.
        let old_local_condition = if object_id == self.local_object_id && self.local_object_id != 0
        {
            self.objects
                .get(&object_id)
                .map(|o| o.condition)
                .unwrap_or(0)
        } else {
            0
        };
        let (new_hp, obj_type, obj_max_hp) = {
            let entry = self.objects.entry(object_id).or_default();
            apply_stats(entry, status, time_ms);
            (entry.hp as i64, entry.object_type, entry.max_hp)
        };
        self.note_aux_instance(object_id, obj_type, obj_max_hp);
        // Rogue Lethal Strike: the buff starts when the local player
        // exits sneak, so open a window on the Invisible condition falling edge.
        if object_id == self.local_object_id && self.local_object_id != 0 {
            let had_invis = old_local_condition & COND_INVISIBLE != 0;
            let has_invis = self
                .objects
                .get(&object_id)
                .map(|o| o.condition & COND_INVISIBLE != 0)
                .unwrap_or(false);
            if had_invis && !has_invis {
                self.open_lethal_strike_window(time_ms);
            }
        }
        // A pet's per-tick position feeds owner-proximity voting.
        if self
            .objects
            .get(&object_id)
            .map(|o| o.is_pet)
            .unwrap_or(false)
        {
            self.record_pet_observation(object_id);
        }

        // Local-player "close call" detection: count each time the
        // local player's HP drops below 20% of max (deaths at 0 HP included),
        // matching the game client's low-HP red sprite flash (Player.as:
        // `hp_ < maxHP_ * 0.2`). Fires on the downward crossing; the state clears
        // only when HP recovers to 20% or above, so hovering below the threshold
        // counts once. Attribute to the most-recently-started active boss fight,
        // or carry it onto the next fight this run when none is active (a pre-boss
        // dungeon dip).
        if object_id == self.local_object_id && self.local_object_id != 0 && obj_max_hp > 0 {
            let is_low = new_hp * 5 < obj_max_hp as i64;
            if is_low && !self.local_low_hp_active {
                self.pending_close_calls = self.pending_close_calls.saturating_add(1);
                if let Some((_, fight)) = self.fights.iter_mut().max_by_key(|(_, f)| f.started_at) {
                    fight.local_close_calls += 1;
                } else {
                    self.run_pending_fight_close_calls += 1;
                }
            }
            self.local_low_hp_active = is_low;
        }
        let has_hp_stat = status
            .stats
            .iter()
            .any(|s| s.stat_type_id == StatType::HP as u8);

        // Keep an active fight's boss HP in sync + detect death / heal-back.
        // Decide the heal-back split without holding a mutable borrow across the
        // finalize call below.
        let heal_back = if let Some(fight) = self.fights.get_mut(&object_id) {
            fight.last_activity = time_ms;
            fight.min_hp_seen = fight.min_hp_seen.min(new_hp);

            // Post-survival second coming: the boss heals to full, and its max HP
            // may have rescaled while the survival phase ran (players died /
            // nexused, so dungeon HP-scaling readjusted). Read the authoritative
            // current max from the status packet and keep the segment's pool
            // pinned to that full value until the rage phase takes real damage,
            // then freeze it. This makes the post card's denominator match the
            // true rescaled pool even though the split fired before the heal
            // completed (taunt) or below the peak (HP fallback).
            if fight.segment_label == Some("Post-survival") && obj_max_hp > 0 {
                // Freeze on the first *positive* damage only. The boss is
                // Invulnerable while it heals, so any local shots that land
                // during the revive resolve to 0 damage (they still register as
                // hits) -- gating on hits would freeze a stale, pre-rescale pool
                // before the authoritative rescaled MaxHP packet arrives.
                let damage_started = fight.attackers.values().any(|a| a.damage > 0);
                if !damage_started {
                    fight.boss_max_hp = obj_max_hp;
                    fight.boss_start_hp = obj_max_hp;
                }
            }

            let max_hp = fight.boss_max_hp as i64;
            // Second-coming split, HP fallback for a culled taunt: the Marble
            // Colossus enters an invulnerable survival phase near 10% HP, then
            // heals back toward full for its second coming. HP can only rise
            // after the survival dip via that heal, so any reading climbing back
            // to >=30% of max signals it (the authoritative trigger is the
            // survival-exit taunt in `on_boss_text`). Gate on the LOWEST HP ever
            // seen (not the previous tick) so a gradual, multi-tick heal-back
            // still triggers the split.
            let hb = has_hp_stat
                && !fight.split_done
                && fight.segment_label == Some("Pre-survival")
                && max_hp > 0
                && fight.min_hp_seen <= max_hp * 15 / 100
                && new_hp >= max_hp * 30 / 100;
            if !hb && new_hp <= 0 {
                fight.killed = true;
            }
            hb
        } else {
            false
        };

        let mut finished = Vec::new();
        if heal_back {
            finished.extend(self.perform_second_coming_split(object_id, time_ms));
        }
        finished
    }

    /// A boss `TextPacket` (taunt) arrived. The Marble Colossus emits a fixed
    /// taunt when it leaves the survival phase and begins its second coming;
    /// that is the authoritative signal to split the fight into pre- and
    /// post-survival segments -- unlike the heal-back HP tick, which is often
    /// culled. Bound to the boss's own object id, so it can never be triggered
    /// by another entity's or a player's chat. Returns the finalized
    /// pre-survival segment, if the split fired here.
    pub fn on_boss_text(
        &mut self,
        object_id: i32,
        text: &str,
        time_ms: i64,
    ) -> Vec<CompletedFight> {
        if !crate::assets::is_second_coming_transition_taunt(text) {
            return Vec::new();
        }
        let ready = self
            .fights
            .get(&object_id)
            .map(|f| !f.split_done && f.segment_label == Some("Pre-survival"))
            .unwrap_or(false);
        if !ready {
            return Vec::new();
        }
        self.perform_second_coming_split(object_id, time_ms)
    }

    /// Finalize the active pre-survival segment for `object_id` (as a
    /// survived-the-phase "kill") and open a fresh post-survival segment under
    /// the same id. Shared by the taunt-driven split (`on_boss_text`, primary)
    /// and the heal-back HP fallback (`on_object_status`). The post segment
    /// starts at the boss's full (possibly rescaled) HP pool, read from the
    /// latest tracked-object max; `on_object_status` keeps it in sync with the
    /// authoritative status packets until real rage damage lands.
    fn perform_second_coming_split(&mut self, object_id: i32, time_ms: i64) -> Vec<CompletedFight> {
        let mut finished = Vec::new();
        let Some(mut first) = self.fights.remove(&object_id) else {
            return finished;
        };
        let obj_type = first.boss_object_type;
        let max_hp = self
            .objects
            .get(&object_id)
            .map(|o| o.max_hp)
            .filter(|&m| m > 0)
            .unwrap_or(first.boss_max_hp);
        first.killed = true; // surviving the phase ends the pre-survival segment.
        if let Some(cf) = self.finalize_fight(first, time_ms) {
            finished.push(cf);
        }
        self.fights.insert(
            object_id,
            FightState {
                boss_object_id: object_id,
                boss_object_type: obj_type,
                boss_max_hp: max_hp,
                boss_start_hp: max_hp,
                started_at: time_ms,
                last_activity: time_ms,
                killed: false,
                min_hp_seen: max_hp as i64,
                local_char_id: self.local_char_id,
                local_close_calls: 0,
                attackers: HashMap::new(),
                segment_label: Some("Post-survival"),
                display_name_override: None,
                split_done: true,
                // The local player saw the whole revive segment; not late.
                joined_late: false,
            },
        );
        finished
    }

    /// Handle an object being removed from the world (an `Update` drop). If the
    /// object was a fight's boss, the fight ends here. A boss removed at or near
    /// zero HP was killed; one removed at high HP simply despawned out of range.
    pub fn on_object_removed(&mut self, object_id: i32, time_ms: i64) -> Option<CompletedFight> {
        // A removed object's id can be reused; drop any summon-ownership so a
        // later reuse of this id can't inherit a stale owner.
        self.summon_owners.remove(&object_id);
        self.pending_summon_shots
            .retain(|&(owner, _), _| owner != object_id);
        let last = self.objects.remove(&object_id);
        // Record a participant departure for death/nexus detection:
        // an object that is currently an attacker in some active fight is leaving
        // view. Snapshot its last-seen position BEFORE it is dropped so a
        // gravestone that spawns at that tile can be correlated at finalize.
        if let Some(obj) = &last {
            if self.is_active_attacker(object_id) {
                self.departures.insert(
                    object_id,
                    Departure {
                        x: obj.last_x,
                        y: obj.last_y,
                        time: time_ms,
                    },
                );
                self.prune_correlation_buffers(time_ms);
            }
        }
        let mut fight = self.fights.remove(&object_id)?;
        // Fold the final observed HP into the minimum before deciding.
        let last_hp = last.as_ref().map(|obj| obj.hp as i64);
        if let Some(obj) = &last {
            fight.min_hp_seen = fight.min_hp_seen.min(obj.hp as i64);
        }
        // A boss whose HP ever reached zero (or <=10%) during the fight was
        // killed, even if a phase transition later reset its HP to full under the
        // same object id (Lair of Draconis dragons petrify this way). A curated
        // wandering boss is exempt from the <=10% heuristic: it routinely dips low
        // then roams out of view still alive, so only an actual zero HP counts as
        // a kill -- otherwise a survivor would be recorded as a phantom kill.
        let max_hp = fight.boss_max_hp as i64;
        let wandering = get_asset_manager().is_wandering_boss(fight.boss_object_type);
        let reached_zero = fight.min_hp_seen <= 0;
        let dipped_lethal = max_hp > 0 && fight.min_hp_seen <= max_hp / 10;
        if reached_zero || (!wandering && dipped_lethal) {
            fight.killed = true;
        }
        // Self-destructing bosses (e.g. the Kogbold Expedition train) explode on
        // death and are removed at high HP, so the HP heuristic above never fires.
        // Score the fight as a kill when the party dealt essentially all of the
        // boss's remaining HP -- this distinguishes a real clear from a genuine
        // escape, where only partial damage was dealt.
        if !fight.killed && get_asset_manager().is_self_destruct_boss(fight.boss_object_type) {
            let start_hp = fight.boss_start_hp as i64;
            let total_damage: i64 = fight.attackers.values().map(|a| a.damage).sum();
            if start_hp > 0 && total_damage * 10 >= start_hp * 9 {
                fight.killed = true;
            }
        }
        // TEMP DIAGNOSTIC: self-destruct bosses (e.g. Kogbold
        // Expedition Engine) can be misrecorded as Escaped in large groups where
        // local-only tracked damage never reaches the 90% threshold. Log the raw
        // signals from every self-destruct removal so a reliable death signal can
        // be identified. Remove once a fix is in place.
        if get_asset_manager().is_self_destruct_boss(fight.boss_object_type) {
            let start_hp = fight.boss_start_hp as i64;
            let total_damage: i64 = fight.attackers.values().map(|a| a.damage).sum();
            let dmg_frac = if start_hp > 0 {
                total_damage as f64 / start_hp as f64
            } else {
                0.0
            };
            let min_hp_frac = if max_hp > 0 {
                fight.min_hp_seen as f64 / max_hp as f64
            } else {
                0.0
            };
            tracing::warn!(
                "[KOGBOLD-DIAG] type={} id={} killed={} start_hp={} max_hp={} \
                 min_hp_seen={} min_hp_frac={:.3} last_hp={:?} tracked_dmg={} \
                 dmg_frac={:.3} attackers={} reached_zero={} dipped_lethal={}",
                fight.boss_object_type,
                fight.boss_object_id,
                fight.killed,
                start_hp,
                max_hp,
                fight.min_hp_seen,
                min_hp_frac,
                last_hp,
                total_damage,
                dmg_frac,
                fight.attackers.len(),
                reached_zero,
                dipped_lethal,
            );
        }
        self.suspend_or_finalize(fight, time_ms)
    }

    /// Route a finalized fight. A boss that despawned without dying is suspended
    /// so its next re-entry resumes the same fight instead of spawning a
    /// duplicate: curated wandering bosses re-enter with a new object id, so they
    /// are keyed by object type; ordinary bosses keep their id (walked out of
    /// range and back), so they are keyed by object id. A real kill finalizes
    /// immediately, folding in any suspended earlier segment first.
    ///
    /// Moonlight Village invulnerable-finish bosses (dancers / Umi) are keyed by
    /// type too: a summoner run re-detects them under many object ids before the
    /// completion marker scores the kill, so type-keyed suspension folds every
    /// re-detection into one fight per boss type instead of one record per id.
    fn suspend_or_finalize(&mut self, fight: FightState, time_ms: i64) -> Option<CompletedFight> {
        let otype = fight.boss_object_type;
        if get_asset_manager().is_wandering_boss(otype)
            || crate::assets::is_invuln_finish_boss(otype)
        {
            if !fight.killed {
                match self.dormant_fights.get_mut(&otype) {
                    Some(existing) => merge_fight_into(existing, fight),
                    None => {
                        self.dormant_fights.insert(otype, fight);
                    }
                }
                return None;
            }
            // A real kill: fold in any still-suspended segment of the same
            // wandering boss so its damage is not lost, then finalize.
            let fight = match self.dormant_fights.remove(&otype) {
                Some(mut base) => {
                    merge_fight_into(&mut base, fight);
                    base
                }
                None => fight,
            };
            return self.finalize_fight(fight, time_ms);
        }

        // Ordinary boss. An un-killed despawn is suspended by object id so that
        // re-engaging the same physical boss (walked out of range and back)
        // resumes this fight instead of starting a duplicate.
        let oid = fight.boss_object_id;
        if !fight.killed {
            match self.dormant_by_id.get_mut(&oid) {
                Some(existing) => merge_fight_into(existing, fight),
                None => {
                    self.dormant_by_id.insert(oid, fight);
                }
            }
            return None;
        }
        // A kill folds in any suspended earlier segment for the same object id.
        let fight = match self.dormant_by_id.remove(&oid) {
            Some(mut base) => {
                merge_fight_into(&mut base, fight);
                base
            }
            None => fight,
        };
        self.finalize_fight(fight, time_ms)
    }

    /// Attribute an incoming `DamagePacket`: `attacker_id` dealt `amount` to
    /// `target_id`. Starts a fight when the target is a boss.
    ///
    /// When the target is another player, the packet describes *damage taken*,
    /// which is routed to [`Self::record_remote_damage_taken`] instead. The local
    /// player is never a `DamagePacket` target for its own hits (they carry
    /// "damage done to other players and enemies"), so the local id is rejected
    /// here to avoid double counting with the `PlayerHit` path.
    pub fn on_damage(&mut self, target_id: i32, attacker_id: i32, amount: i64, time_ms: i64) {
        if target_id != self.local_object_id
            && self
                .objects
                .get(&target_id)
                .is_some_and(|o| class_ids::is_player_class(o.object_type))
        {
            self.record_remote_damage_taken(target_id, attacker_id, amount);
            return;
        }
        let is_boss = self.ensure_fight(target_id, time_ms);
        let aux = if is_boss {
            None
        } else {
            self.aux_target_for(target_id)
        };
        if !is_boss && aux.is_none() {
            return;
        }
        // Remote players' summons deal damage via DamagePackets keyed to the
        // summon entity. Redirect that credit to the owning player so it is not
        // surfaced as a spurious "Unknown"/pet row. (The local player's own
        // summons never produce DamagePackets, so this only ever remaps remotes.)
        let attacker_id = match self.summon_owners.get(&attacker_id) {
            Some(&summoner) if summoner != 0 && summoner != attacker_id => summoner,
            _ => attacker_id,
        };
        let is_local = attacker_id == self.local_object_id;
        let target_guarded = self
            .objects
            .get(&target_id)
            .map(|o| o.is_o3_guarding())
            .unwrap_or(false);
        let (name, object_type, equipment, enchants, skin_id, tex1, tex2) = self
            .objects
            .get(&attacker_id)
            .map(|o| {
                (
                    o.name.clone(),
                    o.object_type,
                    o.equipment,
                    o.equipment_enchants.clone(),
                    o.skin_id,
                    o.tex1,
                    o.tex2,
                )
            })
            .unwrap_or((None, 0, [-1; 4], Default::default(), 0, 0, 0));
        let fight = match aux {
            Some(aux) => self.ensure_aux_fight(aux, time_ms),
            None => self.fights.get_mut(&target_id).expect("fight ensured"),
        };
        fight.last_activity = time_ms;
        let accum = fight.attackers.entry(attacker_id).or_default();
        accum.damage += amount;
        accum.hits += 1;
        if target_guarded {
            accum.guarded_damage += amount;
            accum.guarded_hits += 1;
        }
        accum.is_local = is_local;
        if name.is_some() {
            accum.last_name = name;
        }
        if object_type != 0 {
            accum.last_object_type = object_type;
        }
        if skin_id != 0 {
            accum.last_skin_id = skin_id;
        }
        if tex1 != 0 {
            accum.last_tex1 = tex1;
        }
        if tex2 != 0 {
            accum.last_tex2 = tex2;
        }
        let bucket = accum
            .damage_by_equipment
            .entry((equipment, enchants))
            .or_default();
        bucket.0 += amount;
        bucket.1 += 1;
    }

    /// Credit a remote player's damage taken (exact, post-defense) from a
    /// `DamagePacket` to the fight its `attacker_id` belongs to. Does not refresh
    /// the fight's activity clock (taken damage must not keep a stale fight
    /// alive) and snapshots the victim's identity so a player who only ever took
    /// damage still resolves at finalize.
    fn record_remote_damage_taken(&mut self, victim_id: i32, attacker_id: i32, amount: i64) {
        let Some(dest) = self.taken_dest(attacker_id) else {
            return;
        };
        let snap = self.identity_snapshot(victim_id);
        let accum = self.dest_attacker_mut(dest, victim_id);
        accum.damage_taken += amount;
        accum.damage_taken_hits += 1;
        apply_identity_snapshot(accum, &snap);
    }

    /// Record an enemy projectile (`EnemyShoot`), stashing its pre-defense damage
    /// and armor-piercing flag keyed by (owner, bullet id) so a later local
    /// `PlayerHit` can estimate the local player's damage taken. Multi-shot
    /// volleys carry consecutive bullet ids, so each is registered. `bullet_type`
    /// indexes the firing enemy's projectile list; armor-piercing is resolved
    /// from game assets and defaults to `false` when the enemy or projectile is
    /// unknown.
    pub fn on_enemy_shot(
        &mut self,
        owner_id: i32,
        bullet_id: i16,
        bullet_type: u8,
        damage: i16,
        num_shots: u8,
    ) {
        if damage <= 0 {
            return;
        }
        // Bound memory: enemy bullets that never hit the local player are only
        // freed on map change, so cap the buffer (an estimate can tolerate the
        // rare miss when it overflows).
        if self.pending_enemy_shots.len() > MAX_PENDING_ENEMY_SHOTS {
            self.pending_enemy_shots.clear();
        }
        let armor_piercing = self
            .objects
            .get(&owner_id)
            .and_then(|o| {
                get_asset_manager().projectile_armor_piercing(o.object_type, bullet_type as usize)
            })
            .unwrap_or(false);
        // `num_shots` is 255 when the packet omitted the optional trailing fields,
        // which denotes a single projectile (see `EnemyShootPacket` parsing).
        let shots = if num_shots == 255 {
            1
        } else {
            num_shots.max(1)
        };
        for i in 0..shots as i16 {
            let bid = bullet_id.wrapping_add(i);
            self.pending_enemy_shots
                .insert((owner_id, bid), (damage as i32, armor_piercing));
        }
    }

    /// Record the local player being hit (`PlayerHit`), estimating the damage
    /// taken from the correlated enemy projectile and the local defense. Credited
    /// only while a fight the attacker belongs to is active. Armor-piercing enemy
    /// projectiles bypass the local player's defense (resolved at `EnemyShoot`
    /// time); the flag defaults to non-piercing when the shot's asset is unknown.
    pub fn on_player_hit(&mut self, bullet_id: i16, enemy_id: i32, time_ms: i64) {
        if self.local_object_id == 0 {
            return;
        }
        // Armor of Nil procs "on being hit", so advance its proc/cooldown state
        // for every local hit -- even one we can't value or attribute -- before
        // any correlation gating.
        let armor_item = self
            .objects
            .get(&self.local_object_id)
            .map(|o| o.equipment[ARMOR_SLOT])
            .unwrap_or(-1);
        let nil_active = self.advance_armor_of_nil(armor_item, time_ms);

        let Some(dest) = self.taken_dest(enemy_id) else {
            return;
        };
        let local = self.objects.get(&self.local_object_id);
        let defense = local.map(|o| o.defense).unwrap_or(0);
        let defense_seen = local.map(|o| o.defense_seen).unwrap_or(false);
        let cond0 = local.map(|o| o.condition).unwrap_or(0);
        let cond1 = local.map(|o| o.condition_new).unwrap_or(0);
        let armor_enchants = local
            .map(|o| o.equipment_enchants[ARMOR_SLOT].clone())
            .unwrap_or_default();
        let base = self
            .pending_enemy_shots
            .get(&(enemy_id, bullet_id))
            .copied();
        let resolved = match (base, defense_seen) {
            (Some((b, armor_piercing)), true) => {
                let reductions = self.reductions_for(&armor_enchants, nil_active, time_ms);
                let (taken, blocked) = damage::damage_taken_and_blocked(
                    b,
                    armor_piercing,
                    defense,
                    cond0,
                    cond1,
                    &reductions,
                );
                Some((taken as i64, blocked as i64))
            }
            _ => None,
        };
        let local_id = self.local_object_id;
        let snap = self.identity_snapshot(local_id);
        let accum = self.dest_attacker_mut(dest, local_id);
        accum.is_local = true;
        apply_identity_snapshot(accum, &snap);
        match resolved {
            Some((dmg, blocked)) => {
                accum.damage_taken += dmg;
                accum.damage_blocked += blocked;
                accum.damage_taken_hits += 1;
            }
            None => {
                accum.unmatched_taken_hits += 1;
            }
        }
    }

    /// Reset the local player's transient mitigation proc windows (Armor of Nil,
    /// Knight shield). Called on map change / disconnect.
    fn reset_local_mitigation_windows(&mut self) {
        self.shield_dr_until = 0;
        self.shield_dr_pct = 0;
        self.nil_active_until = 0;
        self.nil_ready_at = 0;
    }

    /// Record a local `UseItem`. If the used item is a Knight shield, open its
    /// damage-reduction window. Non-shield activations are ignored.
    /// `UseItem` is an outgoing packet, so it only ever reflects the local
    /// player.
    pub fn on_use_item(&mut self, item_id: i32, _x: f32, _y: f32, time_ms: i64) {
        if self.local_object_id == 0 || item_id < 0 {
            return;
        }
        if let Some(pct) = get_asset_manager().shield_reduction_pct(item_id) {
            self.shield_dr_until = time_ms + SHIELD_ACTIVE_MS;
            self.shield_dr_pct = pct;
        }
    }

    /// Advance the local player's Armor of Nil "Planar Absorption" proc/cooldown
    /// state for a hit at `time_ms` and return whether the window is active for
    /// that hit. Called for every local hit -- even unvalued ones --
    /// because the proc triggers on being hit. A no-op unless the local player is
    /// wearing Armor of Nil.
    fn advance_armor_of_nil(&mut self, armor_item: i32, time_ms: i64) -> bool {
        if armor_item != ARMOR_OF_NIL_ITEM_ID {
            return false;
        }
        if time_ms < self.nil_active_until {
            true
        } else if time_ms >= self.nil_ready_at {
            // A fresh proc; it also mitigates the triggering hit.
            self.nil_active_until = time_ms + NIL_ACTIVE_MS;
            self.nil_ready_at = time_ms + NIL_COOLDOWN_MS;
            true
        } else {
            false
        }
    }

    /// Build the post-defense damage reductions active for the local player at
    /// `time_ms`. `armor_enchants` are the local player's armor-slot
    /// enchant ids; `nil_active` is the Armor of Nil window state resolved by
    /// [`Self::advance_armor_of_nil`]; the Knight shield window is maintained via
    /// [`Self::on_use_item`]. Pure w.r.t. proc state.
    fn reductions_for(
        &self,
        armor_enchants: &[u16],
        nil_active: bool,
        time_ms: i64,
    ) -> damage::DamageReductions {
        let enchant_pct = armor_enchants
            .iter()
            .map(|&e| damage::damage_resistance_pct(e))
            .max()
            .unwrap_or(0);
        let shield_pct = if time_ms < self.shield_dr_until {
            self.shield_dr_pct
        } else {
            0
        };
        damage::DamageReductions {
            enchant_pct,
            armor_of_nil: nil_active,
            shield_pct,
        }
    }

    /// Resolve which active fight an incoming hit from `attacker_id` belongs to,
    /// using a conservative single-destination rule so one hit is never split or
    /// duplicated across fights:
    /// 1. the attacker is itself a boss with a live (non-killed) fight;
    /// 2. the attacker IS a curated aux entity with an active aux fight;
    /// 3. an otherwise-unknown minion is credited only when exactly one
    ///    non-killed boss fight is active (else the hit is dropped as ambiguous).
    fn taken_dest(&self, attacker_id: i32) -> Option<TakenDest> {
        if let Some(f) = self.fights.get(&attacker_id) {
            if !f.killed {
                return Some(TakenDest::Boss(attacker_id));
            }
        }
        if let Some(aux) = self.aux_target_for(attacker_id) {
            if self.aux_fights.contains_key(aux.category) {
                return Some(TakenDest::Aux(aux.category));
            }
        }
        let mut live = self
            .fights
            .iter()
            .filter(|(_, f)| !f.killed)
            .map(|(&id, _)| id);
        let first = live.next()?;
        if live.next().is_none() {
            Some(TakenDest::Boss(first))
        } else {
            None
        }
    }

    /// Mutable accessor for a destination fight's participant accumulator. The
    /// destination was produced by [`Self::taken_dest`], so the fight exists.
    fn dest_attacker_mut(&mut self, dest: TakenDest, victim_id: i32) -> &mut ParticipantAccum {
        let fight = match dest {
            TakenDest::Boss(id) => self.fights.get_mut(&id).expect("dest boss fight exists"),
            TakenDest::Aux(cat) => self.aux_fights.get_mut(cat).expect("dest aux fight exists"),
        };
        fight.attackers.entry(victim_id).or_default()
    }

    /// Snapshot a participant's current display identity from tracked object
    /// state, for participants that may despawn before finalize.
    fn identity_snapshot(&self, id: i32) -> IdentitySnapshot {
        match self.objects.get(&id) {
            Some(o) => IdentitySnapshot {
                name: o.name.clone(),
                object_type: o.object_type,
                skin_id: o.skin_id,
                tex1: o.tex1,
                tex2: o.tex2,
            },
            None => IdentitySnapshot::default(),
        }
    }

    /// Record a local `PlayerShoot`: roll and store the shot's pre-defense
    /// damage, keyed by bullet id, for later correlation with an `EnemyHit`.
    ///
    /// Must be called for EVERY local shot in packet order (not gated on an
    /// active fight) so the damage RNG stays in lockstep with the client. If the
    /// RNG is not seeded yet, or the weapon's projectile data is unavailable, we
    /// cannot know whether/how to advance the RNG, so self-damage is disabled for
    /// the rest of the map rather than risk a silent desync.
    pub fn on_local_shot(
        &mut self,
        weapon_id: i32,
        projectile_id: i32,
        bullet_id: i16,
        client_time: i32,
        shot_x: f32,
        shot_y: f32,
        angle: f32,
        time_ms: i64,
    ) {
        // This weapon bullet id supersedes any stale local server-shot entry for
        // the same id (newest registration owns an id). Done before the guards so
        // the maps stay disjoint even when self-damage is disabled for this map.
        self.pending_server_shots.remove(&bullet_id);
        // A multi-projectile weapon fires several bullets in one trigger, all
        // stamped with the same client time and weapon. The first is the primary
        // projectile; the rest are secondaries. Tracked across the guards so the
        // classification stays correct even for shots that don't produce a
        // PendingShot.
        let trigger = (client_time, weapon_id);
        let extra_projectile = self.last_shot_trigger == Some(trigger);
        self.last_shot_trigger = Some(trigger);
        if !self.self_damage_available {
            return;
        }
        if !self.rng_seeded {
            self.self_damage_available = false;
            return;
        }
        // The local player's stats must be known to compute the attack multiplier.
        let Some(local) = self.objects.get(&self.local_object_id) else {
            self.self_damage_available = false;
            return;
        };
        if !local.attack_seen {
            self.self_damage_available = false;
            return;
        }
        let atk = local.attacker_stats();
        let lethal_strike = self.lethal_strike_snapshot();

        let proj_index = if projectile_id < 0 {
            0
        } else {
            projectile_id as usize
        };
        let resolved = get_asset_manager().weapon_projectile(weapon_id, proj_index);

        let Some((min, max, ap, slot)) = resolved else {
            // Unknown projectile data: cannot determine whether this shot advances
            // the RNG, so all later rolls would be unreliable.
            self.self_damage_available = false;
            return;
        };

        // Weapon-damage enchant multipliers apply only when the shot's weapon is
        // the one currently equipped in the weapon slot; a stat update lag or a
        // mid-fight swap would otherwise pair one weapon's shot with another's
        // enchants. Unenchanted (or mismatched) weapons roll at (1.0, 1.0), i.e.
        // exactly as before.
        let (min_mult, max_mult) = if weapon_id == local.equipment[WEAPON_SLOT] {
            get_asset_manager()
                .weapon_damage_enchant_mult(&local.equipment_enchants[WEAPON_SLOT], proj_index)
        } else {
            (1.0, 1.0)
        };

        let base_damage =
            damage::roll_base_damage(&mut self.rng, min, max, min_mult, max_mult, slot, &atk);
        self.pending_shots.insert(
            bullet_id,
            PendingShot {
                base_damage,
                armor_piercing: ap,
                shot_time: time_ms,
                lethal_strike,
                procs_credited: false,
                extra_projectile,
                shot_geo: Some((shot_x, shot_y, angle)),
            },
        );
    }

    /// Snapshot the local player's Lethal Strike state at shoot time: the
    /// equipped cloak's params + effective WIS. `None` unless the ability slot
    /// holds a cloak that grants Lethal Strike and a base Wisdom stat has been
    /// observed (so non-rogues and unknown-WIS shots are unaffected).
    fn lethal_strike_snapshot(&self) -> Option<(LethalStrikeParams, i32)> {
        let local = self.objects.get(&self.local_object_id)?;
        let cloak = local.equipment[ABILITY_SLOT];
        if cloak < 0 {
            return None;
        }
        let params = get_asset_manager().lethal_strike_params(cloak)?;
        let stat_val = local.effective_scaling_stat(params.scaling_stat)?;
        Some((params, stat_val))
    }

    /// Whether `shot_time` falls inside any recorded Lethal Strike window.
    fn in_lethal_strike_window(&self, shot_time: i64) -> bool {
        self.lethal_strike_windows
            .iter()
            .any(|&(start, end)| shot_time >= start && shot_time <= end)
    }

    /// Open a Lethal Strike window ending 2.4s after the de-cloak, back-dated by
    /// a small guard so the shot that canceled sneak is still credited. Keeps the
    /// window list small and prunes stale entries.
    fn open_lethal_strike_window(&mut self, time_ms: i64) {
        let start = time_ms - LETHAL_STRIKE_GUARD_MS;
        let end = time_ms + LETHAL_STRIKE_WINDOW_MS;
        self.lethal_strike_windows.push_back((start, end));
        self.lethal_strike_windows
            .retain(|&(_, w_end)| w_end >= time_ms - 10_000);
        while self.lethal_strike_windows.len() > 8 {
            self.lethal_strike_windows.pop_front();
        }
    }

    /// Record an incoming `ServerPlayerShoot`. Learns the summon->owner mapping
    /// for every shot (used to redirect remote summon `DamagePacket`s), stashes
    /// the server-precomputed pre-defense damage for the local player's OWN
    /// summons, and registers the local player's OWN server-fired bullets (weapon
    /// procs, ability volleys/secondary shots) so a later `EnemyHit` can resolve
    /// them. `container_type` is the firing item and `bullet_type` its projectile
    /// index, used to look up the projectile's armor-piercing flag.
    pub fn on_ally_shot(
        &mut self,
        owner_id: i32,
        summoner_id: i32,
        bullet_id: i16,
        bullet_count: i8,
        damage: i16,
        container_type: i32,
        bullet_type: i8,
    ) {
        // Learn ownership only for genuine summons (a normal player's own shot
        // reports summoner 0 or itself, and must not be remapped).
        if summoner_id != 0 && summoner_id != owner_id {
            self.summon_owners.insert(owner_id, summoner_id);
        }

        let count = bullet_count.max(1) as i16;

        // The local player's OWN server-fired bullets arrive with owner == local
        // (summoner 0). The client confirms each with an outgoing EnemyHit, but
        // their damage is only in this packet, so register it for on_local_hit.
        // The armor-piercing flag comes from the projectile's asset data; if it
        // can't be resolved we skip registration so the hit stays SelfPartial
        // rather than claim an exact (possibly wrong) value.
        if owner_id == self.local_object_id && self.local_object_id != 0 {
            // This packet claims these bullet ids for a local server shot, so
            // enforce newest-wins: evict any stale entry for the same id from
            // both maps first, then register only when damage and the AP flag
            // are known. Evicting even when unresolved prevents a recycled id
            // from resolving against an older, unrelated shot.
            let resolved = if damage > 0 {
                let proj_index = if bullet_type < 0 {
                    0
                } else {
                    bullet_type as usize
                };
                get_asset_manager()
                    .weapon_projectile(container_type, proj_index)
                    .map(|(_, _, ap, _)| ap)
            } else {
                None
            };
            for i in 0..count {
                let bid = bullet_id.wrapping_add(i);
                self.pending_shots.remove(&bid);
                match resolved {
                    Some(ap) => {
                        self.pending_server_shots.insert(bid, (damage as i32, ap));
                    }
                    None => {
                        self.pending_server_shots.remove(&bid);
                    }
                }
            }
        }

        // Only reconstruct damage for the local player's own summons; everyone
        // else's summon damage arrives via DamagePacket.
        if summoner_id != self.local_object_id || self.local_object_id == 0 {
            return;
        }
        // A multi-projectile shot emits consecutive bullet ids; register each so
        // any of them can correlate to its EnemyHit.
        for i in 0..count {
            let bid = bullet_id.wrapping_add(i);
            self.pending_summon_shots
                .insert((owner_id, bid), damage as i32);
        }
    }

    /// Record a local `EnemyHit` on `target_id` by bullet `bullet_id`. The local
    /// player's own damage is not in `DamagePacket`s; we correlate the bullet to
    /// its source shot and apply the target's live defense here. `shooter_id` is
    /// the firing entity (the local player for a weapon shot, or one of their
    /// summons); `main_id` is the owning player. A bullet may hit several targets
    /// (piercing), so the pending shot is not consumed.
    pub fn on_local_hit(
        &mut self,
        target_id: i32,
        bullet_id: i16,
        shooter_id: i32,
        main_id: i32,
        time_ms: i64,
    ) {
        if self.local_object_id == 0 {
            return;
        }
        let is_boss = self.ensure_fight(target_id, time_ms);
        let aux = if is_boss {
            None
        } else {
            self.aux_target_for(target_id)
        };
        if !is_boss && aux.is_none() {
            return;
        }
        // The client only sends EnemyHit for bullets it owns (its weapon or its
        // summons), so main_id should be the local player; ignore anything else.
        if main_id != self.local_object_id {
            return;
        }

        let target = self.objects.get(&target_id);
        // Enemy DEF is not broadcast in packets (only players send it), so fall
        // back to the object's base defense from game assets when unseen. Live
        // condition bits (Armor Broken / Exposed / Armored) still adjust it in
        // `damage_with_defense`.
        let defense = target
            .map(|o| {
                if o.defense_seen {
                    o.defense
                } else {
                    get_asset_manager()
                        .object_defense(o.object_type)
                        .unwrap_or(0)
                }
            })
            .unwrap_or(0);
        let cond0 = target.map(|o| o.condition).unwrap_or(0);
        let cond1 = target.map(|o| o.condition_new).unwrap_or(0);
        let target_guarded = target.map(|o| o.is_o3_guarding()).unwrap_or(false);

        // A hit that lands while the target is Invulnerable deals no damage, and
        // the server never broadcasts a DamagePacket for it, so observers never
        // count it. Counting it locally would inflate the hit total, so drop it
        // entirely (no hit, no attacker entry). Keep the fight's activity clock
        // fresh so an invulnerable phase doesn't time the encounter out.
        if cond0 & COND_INVULNERABLE != 0 {
            let fight = match aux {
                Some(aux) => self.ensure_aux_fight(aux, time_ms),
                None => self.fights.get_mut(&target_id).expect("fight ensured"),
            };
            fight.last_activity = time_ms;
            return;
        }

        let resolved = if shooter_id == self.local_object_id {
            // Weapon shot: damage was rolled from the map RNG at shoot time.
            let weapon_resolved = if self.self_damage_available {
                self.pending_shots.get(&bullet_id).copied().map(|shot| {
                    let base = damage::damage_with_defense(
                        shot.base_damage,
                        shot.armor_piercing,
                        defense,
                        cond0,
                        cond1,
                    );
                    let mut dmg = base;
                    // Rogue Lethal Strike: a shot fired during the 2.4s
                    // post-decloak window deals a flat + %DEF bonus, and spawns
                    // 2 server-side side procs that each deal that same bonus.
                    // The side procs are invisible (no PlayerShoot or EnemyHit on
                    // the client), so add all of it here. Skip Invulnerable targets.
                    if cond0 & COND_INVULNERABLE == 0 {
                        if let Some((params, wis)) = shot.lethal_strike {
                            if self.in_lethal_strike_window(shot.shot_time) {
                                let bonus = damage::lethal_strike_bonus(&params, wis, defense);
                                // Main-shot bonus: once per resolved hit (so a
                                // piercing bullet's several hits each get it).
                                dmg += bonus as i32;
                                // Side procs: fire once per trigger, so credit
                                // them on the first hit only to avoid piercing
                                // duplication, and only on the primary projectile
                                // so a multi-projectile weapon does not credit the
                                // two procs once per projectile.
                                if !shot.procs_credited && !shot.extra_projectile {
                                    // Each side proc spawns 4.6 tiles from the
                                    // player along the shot angle +/- 9 degrees
                                    // and flies outward, so a proc only lands if
                                    // its trajectory reaches the target hitbox.
                                    // With the shot's true origin and angle we
                                    // credit only the procs that geometrically
                                    // reach the target; without them (older
                                    // shots, tests) we credit both.
                                    let proc_hits = match shot.shot_geo {
                                        Some((sx, sy, angle)) => self
                                            .objects
                                            .get(&target_id)
                                            .map(|t| {
                                                let scale = get_asset_manager()
                                                    .enemy_hitbox_scale(t.object_type);
                                                let radius =
                                                    LS_ENEMY_BASE_RADIUS * scale + LS_PROC_RADIUS;
                                                ls_proc_hits(
                                                    sx, sy, angle, t.last_x, t.last_y, radius,
                                                )
                                            })
                                            .unwrap_or(2),
                                        None => 2,
                                    };
                                    dmg += (proc_hits as i64 * bonus) as i32;
                                    if let Some(s) = self.pending_shots.get_mut(&bullet_id) {
                                        s.procs_credited = true;
                                    }
                                }
                            }
                        }
                    }
                    dmg
                })
            } else {
                None
            };
            // Fall back to the local player's own server-fired bullets (weapon
            // procs, ability volleys). Their pre-defense damage came from the
            // ServerPlayerShoot packet; apply the target's live defense with the
            // projectile's armor-piercing flag. Ids are disjoint from
            // pending_shots, so this only resolves genuine server shots.
            weapon_resolved.or_else(|| {
                self.pending_server_shots
                    .get(&bullet_id)
                    .map(|&(base, ap)| damage::damage_with_defense(base, ap, defense, cond0, cond1))
            })
        } else {
            // Summon shot: damage is the server-provided projectile value; apply
            // the target's defense. Summon projectiles are treated as non-armor-
            // piercing (an approximation for the rare AP-summon case).
            self.pending_summon_shots
                .get(&(shooter_id, bullet_id))
                .map(|&base| damage::damage_with_defense(base, false, defense, cond0, cond1))
        };

        let local_id = self.local_object_id;
        let (name, object_type, equipment, enchants, skin_id, tex1, tex2) = self
            .objects
            .get(&local_id)
            .map(|o| {
                (
                    o.name.clone(),
                    o.object_type,
                    o.equipment,
                    o.equipment_enchants.clone(),
                    o.skin_id,
                    o.tex1,
                    o.tex2,
                )
            })
            .unwrap_or((None, 0, [-1; 4], Default::default(), 0, 0, 0));
        let fight = match aux {
            Some(aux) => self.ensure_aux_fight(aux, time_ms),
            None => self.fights.get_mut(&target_id).expect("fight ensured"),
        };
        fight.last_activity = time_ms;
        let accum = fight.attackers.entry(local_id).or_default();
        accum.hits += 1;
        accum.is_local = true;
        if target_guarded {
            accum.guarded_hits += 1;
        }
        if name.is_some() {
            accum.last_name = name;
        }
        if object_type != 0 {
            accum.last_object_type = object_type;
        }
        if skin_id != 0 {
            accum.last_skin_id = skin_id;
        }
        if tex1 != 0 {
            accum.last_tex1 = tex1;
        }
        if tex2 != 0 {
            accum.last_tex2 = tex2;
        }
        let dmg = match resolved {
            Some(dmg) => {
                accum.damage += dmg as i64;
                if target_guarded {
                    accum.guarded_damage += dmg as i64;
                }
                dmg as i64
            }
            None => {
                accum.unresolved_local_hits += 1;
                0
            }
        };
        let bucket = accum
            .damage_by_equipment
            .entry((equipment, enchants))
            .or_default();
        bucket.0 += dmg;
        bucket.1 += 1;
    }

    /// Periodic tick: finalize fights whose boss died or timed out.
    pub fn on_tick(&mut self, time_ms: i64) -> Vec<CompletedFight> {
        let mut finished = std::mem::take(&mut self.deferred_finished);

        // Give up on suspended ordinary bosses not seen again within the resume
        // window: finalize them as escaped so they surface in history.
        let expired: Vec<i32> = self
            .dormant_by_id
            .iter()
            .filter(|(_, f)| time_ms - f.last_activity >= BOSS_RESUME_WINDOW_MS)
            .map(|(id, _)| *id)
            .collect();
        for id in expired {
            if let Some(fight) = self.dormant_by_id.remove(&id) {
                if let Some(cf) = self.finalize_fight(fight, time_ms) {
                    finished.push(cf);
                }
            }
        }

        let done: Vec<i32> = self
            .fights
            .iter()
            .filter(|(_, f)| f.killed || (time_ms - f.last_activity) >= FIGHT_TIMEOUT_MS)
            .map(|(id, _)| *id)
            .collect();

        for id in done {
            if let Some(fight) = self.fights.remove(&id) {
                // Route through suspend_or_finalize so a killed wandering boss
                // still folds in any suspended segment, and a wandering boss that
                // timed out (missed removal packet) is suspended, not duplicated.
                if let Some(cf) = self.suspend_or_finalize(fight, time_ms) {
                    finished.push(cf);
                }
            }
        }
        finished
    }

    /// Ensure a fight exists for `target_id` if it is a known boss. Returns true
    /// when a fight is active for the target.
    fn ensure_fight(&mut self, target_id: i32, time_ms: i64) -> bool {
        if self.fights.contains_key(&target_id) {
            return true;
        }
        let Some(obj) = self.objects.get(&target_id) else {
            return false;
        };
        if !obj.is_boss() {
            return false;
        }
        let object_type = obj.object_type;
        let object_max_hp = obj.max_hp;
        let start_hp = if obj.hp > 0 { obj.hp } else { obj.max_hp };

        // A Moonlight Village boss already completed AND recorded this run must
        // not spawn a second fight from lingering fire on the now-invulnerable
        // object, which would duplicate the Completed record. The
        // first fight of the type is still allowed (it may be created lazily on
        // its first hit after the marker already fired).
        if self.completed_recorded_types.contains(&object_type) {
            return false;
        }

        // A curated wandering boss re-entering view: resume its suspended fight
        // under the new object id instead of starting a fresh one, so a boss that
        // roams in and out of range stays a single fight for the whole run.
        if let Some(mut dormant) = self.dormant_fights.remove(&object_type) {
            dormant.boss_object_id = target_id;
            dormant.last_activity = time_ms;
            dormant.min_hp_seen = dormant.min_hp_seen.min(start_hp as i64);
            self.fights.insert(target_id, dormant);
            return true;
        }

        // An ordinary boss re-entering view keeps its object id, so a suspended
        // fight under this id resumes here (walked out of range and back). Only
        // resume when the re-appearing object is the same boss type; a server that
        // recycled the id for a different object evicts the stale segment as
        // escaped and starts fresh below.
        match self.dormant_by_id.remove(&target_id) {
            Some(mut dormant) if dormant.boss_object_type == object_type => {
                dormant.last_activity = time_ms;
                dormant.min_hp_seen = dormant.min_hp_seen.min(start_hp as i64);
                self.fights.insert(target_id, dormant);
                return true;
            }
            Some(stale) => {
                if let Some(cf) = self.finalize_fight(stale, time_ms) {
                    self.deferred_finished.push(cf);
                }
            }
            None => {}
        }

        // Guard against multi-segment bosses: a hydra-style boss (e.g. Adult
        // Baneserpent) exposes several damageable "head" objects that resolve to
        // the same display name as the real boss body but carry no boss label and
        // only pass the HP fallback. Demote such a segment when the labeled boss
        // body is concurrently present, otherwise every head spawns a duplicate
        // fight ("Adult Baneserpent" x3). The labeled body itself is unaffected.
        let am = get_asset_manager();
        if !am.is_boss_like(object_type) {
            if let Some(name) = am.object_name(object_type) {
                let is_boss_segment = self.objects.values().any(|other| {
                    other.object_type != object_type
                        && am.is_boss_like(other.object_type)
                        && am.object_name(other.object_type).as_deref() == Some(name.as_str())
                });
                if is_boss_segment {
                    return false;
                }
            }
        }

        let carried_close_calls = std::mem::take(&mut self.run_pending_fight_close_calls);
        self.fights.insert(
            target_id,
            FightState {
                boss_object_id: target_id,
                boss_object_type: object_type,
                boss_max_hp: object_max_hp,
                boss_start_hp: start_hp,
                started_at: time_ms,
                last_activity: time_ms,
                killed: false,
                min_hp_seen: start_hp as i64,
                local_char_id: self.local_char_id,
                local_close_calls: carried_close_calls,
                attackers: HashMap::new(),
                // Bosses that revive with a fresh HP pool start as "Pre-survival"
                // so the heal-back can split them; all others stay unlabeled.
                segment_label: crate::assets::is_second_coming_boss(object_type)
                    .then_some("Pre-survival"),
                display_name_override: None,
                split_done: false,
                // First sight below full HP means the fight was already underway
                // when the local player arrived.
                joined_late: object_max_hp > 0 && start_hp < object_max_hp,
            },
        );
        true
    }

    /// Classify a damaged object as an aggregated summary target (Marble Core /
    /// Pillar), if its type is curated as one.
    fn aux_target_for(&self, target_id: i32) -> Option<crate::assets::AuxTarget> {
        let otype = self.objects.get(&target_id)?.object_type;
        crate::assets::aux_target_for_type(otype)
    }

    /// Record a seen aux-category member instance (spawn or stat update), keeping
    /// the highest max HP observed for its object id so the aggregated row can sum
    /// every instance's HP and count distinct instances.
    fn note_aux_instance(&mut self, object_id: i32, object_type: i32, max_hp: i32) {
        if let Some(aux) = crate::assets::aux_target_for_type(object_type) {
            let slot = self
                .aux_instances
                .entry(aux.category)
                .or_default()
                .max_hp_by_id
                .entry(object_id)
                .or_insert(0);
            *slot = (*slot).max(max_hp);
        }
    }

    /// Get or create the run-scoped aggregate fight for an aux category.
    fn ensure_aux_fight(&mut self, aux: crate::assets::AuxTarget, time_ms: i64) -> &mut FightState {
        let local_char_id = self.local_char_id;
        self.aux_fights
            .entry(aux.category)
            .or_insert_with(|| FightState {
                boss_object_id: 0,
                boss_object_type: aux.repr_type,
                boss_max_hp: 0,
                boss_start_hp: 0,
                started_at: time_ms,
                last_activity: time_ms,
                killed: false,
                min_hp_seen: 0,
                local_char_id,
                local_close_calls: 0,
                attackers: HashMap::new(),
                segment_label: None,
                display_name_override: Some(aux.display_name),
                split_done: false,
                joined_late: false,
            })
    }

    /// Merge every live and suspended fight of `boss_type` into one, mark it a
    /// kill, and finalize it. Called when a completion marker clears a Moonlight
    /// Village invulnerable-finish boss so a run that re-detected the boss under
    /// many object ids yields a single record rather than one per id. Returns the record, or `None` when no engaged fight of the type is
    /// present.
    fn consolidate_completed_boss(
        &mut self,
        boss_type: i32,
        time_ms: i64,
    ) -> Option<CompletedFight> {
        let mut merged: Option<FightState> = self.dormant_fights.remove(&boss_type);
        let live_ids: Vec<i32> = self
            .fights
            .iter()
            .filter(|(_, f)| f.boss_object_type == boss_type)
            .map(|(&id, _)| id)
            .collect();
        let dorm_ids: Vec<i32> = self
            .dormant_by_id
            .iter()
            .filter(|(_, f)| f.boss_object_type == boss_type)
            .map(|(&id, _)| id)
            .collect();
        for id in live_ids.into_iter().chain(dorm_ids) {
            let seg = self
                .fights
                .remove(&id)
                .or_else(|| self.dormant_by_id.remove(&id));
            if let Some(seg) = seg {
                match &mut merged {
                    Some(base) => merge_fight_into(base, seg),
                    None => merged = Some(seg),
                }
            }
        }
        let mut fight = merged?;
        fight.killed = true;
        self.finalize_fight(fight, time_ms)
    }

    /// Finalize every active fight plus the run's aux summaries (map change /
    /// disconnect).
    fn finalize_all(&mut self, time_ms: i64) -> Vec<CompletedFight> {
        let mut fights: Vec<FightState> = self.fights.drain().map(|(_, f)| f).collect();
        // Fold each still-suspended wandering boss into a concurrent live fight of
        // the same type (defends against the rare case of two instances in view at
        // once); finalize it standalone as an escaped fight when none is active.
        for (_, seg) in self.dormant_fights.drain() {
            match fights
                .iter_mut()
                .find(|f| f.boss_object_type == seg.boss_object_type)
            {
                Some(live) => merge_fight_into(live, seg),
                None => fights.push(seg),
            }
        }
        // Ordinary suspended bosses each finalize as their own escaped fight (the
        // live fights were just drained, so no same-id fight can remain).
        for (_, seg) in self.dormant_by_id.drain() {
            fights.push(seg);
        }
        fights.extend(self.aux_fights.drain().map(|(_, f)| f));
        let mut finished = std::mem::take(&mut self.deferred_finished);
        for fight in fights {
            if let Some(cf) = self.finalize_fight(fight, time_ms) {
                finished.push(cf);
            }
        }
        finished
    }

    /// Finalize every aggregated aux fight belonging to the just-killed boss's
    /// curated encounter, queueing each as a deferred completed fight so its row
    /// surfaces on the boss card at kill time instead of only at map change.
    /// Aux fights resolve to their encounter through the repr type
    /// they carry as `boss_object_type`, the same key the boss anchor resolves
    /// through, so a shared encounter id links an add to its boss.
    fn finalize_encounter_aux_fights(&mut self, boss_type: i32, time_ms: i64) {
        let Some(enc) = crate::assets::encounter_for_boss_type(boss_type) else {
            return;
        };
        let cats: Vec<&'static str> = self
            .aux_fights
            .iter()
            .filter(|(_, f)| {
                crate::assets::encounter_for_boss_type(f.boss_object_type)
                    .map(|e| e.id == enc.id)
                    .unwrap_or(false)
            })
            .map(|(&cat, _)| cat)
            .collect();
        for cat in cats {
            if let Some(aux) = self.aux_fights.remove(cat) {
                if let Some(cf) = self.finalize_fight(aux, time_ms) {
                    self.deferred_finished.push(cf);
                }
            }
        }
    }

    /// Turn a `FightState` into a `CompletedFight`, resolving participant identity
    /// and equipment from tracked objects. Returns `None` for empty fights (no
    /// damage and not killed) to avoid persisting noise.
    fn finalize_fight(&mut self, fight: FightState, time_ms: i64) -> Option<CompletedFight> {
        let mut fight = fight;
        // A Moonlight Village invulnerable-finish boss (dancers / Umi) never
        // reaches 0 HP; a completion marker seen this run scores it as a kill so
        // it shows Completed rather than Escaped. Record the type so
        // a re-touched invulnerable object cannot spawn a duplicate completed run.
        if self.completed_boss_types.contains(&fight.boss_object_type) {
            fight.killed = true;
            self.completed_recorded_types.insert(fight.boss_object_type);
        }
        let total_damage: i64 = fight.attackers.values().map(|a| a.damage).sum();
        // Drop noise: an un-killed encounter with no recorded damage (e.g. invincible
        // adds like Book Bombs that we merely shot at) is not a real fight.
        if !fight.killed && total_damage == 0 {
            return None;
        }

        // A curated boss's kill ends its aggregated adds too: finalize their aux
        // rows now so they surface on the boss card at kill time rather than only
        // when the map changes. Runs before the local-engagement
        // gate so an add the player fought is not stranded when the boss itself
        // was tagged by others. Guards: the aux-target check stops the aux fights
        // recursing into this hook, and the "Pre-survival" check skips the Marble
        // Colossus phase split (a killed segment that is not the real death, which
        // would otherwise flush the run's aggregates early and duplicate them).
        if fight.killed
            && fight.segment_label != Some("Pre-survival")
            && crate::assets::aux_target_for_type(fight.boss_object_type).is_none()
        {
            self.finalize_encounter_aux_fights(fight.boss_object_type, time_ms);
        }

        // For an aggregated aux fight, pull this category's run-scoped instance
        // tally: the distinct member count and their summed max HP replace the
        // aux fight's placeholder 0/0 HP so the row reads e.g. "Spectral Key x6
        // HP 12000/12000". Removing the accumulator prevents a later map-change
        // finalize from double-counting an already-flushed category.
        let aux_totals = crate::assets::aux_target_for_type(fight.boss_object_type)
            .and_then(|aux| self.aux_instances.remove(aux.category))
            .filter(|acc| acc.count() > 0)
            .map(|acc| (acc.count(), acc.total_hp()));

        // Only record fights the local player actually took part in. Engagement
        // is any landed hit (weapon or summon) or attributed damage from the
        // local player; a fight we merely teleported past / others fought (e.g.
        // a realm event boss like Ravenous Rot) is not our combat history.
        // Landed hits (not just damage) count, so a solo fight whose damage could
        // not be resolved (SelfPending) is still kept.
        let local_engaged = fight.attackers.iter().any(|(&aid, a)| {
            (a.is_local || aid == self.local_object_id) && (a.hits > 0 || a.damage > 0)
        });
        if !local_engaged {
            return None;
        }

        let assets = get_asset_manager();
        // Aggregated aux fights carry an explicit name ("Marble Core"); real
        // bosses resolve theirs from assets and may carry a segment suffix
        // ("Marble Colossus (Post-survival)").
        let boss_name = match fight.display_name_override {
            Some(name) => name.to_string(),
            None => {
                let base = assets
                    .object_name(fight.boss_object_type)
                    .unwrap_or_else(|| format!("Boss 0x{:04X}", fight.boss_object_type as u16));
                match fight.segment_label {
                    Some(label) => format!("{base} ({label})"),
                    None => base,
                }
            }
        };

        let dungeon = normalize_dungeon(&self.current_map);
        // Some bosses fought from the realm belong to a rated dungeon the raw
        // map name doesn't reflect (Oryx the Mad God 2 -> Wine Cellar). Remap so
        // the fight classifies by that dungeon's difficulty and shows its card.
        let dungeon = crate::assets::canonical_dungeon_for_boss(fight.boss_object_type)
            .map(str::to_string)
            .unwrap_or(dungeon);

        // Group every boss of a dungeon instance under one card. Curated bosses
        // keep their encounter id (so aux repr types still resolve); non-curated
        // bosses use the "dungeon_run" sentinel. Curated realm-event encounters
        // (Astral Rift, Hermit God, The Plague Doctor) still fold their boss and
        // aggregated adds into one card even in the Realm; the run id is scoped
        // per encounter id so distinct events in one realm visit stay separate.
        let curated_enc = crate::assets::encounter_for_boss_type(fight.boss_object_type);
        let (encounter_id, encounter_run_id) = if is_groupable_dungeon(&dungeon) {
            let enc_id = curated_enc
                .map(|e| e.id.to_string())
                .unwrap_or_else(|| "dungeon_run".to_string());
            (Some(enc_id), self.current_run_id.clone())
        } else if let Some(enc) =
            curated_enc.filter(|e| crate::assets::encounter_realm_grouped(e.id))
        {
            let run = self
                .current_run_id
                .as_ref()
                .map(|r| format!("{r}-{}", enc.id));
            (Some(enc.id.to_string()), run)
        } else {
            (None, None)
        };

        // The local player joined after the fight began when the boss was already
        // damaged on first sight; ally damage dealt before then was never seen.
        let joined_late = fight.joined_late;
        let mut participants: Vec<FightParticipant> = fight
            .attackers
            .iter()
            .map(|(&aid, accum)| self.build_participant(aid, accum, joined_late))
            .collect();
        drop_unresolved(&mut participants);
        participants.sort_by(|a, b| b.damage.cmp(&a.damage).then(b.hits.cmp(&a.hits)));

        // Solo server-echo reconciliation: the server re-sends the local player's
        // own hits as damage packets with the bullet owner masked (a sentinel id),
        // giving an exact direct-damage total the shot simulation only
        // approximates. In an effectively-solo fight that masked total is entirely
        // the local player's, so raise the local figure to it. Runs before overkill
        // trimming so an over-full total is scaled with everyone else.
        let masked_is_local = self.reconcile_masked_local(&fight, &mut participants);

        // Trim overkill: when the boss is killed, the sum of attributed damage
        // must not exceed the HP it actually lost (its start HP). The final
        // volley routinely lands several projectiles in the death tick that each
        // roll full damage, overshooting the HP pool. Scale participant damage
        // down proportionally so shares are preserved and the total matches the
        // boss HP (exact for a solo kill). Only shrinks totals, never inflates,
        // so it is a no-op for crowded fights where we saw less than the full HP.
        if fight.killed {
            cap_overkill(&mut participants, fight.boss_start_hp as i64);
        }

        // Solo HP-gap reconciliation: in an effectively-solo, fully-observed kill,
        // 100% of the boss's HP loss is the local player's damage. Server-fired
        // ability / summon projectiles never arrive in any packet, so they show
        // only as boss HP dropping; credit that unaccounted remainder to the local
        // player so the total matches the boss HP exactly. Deliberately narrow --
        // every gate below must hold, and any doubt leaves the figure as its
        // packet-derived lower bound (SelfComputed / SelfPartial).
        self.reconcile_solo_gap(
            &fight,
            &mut participants,
            is_groupable_dungeon(&dungeon),
            masked_is_local,
        );

        // Damage-changing reconciliation above can move the local player past
        // other participants, so restore the descending order for display.
        participants.sort_by(|a, b| b.damage.cmp(&a.damage).then(b.hits.cmp(&a.hits)));

        // Resolve per-participant death/nexus status. This is a
        // separate mutable pass because grave correlation consumes buffered
        // spawns, which `build_participant` (immutable `&self`) cannot do. The
        // local player left this boss unfinished when the map change / disconnect
        // that triggered this finalize found the fight still open.
        let local_left = self.local_departing && !fight.killed;
        self.resolve_end_statuses(
            &mut participants,
            fight.started_at,
            time_ms,
            fight.local_char_id,
            local_left,
        );

        // Attach each participant's pet, matched by proximity votes.
        self.assign_pets(&mut participants);

        let cf = CompletedFight {
            started_at: fight.started_at,
            ended_at: time_ms,
            dungeon: dungeon.clone(),
            dungeon_entered_at: self.dungeon_entered_at,
            map_seed: self.current_seed,
            boss_object_type: fight.boss_object_type,
            boss_name,
            boss_max_hp: aux_totals.map(|(_, hp)| hp).unwrap_or(fight.boss_max_hp),
            boss_start_hp: aux_totals.map(|(_, hp)| hp).unwrap_or(fight.boss_start_hp),
            local_object_id: self.local_object_id,
            local_char_id: fight.local_char_id,
            killed: fight.killed,
            reached_zero: fight.min_hp_seen <= 0,
            joined_late: fight.joined_late,
            encounter_id,
            encounter_run_id,
            local_close_calls: fight.local_close_calls,
            aux_member_count: aux_totals.and_then(|(count, _)| {
                let hide = crate::assets::aux_target_for_type(fight.boss_object_type)
                    .map_or(false, |a| {
                        crate::assets::aux_category_hides_count(a.category)
                    });
                if hide {
                    None
                } else {
                    Some(count)
                }
            }),
            participants,
        };

        self.recent_fights.push(cf.clone());
        if self.recent_fights.len() > MAX_RECENT_FIGHTS {
            let overflow = self.recent_fights.len() - MAX_RECENT_FIGHTS;
            self.recent_fights.drain(0..overflow);
        }
        let _ = fight.boss_object_id; // (kept for clarity/debugging)
        Some(cf)
    }

    /// Raise the single local participant's damage to the server's masked echo of
    /// their own direct hits, in an effectively-solo fight (see the call site in
    /// `finalize_fight`). The server re-broadcasts the local player's hits as
    /// damage packets with the bullet owner hidden behind a sentinel id; in a solo
    /// fight that masked total is entirely the local player's and is an exact
    /// direct-damage figure the shot simulation only approximates. No-op unless
    /// every gate holds, so a group fight keeps its packet-derived figure (in
    /// groups the server suppresses these self-echoes anyway, so the total is ~0).
    fn reconcile_masked_local(
        &self,
        fight: &FightState,
        participants: &mut [FightParticipant],
    ) -> bool {
        // Not a synthetic phase segment nor an aggregated-aux row (whose start HP
        // and attacker set span many members, not one boss's pool).
        if fight.segment_label.is_some()
            || fight.display_name_override.is_some()
            || crate::assets::aux_target_for_type(fight.boss_object_type).is_some()
        {
            return false;
        }
        // Effectively solo: no *other* player-class object was ever seen this run,
        // so the only source of masked (owner-hidden) damage is the local player.
        if self
            .seen_player_ids
            .iter()
            .any(|&id| id != self.local_object_id)
        {
            return false;
        }
        // No non-local attacker dealt positive damage. Checked on the raw attacker
        // set so an unknown/environmental source with damage blocks crediting its
        // HP to the local player. The sentinel row is the local player's own masked
        // echo, so it is excluded here rather than treated as a foreign attacker.
        let other_damage = fight.attackers.iter().any(|(&aid, a)| {
            !(a.is_local || aid == self.local_object_id)
                && aid != SENTINEL_ATTACKER_ID
                && a.damage > 0
        });
        if other_damage {
            return false;
        }
        // Exactly one local row to receive the figure.
        if participants.iter().filter(|p| p.is_local).count() != 1 {
            return false;
        }
        let (masked_damage, masked_hits) = match fight.attackers.get(&SENTINEL_ATTACKER_ID) {
            Some(m) => (m.damage, m.hits),
            None => return false,
        };
        let Some(local) = participants.iter_mut().find(|p| p.is_local) else {
            return false;
        };
        // Require a per-hit-complete masked stream (roughly one packet per local
        // hit). A sparse or single-lump stream (e.g. one kill-credit packet) is not
        // a trustworthy total, so reject it and keep the computed figure.
        if masked_hits < 5 || masked_hits.saturating_mul(2) < local.hits {
            return false;
        }
        // The masked stream is the local player's own direct hits. Raise the figure
        // to it when it exceeds the simulated total (it is exact where the
        // simulation only approximates). Even when it does not raise the value, the
        // stream is confirmed local, so report that so the HP-gap reconciliation can
        // still recover any remaining (non-direct) damage on a solo kill.
        if masked_damage > local.damage {
            local.damage = masked_damage;
            local.provenance = DamageProvenance::SelfServerReconciled;
        }
        true
    }

    /// Credit unattributed boss-HP loss to the local player in an
    /// effectively-solo, fully-observed kill (see the call site in
    /// `finalize_fight`). No-op unless every gate holds, so a group fight or any
    /// non-clean kill keeps its packet-derived figure.
    fn reconcile_solo_gap(
        &self,
        fight: &FightState,
        participants: &mut [FightParticipant],
        instanced_dungeon: bool,
        masked_is_local: bool,
    ) {
        // HP was truly fully removed. Excludes dipped-low despawns, self-destruct
        // bosses removed at high HP, invulnerable-finish completion markers and
        // the Marble Colossus heal-back "kill" -- none of which reach 0 HP.
        // A tiny positive floor absorbs tick granularity: the boss's last HP
        // status can read a few HP before the death removal arrives without a
        // 0-HP tick ever being sent, so a confirmed kill whose lowest observed HP
        // is within 1% of max still counts as fully removed. The 1% band stays
        // well below the >=10% marks where the excluded high-HP removals sit.
        let near_zero = fight.min_hp_seen <= 0
            || (fight.boss_max_hp > 0 && fight.min_hp_seen <= fight.boss_max_hp as i64 / 100);
        if !fight.killed || !near_zero {
            return;
        }
        // Only instanced dungeons bound the party to a small map where every
        // attacker is within our object-visibility radius. In the open realm (or
        // hub spaces) a distant player could damage this boss with a server-fired
        // ability yet never spawn as an object we see, so an apparent "solo" kill
        // there can't be trusted -- credit only what we can attribute directly.
        if !instanced_dungeon {
            return;
        }
        // Saw the boss from full HP, so its start HP is the entire damage pool.
        if fight.joined_late || fight.boss_max_hp <= 0 || fight.boss_start_hp != fight.boss_max_hp {
            return;
        }
        // Not a synthetic phase segment nor an aggregated-aux row (whose start HP
        // is a sum of many members, not one boss's pool).
        if fight.segment_label == Some("Pre-survival")
            || fight.display_name_override.is_some()
            || crate::assets::aux_target_for_type(fight.boss_object_type).is_some()
        {
            return;
        }
        // Wandering realm bosses roam the open world where our view of other
        // attackers is incomplete, so an apparent "solo" kill can't be trusted.
        if get_asset_manager().is_wandering_boss(fight.boss_object_type) {
            return;
        }
        // Effectively solo: no *other* player-class object was ever seen this run.
        // The run-wide check catches teammates who were visible but whose damage
        // packets this client never received.
        if self
            .seen_player_ids
            .iter()
            .any(|&id| id != self.local_object_id)
        {
            return;
        }
        // No non-local attacker dealt positive damage. Checked on the raw attacker
        // set (before `drop_unresolved`) so an unknown / environmental source with
        // damage still blocks crediting its HP to the local player. The masked
        // sentinel row is excluded only when it was already confirmed to be the
        // local player's own echo, so it does not block recovering the remaining
        // (non-direct) HP gap on a solo kill.
        let other_damage = fight.attackers.iter().any(|(&aid, a)| {
            !(a.is_local || aid == self.local_object_id)
                && !(masked_is_local && aid == SENTINEL_ATTACKER_ID)
                && a.damage > 0
        });
        if other_damage {
            return;
        }
        // Exactly one local row to receive the gap.
        if participants.iter().filter(|p| p.is_local).count() != 1 {
            return;
        }
        let attributed: i64 = participants.iter().map(|p| p.damage.max(0)).sum();
        let gap = fight.boss_start_hp as i64 - attributed;
        if gap <= 0 {
            return;
        }
        if let Some(local) = participants.iter_mut().find(|p| p.is_local) {
            local.damage += gap;
            local.provenance = DamageProvenance::SelfReconciled;
        }
    }

    /// Resolve a participant's display identity from tracked object state.
    /// `joined_late` marks the fight as one the local player entered after it had
    /// already begun, so remote observed damage is only a partial figure.
    fn build_participant(
        &self,
        attacker_id: i32,
        accum: &ParticipantAccum,
        joined_late: bool,
    ) -> FightParticipant {
        let obj = self.objects.get(&attacker_id);
        // Prefer the live object, but fall back to the identity snapshotted at hit
        // time: aggregated aux fights span the whole run, by which point the
        // attacker object may have despawned.
        let name = obj
            .and_then(|o| o.name.clone())
            .or_else(|| accum.last_name.clone());
        let object_type = obj
            .map(|o| o.object_type)
            .filter(|&t| t != 0)
            .unwrap_or(accum.last_object_type);
        let skin_id = obj
            .map(|o| o.skin_id)
            .filter(|&s| s != 0)
            .unwrap_or(accum.last_skin_id);
        let tex1 = obj
            .map(|o| o.tex1)
            .filter(|&t| t != 0)
            .unwrap_or(accum.last_tex1);
        let tex2 = obj
            .map(|o| o.tex2)
            .filter(|&t| t != 0)
            .unwrap_or(accum.last_tex2);
        // Prefer the loadout (gear + enchants) the attacker did the most damage
        // with over the course of the fight (tie-break by hit count), so mid-fight
        // gear swaps don't misreport the loadout as whatever was equipped when the
        // boss died. Fall back to the last-seen snapshot when no per-set data was
        // recorded. Enchants ride with the gear so the two always agree.
        let best = accum
            .damage_by_equipment
            .iter()
            .filter(|((gear, _), _)| gear.iter().any(|&s| s != -1))
            .max_by_key(|(_, &(dmg, hits))| (dmg, hits))
            .map(|(loadout, _)| loadout.clone());
        let (equipment, equipment_enchants) = match best {
            Some((gear, enchants)) => (gear, enchants),
            None => (
                obj.map(|o| o.equipment).unwrap_or([-1; 4]),
                obj.map(|o| o.equipment_enchants.clone())
                    .unwrap_or_default(),
            ),
        };

        let is_local = accum.is_local || attacker_id == self.local_object_id;
        // Estimated stand-in for `accum.damage` when some local hits could not be
        // valued; `None` leaves the raw accumulated figure in place.
        let mut local_damage_override: Option<i64> = None;
        let (resolved_name, provenance) = if is_local {
            let n = name
                .map(|n| sanitize_player_name(&n))
                .unwrap_or_else(|| "You".to_string());
            // Every local hit that resolved to a rolled shot contributed a damage
            // value; any unresolved hit (or a desynced/unavailable RNG) means we
            // cannot claim an exact total.
            let resolved_hits = accum.hits.saturating_sub(accum.unresolved_local_hits);
            let prov = if accum.unresolved_local_hits > 0 || accum.hits == 0 {
                if accum.damage > 0 && resolved_hits > 0 {
                    // Estimate the unvalued hits from the average of the resolved
                    // ones instead of leaving them at zero. An unresolved EnemyHit
                    // still landed on the target, so zero systematically
                    // under-counts; the average is a far better approximation.
                    let avg = accum.damage / resolved_hits as i64;
                    let extra = (accum.unresolved_local_hits as i128 * avg as i128)
                        .min(i64::MAX as i128) as i64;
                    local_damage_override = Some(accum.damage.saturating_add(extra));
                    DamageProvenance::SelfEstimated
                } else if accum.damage > 0 {
                    DamageProvenance::SelfPartial
                } else {
                    DamageProvenance::SelfPending
                }
            } else {
                DamageProvenance::SelfComputed
            };
            (n, prov)
        } else if attacker_id == SENTINEL_ATTACKER_ID {
            ("Unknown".to_string(), DamageProvenance::Unresolved)
        } else if let Some(n) = name.filter(|n| !n.is_empty()) {
            let prov = if joined_late {
                DamageProvenance::PartiallyObserved
            } else {
                DamageProvenance::Observed
            };
            (sanitize_player_name(&n), prov)
        } else {
            // Attacker never resolved to a named player object. The id may be a
            // stale/reused enemy or minion id, so never surface an enemy asset
            // name here -- these are collapsed into one "Unknown" row later.
            ("Unknown".to_string(), DamageProvenance::Unresolved)
        };

        // Damage taken: remote figures are exact (`Observed`); the local figure is
        // reconstructed (`Estimated`), or a lower bound (`Partial`) when some
        // incoming hit could not be valued. `None` when nothing was observed, so
        // the UI never shows a factual zero.
        let (damage_taken, damage_taken_provenance) = if accum.damage_taken_hits > 0 {
            let prov = if is_local {
                if accum.unmatched_taken_hits > 0 {
                    DamageTakenProvenance::Partial
                } else {
                    DamageTakenProvenance::Estimated
                }
            } else {
                DamageTakenProvenance::Observed
            };
            (Some(accum.damage_taken), prov)
        } else if is_local && accum.unmatched_taken_hits > 0 {
            // Local player was hit but no hit could be valued: a lower bound of 0.
            (Some(accum.damage_taken), DamageTakenProvenance::Partial)
        } else {
            (None, DamageTakenProvenance::Observed)
        };

        // Damage blocked (post-defense mitigation) is only reconstructable for the
        // local player; remote `DamagePacket`s are already post-mitigation with no
        // pre-defense figure. `None` for everyone else, and when no incoming hit
        // was actually valued (a `Some(0)` lower-bound taken must not report a
        // factual zero blocked), so the UI shows "—".
        let damage_blocked = if is_local && accum.damage_taken_hits > 0 {
            Some(accum.damage_blocked)
        } else {
            None
        };

        FightParticipant {
            object_id: attacker_id,
            object_type,
            skin_id,
            tex1,
            tex2,
            name: resolved_name,
            equipment,
            equipment_enchants,
            damage: local_damage_override.unwrap_or(accum.damage),
            hits: accum.hits,
            is_local,
            provenance,
            end_status: ParticipantEndStatus::Present,
            pet: None,
            damage_taken,
            damage_taken_provenance,
            damage_blocked,
            guarded_damage: Some(accum.guarded_damage),
            guarded_hits: Some(accum.guarded_hits),
        }
    }

    /// Record a proximity observation for a pet: upsert its identity and add a
    /// vote to the nearest player object within `PET_ASSOC_MAX_DIST`. Called on every pet spawn / status tick.
    fn record_pet_observation(&mut self, pet_id: i32) {
        let (px, py, pet_type, pet_abilities) = match self.objects.get(&pet_id) {
            Some(o) => (o.last_x, o.last_y, o.pet_type, o.pet_abilities),
            None => return,
        };
        // Nearest player object (excluding pets and the pet itself).
        let mut best: Option<(i32, f32)> = None;
        for (&oid, o) in &self.objects {
            if oid == pet_id || o.is_pet || !class_ids::is_player_class(o.object_type) {
                continue;
            }
            let dx = o.last_x - px;
            let dy = o.last_y - py;
            let dist = (dx * dx + dy * dy).sqrt();
            if dist <= PET_ASSOC_MAX_DIST && best.map_or(true, |(_, bd)| dist < bd) {
                best = Some((oid, dist));
            }
        }
        let track = self.pet_tracks.entry(pet_id).or_default();
        if pet_type != 0 {
            track.pet_type = pet_type;
        }
        if pet_abilities.iter().any(|&a| a != 0) {
            track.pet_abilities = pet_abilities;
        }
        if let Some((player_id, _)) = best {
            *track.votes.entry(player_id).or_insert(0) += 1;
        }
    }

    /// Attach each tracked pet to the participant it most often followed. Greedy by vote count with deterministic tie-breaks; each pet and
    /// each player is matched at most once.
    fn assign_pets(&self, participants: &mut [FightParticipant]) {
        if self.pet_tracks.is_empty() {
            return;
        }
        // Candidate (votes, pet_id, player_id) for each pet's top-voted player.
        let mut candidates: Vec<(u32, i32, i32)> = Vec::new();
        for (&pet_id, track) in &self.pet_tracks {
            if track.pet_type == 0 {
                continue;
            }
            // Highest votes; tie-break on lowest player id for determinism.
            let best = track
                .votes
                .iter()
                .max_by(|a, b| a.1.cmp(b.1).then(b.0.cmp(a.0)));
            if let Some((&player_id, &votes)) = best {
                if votes > 0 {
                    candidates.push((votes, pet_id, player_id));
                }
            }
        }
        // Strongest evidence first; deterministic tie-break on ids.
        candidates.sort_by(|a, b| b.0.cmp(&a.0).then(a.1.cmp(&b.1)).then(a.2.cmp(&b.2)));

        let mut used_pets: std::collections::HashSet<i32> = std::collections::HashSet::new();
        let mut used_players: std::collections::HashSet<i32> = std::collections::HashSet::new();
        for (_, pet_id, player_id) in candidates {
            if used_pets.contains(&pet_id) || used_players.contains(&player_id) {
                continue;
            }
            let Some(p) = participants.iter_mut().find(|p| p.object_id == player_id) else {
                continue;
            };
            let track = &self.pet_tracks[&pet_id];
            p.pet = Some(PetInfo {
                pet_type: track.pet_type,
                abilities: track
                    .pet_abilities
                    .iter()
                    .copied()
                    .filter(|&a| a != 0)
                    .collect(),
            });
            used_pets.insert(pet_id);
            used_players.insert(player_id);
        }
    }

    /// Whether `object_id` is currently a recorded attacker in any active or aux
    /// fight (so its departure is worth buffering for death detection).
    fn is_active_attacker(&self, object_id: i32) -> bool {
        self.fights
            .values()
            .any(|f| f.attackers.contains_key(&object_id))
            || self
                .aux_fights
                .values()
                .any(|f| f.attackers.contains_key(&object_id))
    }

    /// Drop grave/departure entries too old to correlate with any open fight, and
    /// bound each buffer's size.
    fn prune_correlation_buffers(&mut self, now: i64) {
        let cutoff = now - CORRELATION_MAX_AGE_MS;
        while self.recent_graves.front().is_some_and(|g| g.time < cutoff) {
            self.recent_graves.pop_front();
        }
        while self.recent_graves.len() > CORRELATION_MAX_ENTRIES {
            self.recent_graves.pop_front();
        }
        self.departures.retain(|_, d| d.time >= cutoff);
        if self.departures.len() > CORRELATION_MAX_ENTRIES {
            // Retain only the most recent entries when over the hard cap.
            let mut kept: Vec<(i32, Departure)> =
                self.departures.iter().map(|(&k, &v)| (k, v)).collect();
            kept.sort_by_key(|(_, d)| std::cmp::Reverse(d.time));
            kept.truncate(CORRELATION_MAX_ENTRIES);
            self.departures = kept.into_iter().collect();
        }
    }

    /// Consume the buffered gravestone spawn closest (in position, then time) to a
    /// departure at `(x, y, time)`, if one lies within the match radius/window.
    fn take_matching_grave(&mut self, x: f32, y: f32, time: i64) -> Option<i32> {
        let r2 = GRAVE_MATCH_RADIUS * GRAVE_MATCH_RADIUS;
        let mut best: Option<(usize, f32)> = None;
        for (i, g) in self.recent_graves.iter().enumerate() {
            if (g.time - time).abs() > GRAVE_MATCH_WINDOW_MS {
                continue;
            }
            let dx = g.x - x;
            let dy = g.y - y;
            let dist2 = dx * dx + dy * dy;
            if dist2 <= r2 && best.is_none_or(|(_, b)| dist2 < b) {
                best = Some((i, dist2));
            }
        }
        let (idx, _) = best?;
        self.recent_graves.remove(idx).map(|g| g.grave_type)
    }

    /// Resolve each participant's end status (present / died / nexused), consuming
    /// matched gravestone spawns so no grave is claimed by two participants.
    fn resolve_end_statuses(
        &mut self,
        participants: &mut [FightParticipant],
        started: i64,
        ended: i64,
        local_char_id: i32,
        local_left: bool,
    ) {
        for p in participants.iter_mut() {
            // The local player's death is authoritative (from `CharacterDied`), and
            // overrides presence: on a death-to-nexus map change the local object
            // is still in `objects` when this finalize runs.
            if p.is_local {
                if let Some((char_id, grave, gtime)) = self.pending_local_grave {
                    let matches_char =
                        char_id == 0 || local_char_id == 0 || char_id == local_char_id;
                    if matches_char
                        && gtime >= started - GRAVE_MATCH_WINDOW_MS
                        && gtime <= ended + GRAVE_MATCH_WINDOW_MS
                    {
                        p.end_status = ParticipantEndStatus::Died { grave_type: grave };
                        continue;
                    }
                }
                // No death recorded, but the local player left this boss before it
                // was completed (nexus / teleport / disconnect). They are always
                // present in `objects` from their own view, so this departure flag
                // is the only signal that they nexused out.
                if local_left {
                    p.end_status = ParticipantEndStatus::Nexused;
                    continue;
                }
            }
            // Still in view when the fight ended -> present (alive).
            if self.objects.contains_key(&p.object_id) {
                p.end_status = ParticipantEndStatus::Present;
                continue;
            }
            // The server death broadcast ("<name> died at level N, killed by ...")
            // is the authoritative death signal, matched by name within this
            // fight's window. Position-based grave correlation is only used to
            // supply the exact grave-tier sprite; when no grave can be matched
            // (one was stolen by a nearby simultaneous death, or none was seen)
            // a name-confirmed death is still recorded with a generic grave
            // (type 0, which the UI renders as the default gravestone).
            let name_died = self.remote_death_in_window(&p.name, started, ended);
            // Only a departure inside this fight's lifetime can carry a grave.
            let dep = self
                .departures
                .get(&p.object_id)
                .copied()
                .filter(|d| d.time >= started - DEPARTURE_GRACE_MS);
            let grave_type = dep.and_then(|d| self.take_matching_grave(d.x, d.y, d.time));
            p.end_status = match (grave_type, name_died) {
                (Some(grave_type), _) => ParticipantEndStatus::Died { grave_type },
                (None, true) => ParticipantEndStatus::Died { grave_type: 0 },
                // No death evidence: a departure inside the fight means they left
                // (nexused); no departure means they were simply present.
                (None, false) if dep.is_some() => ParticipantEndStatus::Nexused,
                (None, false) => ParticipantEndStatus::Present,
            };
        }
    }

    /// Record the local player's death from a `CharacterDied` game event so the
    /// finalizing fight can mark the local participant as dead with the right
    /// gravestone sprite. Buffered until the current fight(s) finalize.
    pub fn on_local_death(&mut self, char_id: i32, gravestone_type: i32, time_ms: i64) {
        self.pending_local_grave = Some((char_id, gravestone_type, time_ms));
    }

    /// Record a remote player's death from the server death-broadcast text
    /// ("<name> died at level N, killed by <killer>"). Buffered by name until the
    /// current fight(s) finalize; this authoritative signal is what marks a
    /// clustered-death player as Died even when their gravestone couldn't be
    /// position-correlated (see `resolve_end_statuses`).
    pub fn on_remote_death(&mut self, player_name: &str, time_ms: i64) {
        let key = sanitize_player_name(player_name).to_ascii_lowercase();
        if key.is_empty() {
            return;
        }
        self.remote_deaths.insert(key, time_ms);
    }

    /// Whether a remote death broadcast named `name` and it landed within this
    /// fight's window (matching the local-death gate). Gating to the fight window
    /// keeps a death at a later sequential boss from bleeding back onto an earlier
    /// boss the player cleared alive.
    fn remote_death_in_window(&self, name: &str, started: i64, ended: i64) -> bool {
        if self.remote_deaths.is_empty() {
            return false;
        }
        let key = sanitize_player_name(name).to_ascii_lowercase();
        if key.is_empty() {
            return false;
        }
        self.remote_deaths.get(&key).is_some_and(|&t| {
            t >= started - GRAVE_MATCH_WINDOW_MS && t <= ended + GRAVE_MATCH_WINDOW_MS
        })
    }
}

/// Apply an object status update to a tracked object.
fn apply_stats(obj: &mut TrackedObject, status: &ObjectStatusData, time_ms: i64) {
    obj.last_seen = time_ms;
    obj.last_x = status.pos.x;
    obj.last_y = status.pos.y;
    for stat in &status.stats {
        match stat.stat_type_id {
            x if x == StatType::MaxHP as u8 => obj.max_hp = stat.stat_value,
            x if x == StatType::HP as u8 => obj.hp = stat.stat_value,
            x if x == StatType::Name as u8 => {
                if let Some(ref n) = stat.string_stat_value {
                    if !n.is_empty() {
                        obj.name = Some(n.clone());
                    }
                }
            }
            x if x == StatType::Inventory0 as u8 => obj.equipment[0] = stat.stat_value,
            x if x == StatType::Inventory1 as u8 => obj.equipment[1] = stat.stat_value,
            x if x == StatType::Inventory2 as u8 => obj.equipment[2] = stat.stat_value,
            x if x == StatType::Inventory3 as u8 => obj.equipment[3] = stat.stat_value,
            x if x == StatType::SkinId as u8 => obj.skin_id = stat.stat_value,
            x if x == StatType::Texture1 as u8 => obj.tex1 = stat.stat_value as u32,
            x if x == StatType::Texture2 as u8 => obj.tex2 = stat.stat_value as u32,
            x if x == StatType::UniqueDataString as u8 => {
                obj.equipment_enchants =
                    parse_equipment_enchants(stat.string_stat_value.as_deref());
            }
            x if x == StatType::Attack as u8 => {
                obj.attack = stat.stat_value;
                obj.attack_seen = true;
            }
            x if x == StatType::Defense as u8 => {
                obj.defense = stat.stat_value;
                obj.defense_seen = true;
            }
            x if x == StatType::Condition as u8 => obj.condition = stat.stat_value,
            x if x == StatType::ConditionNew as u8 => obj.condition_new = stat.stat_value,
            x if x == StatType::Animation as u8 => obj.animation = stat.stat_value,
            x if x == StatType::ExaltationBonusDamage as u8 => obj.exalt_bonus = stat.stat_value,
            x if x == StatType::Wisdom as u8 => {
                obj.wisdom = stat.stat_value;
                obj.wisdom_seen = true;
            }
            x if x == StatType::MaxMP as u8 => obj.max_mp = stat.stat_value,
            x if x == StatType::Dexterity as u8 => obj.dexterity = stat.stat_value,
            x if x == StatType::PetType as u8 => {
                obj.is_pet = true;
                obj.pet_type = stat.stat_value;
            }
            x if x == StatType::PetName as u8 => obj.is_pet = true,
            x if x == StatType::PetFirstAbilityType as u8 => obj.pet_abilities[0] = stat.stat_value,
            x if x == StatType::PetSecondAbilityType as u8 => {
                obj.pet_abilities[1] = stat.stat_value
            }
            x if x == StatType::PetThirdAbilityType as u8 => obj.pet_abilities[2] = stat.stat_value,
            _ => {}
        }
    }
}

/// Strip the server-sent `,<hex>,<hex>` metadata suffix some player Name stats
/// carry (e.g. "Knekten,fe2,a19f"). RotMG player names never contain commas.
pub fn sanitize_player_name(name: &str) -> String {
    name.split(',').next().unwrap_or(name).trim().to_string()
}

/// Decode the `UniqueDataString` stat into per-equipment-slot enchant ids.
///
/// The value is a comma-separated per-inventory-slot string; slots 0-3 map to
/// weapon/ability/armor/ring. Missing slots (fewer than 4 segments, empty
/// segments, or a `None`/empty string) become empty vecs; segments past slot 3
/// (backpack/inventory items) are ignored.
fn parse_equipment_enchants(data: Option<&str>) -> [Vec<u16>; 4] {
    let data = match data {
        Some(d) if !d.is_empty() => d,
        _ => return Default::default(),
    };
    let mut segments = data.split(',');
    std::array::from_fn(|_| {
        segments
            .next()
            .map(|seg| {
                let trimmed = seg.trim();
                if trimmed.is_empty() {
                    Vec::new()
                } else {
                    parse_enchant_ids(trimmed)
                }
            })
            .unwrap_or_default()
    })
}

/// Map a raw realm map-name token (e.g. "{s.rotmg}") to a readable label.
pub fn normalize_dungeon(name: &str) -> String {
    let trimmed = name.trim();
    if trimmed.is_empty() || trimmed.starts_with('{') {
        "Realm".to_string()
    } else {
        trimmed.to_string()
    }
}

/// Whether a (normalized) map name is a dungeon instance whose bosses should be
/// folded into one per-run card. Excludes the open world and hub/social spaces
/// (Realm, Nexus, Vault, Guild Hall, Pet Yard, marketplaces, tutorial), which
/// keep every boss as its own standalone card.
pub fn is_groupable_dungeon(name: &str) -> bool {
    let trimmed = name.trim();
    if trimmed.is_empty() || trimmed.starts_with('{') {
        return false;
    }
    let lower = trimmed.to_lowercase();
    const EXCLUDED_PREFIXES: &[&str] = &[
        "realm",
        "nexus",
        "vault",
        "guild hall",
        "pet yard",
        "daily quest",
        "tutorial",
        "marketplace",
        "bazaar",
        "cloth bazaar",
    ];
    !EXCLUDED_PREFIXES.iter().any(|p| lower.starts_with(p))
}

/// Merge the accumulated state of a resumed wandering-boss fight segment (`src`)
/// into the segment kept as the run's canonical fight (`dst`). `dst` keeps its
/// identity (boss type, original start HP, segment label); the earliest start,
/// latest activity, lowest observed HP and killed flag win, and every attacker's
/// damage, hits, unresolved-hit count and per-loadout buckets are summed so no
/// contribution is lost across the boss's re-entries.
fn merge_fight_into(dst: &mut FightState, src: FightState) {
    dst.started_at = dst.started_at.min(src.started_at);
    dst.last_activity = dst.last_activity.max(src.last_activity);
    dst.min_hp_seen = dst.min_hp_seen.min(src.min_hp_seen);
    dst.killed = dst.killed || src.killed;
    dst.joined_late = dst.joined_late || src.joined_late;
    dst.local_close_calls += src.local_close_calls;
    for (aid, accum) in src.attackers {
        let e = dst.attackers.entry(aid).or_default();
        e.damage += accum.damage;
        e.hits += accum.hits;
        e.is_local = e.is_local || accum.is_local;
        e.guarded_damage += accum.guarded_damage;
        e.guarded_hits += accum.guarded_hits;
        e.unresolved_local_hits += accum.unresolved_local_hits;
        e.damage_taken += accum.damage_taken;
        e.damage_taken_hits += accum.damage_taken_hits;
        e.damage_blocked += accum.damage_blocked;
        e.unmatched_taken_hits += accum.unmatched_taken_hits;
        if accum.last_name.is_some() {
            e.last_name = accum.last_name;
        }
        if accum.last_object_type != 0 {
            e.last_object_type = accum.last_object_type;
        }
        if accum.last_skin_id != 0 {
            e.last_skin_id = accum.last_skin_id;
        }
        if accum.last_tex1 != 0 {
            e.last_tex1 = accum.last_tex1;
        }
        if accum.last_tex2 != 0 {
            e.last_tex2 = accum.last_tex2;
        }
        for (loadout, (dmg, hits)) in accum.damage_by_equipment {
            let bucket = e.damage_by_equipment.entry(loadout).or_default();
            bucket.0 += dmg;
            bucket.1 += hits;
        }
    }
}

/// Drop every unresolved attacker (server sentinel id `0xFFFFFF` = damage with
/// no attributable source: area/ground hazards, status ticks, or bullets the
/// server sent without an owner). These are never a real player -- attributing
/// them to anyone would be wrong -- so they are removed entirely rather than
/// surfaced as a misleading "Unknown" row. The local player and named players
/// are left untouched. Runs before overkill trimming so real players keep their
/// exact damage instead of being scaled down to share HP with unattributed hits.
fn drop_unresolved(participants: &mut Vec<FightParticipant>) {
    participants.retain(|p| !(p.provenance == DamageProvenance::Unresolved && !p.is_local));
}

/// Scale participant damage so the total does not exceed `cap` (the boss HP the
/// fight removed). Preserves each participant's share and integer exactness
/// (largest-remainder distribution). A no-op when the total is already within
/// the cap, so it only ever removes overkill, never inflates undercounted fights.
fn cap_overkill(participants: &mut [FightParticipant], cap: i64) {
    if cap <= 0 {
        return;
    }
    let total: i64 = participants.iter().map(|p| p.damage.max(0)).sum();
    if total <= cap {
        return;
    }

    // Proportional floor plus the fractional remainder, ranked by largest
    // fractional part, so the scaled values sum to exactly `cap`.
    let mut remainders: Vec<(usize, i64)> = Vec::with_capacity(participants.len());
    let mut assigned: i64 = 0;
    for (i, p) in participants.iter_mut().enumerate() {
        let raw = p.damage.max(0) as i128 * cap as i128;
        let floor = (raw / total as i128) as i64;
        let rem = (raw % total as i128) as i64;
        p.damage = floor;
        // Keep guarded damage a subset of the (now capped) total by scaling it
        // with the same ratio; floor(guarded*cap/total) <= floor(dmg*cap/total).
        if let Some(gd) = p.guarded_damage.as_mut() {
            let graw = (*gd).max(0) as i128 * cap as i128;
            *gd = (graw / total as i128) as i64;
        }
        assigned += floor;
        remainders.push((i, rem));
    }
    let mut leftover = cap - assigned;
    remainders.sort_by(|a, b| b.1.cmp(&a.1));
    for &(i, _) in remainders.iter() {
        if leftover <= 0 {
            break;
        }
        participants[i].damage += 1;
        leftover -= 1;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::protocol::data::{StatData, StatType, WorldPosData};

    #[test]
    fn ls_proc_point_blank_overshoot_misses() {
        // Target sits inside the 4.6-tile spawn ring (0.9 tiles ahead), so both
        // procs spawn past it and travel away: no hits regardless of radius.
        assert_eq!(ls_proc_hits(0.0, 0.0, 0.0, 0.9, 0.0, 1.5), 0);
    }

    #[test]
    fn ls_proc_beyond_spawn_ring_hits_with_large_hitbox() {
        // Target 6 tiles dead ahead with a 1.5-tile radius: both +/-9 procs
        // (perp offset ~0.94 tiles at that range) fall inside, so both land.
        assert_eq!(ls_proc_hits(0.0, 0.0, 0.0, 6.0, 0.0, 1.5), 2);
    }

    #[test]
    fn ls_proc_perpendicular_miss_with_small_hitbox() {
        // Same 6-tile-ahead target but a 0.5-tile radius: the +/-9 procs spread
        // ~0.94 tiles to each side, so a small target slips between them.
        assert_eq!(ls_proc_hits(0.0, 0.0, 0.0, 6.0, 0.0, 0.5), 0);
    }

    #[test]
    fn ls_proc_beyond_range_misses() {
        // Target far past the proc's travel distance is never reached.
        assert_eq!(ls_proc_hits(0.0, 0.0, 0.0, 40.0, 0.0, 2.0), 0);
    }

    fn stat(id: StatType, value: i32) -> StatData {
        StatData {
            stat_type_id: id as u8,
            stat_type: id,
            stat_value: value,
            string_stat_value: None,
            stat_value_two: 0,
        }
    }

    fn name_stat(name: &str) -> StatData {
        StatData {
            stat_type_id: StatType::Name as u8,
            stat_type: StatType::Name,
            stat_value: 0,
            string_stat_value: Some(name.to_string()),
            stat_value_two: 0,
        }
    }

    fn status(object_id: i32, stats: Vec<StatData>) -> ObjectStatusData {
        ObjectStatusData {
            object_id,
            pos: WorldPosData { x: 0.0, y: 0.0 },
            stats,
        }
    }

    fn status_at(object_id: i32, x: f32, y: f32, stats: Vec<StatData>) -> ObjectStatusData {
        ObjectStatusData {
            object_id,
            pos: WorldPosData { x, y },
            stats,
        }
    }

    #[test]
    fn departure_snapshot_uses_last_seen_position() {
        // An attacker that is removed while its boss is alive records a departure
        // at its last-seen position, which later correlates to a grave there.
        let mut t = CombatTracker::new();
        t.on_map_change("Mad Lab", 1, 0);
        t.on_player_loaded(1000, 1);
        t.on_object_spawn(
            500,
            0x10E4,
            &status(
                500,
                vec![stat(StatType::MaxHP, 12_000), stat(StatType::HP, 12_000)],
            ),
            0,
        );
        // Remote attacker deals damage from (25, 30), then leaves.
        t.on_object_spawn(
            600,
            0x0321,
            &status_at(600, 25.0, 30.0, vec![name_stat("Bob")]),
            10,
        );
        t.on_damage(500, 600, 100, 20);
        t.on_object_removed(600, 30);
        let dep = t.departures.get(&600).copied().expect("departure recorded");
        assert_eq!((dep.x, dep.y), (25.0, 30.0));
    }

    fn participant(object_id: i32, is_local: bool) -> FightParticipant {
        FightParticipant {
            object_id,
            object_type: 0x0321,
            skin_id: 0,
            tex1: 0,
            tex2: 0,
            name: "P".into(),
            equipment: [-1; 4],
            equipment_enchants: Default::default(),
            damage: 100,
            hits: 1,
            is_local,
            provenance: DamageProvenance::Observed,
            end_status: ParticipantEndStatus::Present,
            pet: None,
            damage_taken: None,
            damage_taken_provenance: DamageTakenProvenance::Observed,
            damage_blocked: None,
            guarded_damage: None,
            guarded_hits: None,
        }
    }

    #[test]
    fn pet_associates_to_nearest_player_by_position() {
        let mut t = CombatTracker::new();
        t.on_map_change("Mad Lab", 42, 0);
        t.on_player_loaded(1000, 42);

        // Boss at origin.
        t.on_object_spawn(
            500,
            0x10E4,
            &status(
                500,
                vec![stat(StatType::MaxHP, 300_000), stat(StatType::HP, 300_000)],
            ),
            100,
        );
        // Two remote players far apart. 0x0321 (801) is a real player class.
        t.on_object_spawn(
            600,
            0x0321,
            &status_at(600, 10.0, 10.0, vec![name_stat("Alice")]),
            100,
        );
        t.on_object_spawn(
            700,
            0x0321,
            &status_at(700, 50.0, 50.0, vec![name_stat("Bob")]),
            100,
        );
        // A pet next to Alice with two abilities, and a pet next to Bob.
        t.on_object_spawn(
            800,
            0x9999,
            &status_at(
                800,
                11.0,
                11.0,
                vec![
                    stat(StatType::PetType, 1234),
                    stat(StatType::PetFirstAbilityType, 404),
                    stat(StatType::PetSecondAbilityType, 407),
                ],
            ),
            100,
        );
        t.on_object_spawn(
            900,
            0x9999,
            &status_at(900, 51.0, 51.0, vec![stat(StatType::PetType, 5678)]),
            100,
        );

        // Both remotes damage the boss; local lands a hit so the fight is kept.
        t.on_damage(500, 600, 5000, 200);
        t.on_damage(500, 700, 5000, 200);
        t.on_local_hit(500, 7, 1000, 1000, 250);
        t.on_object_status(500, &status(500, vec![stat(StatType::HP, 0)]), 500);
        let done = t.on_tick(500);
        assert_eq!(done.len(), 1);

        let alice = done[0]
            .participants
            .iter()
            .find(|p| p.name == "Alice")
            .expect("Alice");
        let bob = done[0]
            .participants
            .iter()
            .find(|p| p.name == "Bob")
            .expect("Bob");
        let alice_pet = alice.pet.as_ref().expect("Alice pet matched");
        assert_eq!(alice_pet.pet_type, 1234);
        assert_eq!(alice_pet.abilities, vec![404, 407]);
        assert_eq!(bob.pet.as_ref().expect("Bob pet matched").pet_type, 5678);
    }

    #[test]
    fn recycled_pet_object_id_does_not_inherit_owner_votes() {
        let mut t = CombatTracker::new();
        t.on_map_change("Mad Lab", 42, 0);
        t.on_player_loaded(1000, 42);
        t.on_object_spawn(
            500,
            0x10E4,
            &status(
                500,
                vec![stat(StatType::MaxHP, 300_000), stat(StatType::HP, 300_000)],
            ),
            100,
        );
        t.on_object_spawn(
            600,
            0x0321,
            &status_at(600, 10.0, 10.0, vec![name_stat("Alice")]),
            100,
        );
        t.on_object_spawn(
            700,
            0x0321,
            &status_at(700, 50.0, 50.0, vec![name_stat("Bob")]),
            100,
        );

        t.on_object_spawn(
            800,
            0x9999,
            &status_at(800, 11.0, 11.0, vec![stat(StatType::PetType, 1234)]),
            100,
        );
        t.on_object_status(800, &status_at(800, 11.0, 11.0, vec![]), 110);

        t.on_object_spawn(
            800,
            0x9999,
            &status_at(800, 51.0, 51.0, vec![stat(StatType::PetType, 5678)]),
            120,
        );
        t.on_damage(500, 600, 5000, 200);
        t.on_damage(500, 700, 5000, 200);
        t.on_local_hit(500, 7, 1000, 1000, 250);
        t.on_object_status(500, &status(500, vec![stat(StatType::HP, 0)]), 500);

        let done = t.on_tick(500);
        let bob = done[0]
            .participants
            .iter()
            .find(|p| p.name == "Bob")
            .expect("Bob");
        assert_eq!(
            bob.pet.as_ref().expect("recycled pet matched").pet_type,
            5678
        );
    }

    #[test]
    fn pet_beyond_threshold_is_not_associated() {
        let mut t = CombatTracker::new();
        t.on_map_change("Mad Lab", 42, 0);
        t.on_player_loaded(1000, 42);
        t.on_object_spawn(
            500,
            0x10E4,
            &status(
                500,
                vec![stat(StatType::MaxHP, 300_000), stat(StatType::HP, 300_000)],
            ),
            100,
        );
        t.on_object_spawn(
            600,
            0x0321,
            &status_at(600, 10.0, 10.0, vec![name_stat("Alice")]),
            100,
        );
        // Pet is 20+ tiles away -> beyond PET_ASSOC_MAX_DIST, no vote.
        t.on_object_spawn(
            800,
            0x9999,
            &status_at(800, 30.0, 30.0, vec![stat(StatType::PetType, 1234)]),
            100,
        );

        t.on_damage(500, 600, 5000, 200);
        t.on_local_hit(500, 7, 1000, 1000, 250);
        t.on_object_status(500, &status(500, vec![stat(StatType::HP, 0)]), 500);
        let done = t.on_tick(500);
        let alice = done[0]
            .participants
            .iter()
            .find(|p| p.name == "Alice")
            .expect("Alice");
        assert!(alice.pet.is_none(), "distant pet must not be associated");
    }

    // ----- Close-call detection -----

    /// Set the local player HP/MaxHP, simulating a NewTick status. MaxHP is only
    /// sent when it changes, matching the real stream (apply_stats retains it).
    fn local_hp(t: &mut CombatTracker, hp: i32, max_hp: Option<i32>, time_ms: i64) {
        let mut stats = vec![stat(StatType::HP, hp)];
        if let Some(m) = max_hp {
            stats.push(stat(StatType::MaxHP, m));
        }
        t.on_object_status(1000, &status(1000, stats), time_ms);
    }

    #[test]
    fn close_call_counts_once_per_dip_and_clears_above_threshold() {
        let mut t = CombatTracker::new();
        t.on_map_change("Realm", 1, 0);
        t.on_player_loaded(1000, 42);
        // Full HP: no close call.
        local_hp(&mut t, 1000, Some(1000), 10);
        // Exactly 20% is NOT a close call (matches the game's strict `< 20%`).
        local_hp(&mut t, 200, None, 20);
        assert_eq!(t.pending_close_calls, 0);
        // Drop below 20%: counts once.
        local_hp(&mut t, 199, None, 30);
        // Still under 20% across ticks: no double count.
        local_hp(&mut t, 150, None, 40);
        local_hp(&mut t, 199, None, 50);
        assert_eq!(t.pending_close_calls, 1);
        // Recover to 20% or above: clears.
        local_hp(&mut t, 500, None, 60);
        // Dip again: a fresh occasion.
        local_hp(&mut t, 100, None, 70);
        assert_eq!(t.take_pending_close_calls(), 2);
        // Draining resets the counter.
        assert_eq!(t.take_pending_close_calls(), 0);
    }

    #[test]
    fn close_call_attributes_to_active_fight() {
        let mut t = CombatTracker::new();
        t.on_map_change("Realm", 1, 0);
        t.on_player_loaded(1000, 42);
        local_hp(&mut t, 1000, Some(1000), 10);
        // Start a boss fight, then dip.
        t.on_object_spawn(
            500,
            0x10E4,
            &status(
                500,
                vec![stat(StatType::MaxHP, 300_000), stat(StatType::HP, 300_000)],
            ),
            100,
        );
        t.on_damage(500, 1000, 5000, 150);
        t.on_local_hit(500, 7, 1000, 1000, 150);
        local_hp(&mut t, 100, None, 200);
        t.on_object_status(500, &status(500, vec![stat(StatType::HP, 0)]), 500);
        let done = t.on_tick(500);
        assert_eq!(done.len(), 1);
        assert_eq!(done[0].local_close_calls, 1);
    }

    #[test]
    fn close_call_before_fight_carries_onto_next_fight() {
        let mut t = CombatTracker::new();
        t.on_map_change("Realm", 1, 0);
        t.on_player_loaded(1000, 42);
        local_hp(&mut t, 1000, Some(1000), 10);
        // Dip with no active fight: buffered for the run.
        local_hp(&mut t, 100, None, 50);
        assert_eq!(t.run_pending_fight_close_calls, 1);
        // Recover then start a fight: the buffered dip folds into it.
        local_hp(&mut t, 1000, None, 80);
        t.on_object_spawn(
            500,
            0x10E4,
            &status(
                500,
                vec![stat(StatType::MaxHP, 300_000), stat(StatType::HP, 300_000)],
            ),
            100,
        );
        t.on_damage(500, 1000, 5000, 150);
        t.on_local_hit(500, 7, 1000, 1000, 150);
        t.on_object_status(500, &status(500, vec![stat(StatType::HP, 0)]), 500);
        let done = t.on_tick(500);
        assert_eq!(done[0].local_close_calls, 1);
        assert_eq!(t.run_pending_fight_close_calls, 0);
    }

    fn fight_state(local_close_calls: i32, started_at: i64) -> FightState {
        FightState {
            boss_object_id: 500,
            boss_object_type: 0x10E4,
            boss_max_hp: 300_000,
            boss_start_hp: 300_000,
            started_at,
            last_activity: started_at,
            killed: false,
            min_hp_seen: 300_000,
            local_char_id: 42,
            local_close_calls,
            attackers: HashMap::new(),
            segment_label: None,
            display_name_override: None,
            split_done: false,
            joined_late: false,
        }
    }

    #[test]
    fn merge_fight_into_sums_close_calls() {
        // A wandering boss re-detected under a new segment must not drop the
        // close calls recorded on either segment.
        let mut dst = fight_state(1, 100);
        let src = fight_state(2, 200);
        merge_fight_into(&mut dst, src);
        assert_eq!(dst.local_close_calls, 3);
    }

    #[test]
    fn close_call_map_change_resets_edge_state() {
        let mut t = CombatTracker::new();
        t.on_map_change("Realm", 1, 0);
        t.on_player_loaded(1000, 42);
        local_hp(&mut t, 100, Some(1000), 20);
        assert_eq!(t.take_pending_close_calls(), 1);
        // A new instance clears the low-HP edge so the first dip there counts.
        t.on_map_change("Realm", 2, 1000);
        t.on_player_loaded(1000, 42);
        local_hp(&mut t, 100, Some(1000), 1020);
        assert_eq!(t.take_pending_close_calls(), 1);
    }

    #[test]
    fn end_status_resolves_died_nexused_and_present() {
        let mut t = CombatTracker::new();
        // Participant 3 is still in view -> Present.
        t.objects.insert(3, TrackedObject::default());
        // Participant 1 left near (10,10); a grave spawned close by -> Died.
        t.departures.insert(
            1,
            Departure {
                x: 10.0,
                y: 10.0,
                time: 1_000,
            },
        );
        // Participant 2 left far from any grave -> Nexused.
        t.departures.insert(
            2,
            Departure {
                x: 90.0,
                y: 90.0,
                time: 1_000,
            },
        );
        t.recent_graves.push_back(GraveSpawn {
            grave_type: 1830,
            x: 10.4,
            y: 9.7,
            time: 1_300,
        });

        let mut ps = vec![
            participant(1, false),
            participant(2, false),
            participant(3, false),
        ];
        t.resolve_end_statuses(&mut ps, 0, 5_000, 0, false);

        assert_eq!(
            ps[0].end_status,
            ParticipantEndStatus::Died { grave_type: 1830 }
        );
        assert_eq!(ps[1].end_status, ParticipantEndStatus::Nexused);
        assert_eq!(ps[2].end_status, ParticipantEndStatus::Present);
        // The grave was consumed so it cannot be claimed twice.
        assert!(t.recent_graves.is_empty());
    }

    #[test]
    fn end_status_remote_death_broadcast_overrides_nexused() {
        let mut t = CombatTracker::new();
        // Two players die in a tight cluster; the position match assigns the one
        // grave to whoever is closest, leaving the other with no grave -> the old
        // logic scored them Nexused. The server death broadcast (by name) recovers
        // it: both must be Died.
        t.departures.insert(
            1,
            Departure {
                x: 10.0,
                y: 10.0,
                time: 1_000,
            },
        );
        t.departures.insert(
            2,
            Departure {
                x: 10.5,
                y: 10.5,
                time: 1_000,
            },
        );
        // Only one grave observed, closest to participant 1.
        t.recent_graves.push_back(GraveSpawn {
            grave_type: 1837,
            x: 10.1,
            y: 10.0,
            time: 1_100,
        });
        // Both deaths were broadcast during the fight window.
        t.on_remote_death("Zannmp", 1_050);
        t.on_remote_death("Yumiho", 1_050);

        let mut p1 = participant(1, false);
        p1.name = "Zannmp".into();
        let mut p2 = participant(2, false);
        p2.name = "Yumiho".into();
        // A third participant with no death broadcast and no grave stays Nexused.
        let mut p3 = participant(3, false);
        p3.name = "Alive".into();
        t.departures.insert(
            3,
            Departure {
                x: 90.0,
                y: 90.0,
                time: 1_000,
            },
        );
        let mut ps = vec![p1, p2, p3];
        t.resolve_end_statuses(&mut ps, 0, 5_000, 0, false);

        // Participant 1 gets the exact grave tier; participant 2 is name-confirmed
        // with a generic grave (type 0); participant 3 has no death evidence.
        assert_eq!(
            ps[0].end_status,
            ParticipantEndStatus::Died { grave_type: 1837 }
        );
        assert_eq!(
            ps[1].end_status,
            ParticipantEndStatus::Died { grave_type: 0 }
        );
        assert_eq!(ps[2].end_status, ParticipantEndStatus::Nexused);
    }

    #[test]
    fn end_status_remote_death_outside_window_is_ignored() {
        let mut t = CombatTracker::new();
        // A death broadcast far outside this fight's window (a later boss) must not
        // bleed back onto an earlier boss the player cleared alive.
        t.departures.insert(
            1,
            Departure {
                x: 10.0,
                y: 10.0,
                time: 1_000,
            },
        );
        t.on_remote_death("Later", 500_000);
        let mut p1 = participant(1, false);
        p1.name = "Later".into();
        let mut ps = vec![p1];
        t.resolve_end_statuses(&mut ps, 0, 5_000, 0, false);
        assert_eq!(ps[0].end_status, ParticipantEndStatus::Nexused);
    }

    #[test]
    fn end_status_local_death_overrides_presence() {
        let mut t = CombatTracker::new();
        t.on_player_loaded(1000, 77);
        // Local object is still tracked (map-change finalize runs before clear).
        t.objects.insert(1000, TrackedObject::default());
        t.on_local_death(77, 1837, 2_000);

        let mut ps = vec![participant(1000, true)];
        t.resolve_end_statuses(&mut ps, 0, 5_000, 77, false);
        assert_eq!(
            ps[0].end_status,
            ParticipantEndStatus::Died { grave_type: 1837 }
        );
    }

    #[test]
    fn end_status_departure_before_fight_is_present() {
        let mut t = CombatTracker::new();
        // A departure long before the fight started must not be correlated.
        t.departures.insert(
            1,
            Departure {
                x: 10.0,
                y: 10.0,
                time: 0,
            },
        );
        t.recent_graves.push_back(GraveSpawn {
            grave_type: 1830,
            x: 10.0,
            y: 10.0,
            time: 100,
        });

        let mut ps = vec![participant(1, false)];
        t.resolve_end_statuses(&mut ps, 60_000, 65_000, 0, false);
        assert_eq!(ps[0].end_status, ParticipantEndStatus::Present);
    }

    #[test]
    fn local_player_nexus_mid_fight_marks_nexused() {
        // The local player engages an MV dancer, then teleports to the Nexus
        // before it is completed. They are always present in `objects` from their
        // own view, so the map-change departure is the only nexus signal.
        let mut t = CombatTracker::new();
        t.on_map_change("Moonlight Village", 42, 0);
        t.on_player_loaded(1000, 42);
        t.on_object_spawn(
            500,
            20450,
            &status(
                500,
                vec![stat(StatType::MaxHP, 360_000), stat(StatType::HP, 360_000)],
            ),
            100,
        );
        t.pending_shots.insert(
            10,
            PendingShot {
                base_damage: 1000,
                armor_piercing: false,
                ..Default::default()
            },
        );
        t.on_local_hit(500, 10, 1000, 1000, 200);
        let done = t.on_map_change("Nexus", 0, 500);
        assert_eq!(done.len(), 1);
        assert!(!done[0].killed, "dancer left unfinished");
        let local = done[0]
            .participants
            .iter()
            .find(|p| p.is_local)
            .expect("local");
        assert_eq!(local.end_status, ParticipantEndStatus::Nexused);
    }

    #[test]
    fn lethal_strike_procs_credited_once_per_shot_not_per_projectile() {
        // A multi-projectile weapon fires several bullets per trigger, but the
        // two Lethal Strike side procs fire once per trigger. Each projectile
        // still gets the per-hit main-shot bonus, so for a 2-projectile weapon
        // one shot yields: 2 main bonuses + 2 procs (not 2 + 4).
        use crate::assets::ScalingStat;
        // Flat 100 bonus regardless of WIS/DEF for exact arithmetic.
        let params = LethalStrikeParams {
            ignore_flat: 100.0,
            ignore_perc: 0.0,
            stat_mod_flat: 0.0,
            stat_mod_perc: 0.0,
            stat_mod_scaling_min: 0.0,
            scaling_stat: ScalingStat::Wisdom,
            duration: 2.4,
            extra_shots: 2,
        };
        let mut t = CombatTracker::new();
        t.on_map_change("Mad Lab", 42, 0);
        t.on_player_loaded(1000, 42);
        t.on_object_spawn(
            500,
            0x10E4,
            &status(
                500,
                vec![
                    stat(StatType::MaxHP, 500_000),
                    stat(StatType::HP, 500_000),
                    stat(StatType::Defense, 0),
                ],
            ),
            0,
        );
        t.lethal_strike_windows.push_back((0, 10_000));
        // Two projectiles of the same shot: primary (index 0) and secondary.
        t.pending_shots.insert(
            10,
            PendingShot {
                base_damage: 1000,
                shot_time: 100,
                lethal_strike: Some((params, 0)),
                extra_projectile: false,
                ..Default::default()
            },
        );
        t.pending_shots.insert(
            11,
            PendingShot {
                base_damage: 1000,
                shot_time: 100,
                lethal_strike: Some((params, 0)),
                extra_projectile: true,
                ..Default::default()
            },
        );
        t.on_local_hit(500, 10, 1000, 1000, 100);
        t.on_local_hit(500, 11, 1000, 1000, 100);
        let done = t.on_map_change("Nexus", 0, 200);
        let local = done[0]
            .participants
            .iter()
            .find(|p| p.is_local)
            .expect("local");
        // primary: 1000 + 100 (main) + 200 (procs) = 1300
        // secondary: 1000 + 100 (main) = 1100  (procs NOT re-credited)
        assert_eq!(local.damage, 2400);
    }

    #[test]
    fn cursed_target_via_stat_id_69_amplifies_local_damage() {
        // ConditionNew (Cursed/Exposed/Petrified) is broadcast on stat id 69 in
        // the current protocol, not the legacy id 96. A cursed boss must take
        // +25% from local weapon hits. The stat is built with a literal id 69
        // (not `StatType::ConditionNew as u8`) so a discriminant regression fails
        // this test rather than passing tautologically.
        const CURSED_BIT: i32 = 0x0000_0040;
        let cursed = StatData {
            stat_type_id: 69,
            stat_type: StatType::ConditionNew,
            stat_value: CURSED_BIT,
            string_stat_value: None,
            stat_value_two: 0,
        };
        let mut t = CombatTracker::new();
        t.on_map_change("Mad Lab", 42, 0);
        t.on_player_loaded(1000, 42);
        // Boss with 0 DEF so Curse is the only damage modifier.
        t.on_object_spawn(
            500,
            0x10E4,
            &status(
                500,
                vec![
                    stat(StatType::MaxHP, 50_000),
                    stat(StatType::HP, 50_000),
                    stat(StatType::Defense, 0),
                    cursed,
                ],
            ),
            0,
        );
        t.pending_shots.insert(
            10,
            PendingShot {
                base_damage: 1000,
                armor_piercing: false,
                ..Default::default()
            },
        );
        t.on_local_hit(500, 10, 1000, 1000, 10);
        let done = t.on_map_change("Nexus", 0, 20);
        let local = done[0]
            .participants
            .iter()
            .find(|p| p.is_local)
            .expect("local");
        assert_eq!(local.damage, 1250, "cursed boss takes 1000 * 1.25");
    }

    #[test]
    fn local_player_present_when_boss_killed_before_map_change() {
        // The boss dies first (killed set), then the local player leaves. A map
        // change that finalizes an already-completed fight keeps them Present, not
        // nexused.
        let mut t = CombatTracker::new();
        t.on_map_change("Mad Lab", 42, 0);
        t.on_player_loaded(1000, 42);
        t.on_object_spawn(
            500,
            0x10E4,
            &status(
                500,
                vec![stat(StatType::MaxHP, 50_000), stat(StatType::HP, 50_000)],
            ),
            0,
        );
        t.pending_shots.insert(
            11,
            PendingShot {
                base_damage: 1000,
                armor_piercing: false,
                ..Default::default()
            },
        );
        t.on_local_hit(500, 11, 1000, 1000, 10);
        // Boss reaches 0 HP: the fight is flagged killed but not yet finalized.
        t.on_object_status(500, &status(500, vec![stat(StatType::HP, 0)]), 20);
        let done = t.on_map_change("Nexus", 0, 30);
        assert_eq!(done.len(), 1);
        assert!(done[0].killed, "boss was killed before the map change");
        let local = done[0]
            .participants
            .iter()
            .find(|p| p.is_local)
            .expect("local");
        assert_eq!(local.end_status, ParticipantEndStatus::Present);
    }

    #[test]
    fn mv_completed_dancer_keeps_local_present_when_nexusing_after() {
        // Moonlight Village bosses never reach 0 HP -- they lock invulnerable and
        // are completed by a marker object. A completion marker seen this run
        // scores the dancer as killed, so a later nexus must keep the local player
        // Present / Completed rather than flagging them Nexused.
        let mut t = CombatTracker::new();
        t.on_map_change("Moonlight Village", 42, 0);
        t.on_player_loaded(1000, 42);
        t.on_object_spawn(
            500,
            20450,
            &status(
                500,
                vec![stat(StatType::MaxHP, 360_000), stat(StatType::HP, 360_000)],
            ),
            100,
        );
        t.pending_shots.insert(
            10,
            PendingShot {
                base_damage: 1000,
                armor_piercing: false,
                ..Default::default()
            },
        );
        t.on_local_hit(500, 10, 1000, 1000, 200);
        // Dancer floors at 1 HP (invulnerable), never removed.
        t.on_object_status(500, &status(500, vec![stat(StatType::HP, 1)]), 300);
        // MV Dungeon Complete marker (20658) -> the dancer is scored completed.
        t.on_object_spawn(
            15598,
            20658,
            &status(
                15598,
                vec![stat(StatType::MaxHP, 100), stat(StatType::HP, 100)],
            ),
            400,
        );
        // Local nexuses afterwards; completion must survive the departure finalize.
        let done = t.on_map_change("Nexus", 0, 500);
        assert_eq!(done.len(), 1);
        assert!(
            done[0].killed,
            "marker completion survives the nexus finalize"
        );
        let local = done[0]
            .participants
            .iter()
            .find(|p| p.is_local)
            .expect("local");
        assert_eq!(local.end_status, ParticipantEndStatus::Present);
    }

    #[test]
    fn late_arrival_marks_remote_partially_observed() {
        let mut t = CombatTracker::new();
        t.on_map_change("Mad Lab", 42, 0);
        t.on_player_loaded(1000, 42);

        // Boss is first seen already damaged (200k of 300k), meaning the local
        // player arrived after the fight had begun.
        t.on_object_spawn(
            500,
            0x10E4,
            &status(
                500,
                vec![stat(StatType::MaxHP, 300_000), stat(StatType::HP, 200_000)],
            ),
            100,
        );
        t.on_object_spawn(600, 0x0321, &status(600, vec![name_stat("Alice")]), 100);

        t.on_damage(500, 600, 5000, 200);
        t.on_local_hit(500, 7, 1000, 1000, 250);
        t.on_object_status(500, &status(500, vec![stat(StatType::HP, 0)]), 500);

        let done = t.on_tick(500);
        assert_eq!(done.len(), 1);
        let alice = done[0]
            .participants
            .iter()
            .find(|p| p.name == "Alice")
            .expect("Alice tracked");
        assert_eq!(alice.provenance, DamageProvenance::PartiallyObserved);
    }

    #[test]
    fn full_hp_start_keeps_remote_observed() {
        let mut t = CombatTracker::new();
        t.on_map_change("Mad Lab", 42, 0);
        t.on_player_loaded(1000, 42);

        // Boss seen at full HP: the local player was present from the start.
        t.on_object_spawn(
            500,
            0x10E4,
            &status(
                500,
                vec![stat(StatType::MaxHP, 300_000), stat(StatType::HP, 300_000)],
            ),
            100,
        );
        t.on_object_spawn(600, 0x0321, &status(600, vec![name_stat("Alice")]), 100);

        t.on_damage(500, 600, 5000, 200);
        t.on_local_hit(500, 7, 1000, 1000, 250);
        t.on_object_status(500, &status(500, vec![stat(StatType::HP, 0)]), 500);

        let done = t.on_tick(500);
        let alice = done[0]
            .participants
            .iter()
            .find(|p| p.name == "Alice")
            .expect("Alice tracked");
        assert_eq!(alice.provenance, DamageProvenance::Observed);
    }

    #[test]
    fn tracks_remote_damage_and_kill() {
        let mut t = CombatTracker::new();
        t.on_map_change("Mad Lab", 42, 0);
        t.on_player_loaded(1000, 42);

        // Boss spawns at full HP.
        t.on_object_spawn(
            500,
            0x10E4,
            &status(
                500,
                vec![stat(StatType::MaxHP, 300_000), stat(StatType::HP, 300_000)],
            ),
            100,
        );
        // A remote player spawns with a name + weapon.
        t.on_object_spawn(
            600,
            0x0321,
            &status(
                600,
                vec![name_stat("Alice"), stat(StatType::Inventory0, 1234)],
            ),
            100,
        );

        // Alice deals damage to the boss.
        t.on_damage(500, 600, 5000, 200);
        t.on_damage(500, 600, 5000, 300);
        // The local player also lands a hit (engaged), so the fight is retained
        // even though the boss carries no boss-like catalog labels here.
        t.on_local_hit(500, 7, 1000, 1000, 350);
        assert!(t.on_tick(400).is_empty(), "fight still active");

        // Boss dies.
        t.on_object_status(500, &status(500, vec![stat(StatType::HP, 0)]), 500);
        let done = t.on_tick(500);
        assert_eq!(done.len(), 1);
        let fight = &done[0];
        assert!(fight.killed);
        assert_eq!(fight.dungeon, "Mad Lab");
        assert_eq!(fight.boss_max_hp, 300_000);
        assert_eq!(fight.total_damage(), 10_000);
        // Alice is attributed all observed damage and sorts first.
        let alice = fight
            .participants
            .iter()
            .find(|p| p.name == "Alice")
            .expect("Alice tracked");
        assert_eq!(alice.equipment[0], 1234);
        assert_eq!(alice.provenance, DamageProvenance::Observed);
        assert!(
            fight.participants.iter().any(|p| p.is_local),
            "local participant retained"
        );
    }

    #[test]
    fn participant_captures_skin_and_dyes() {
        let mut t = CombatTracker::new();
        t.on_map_change("Mad Lab", 42, 0);
        t.on_player_loaded(1000, 42);

        t.on_object_spawn(
            500,
            0x10E4,
            &status(
                500,
                vec![stat(StatType::MaxHP, 300_000), stat(StatType::HP, 300_000)],
            ),
            100,
        );
        // Remote player carries a skin id + two dye textures.
        t.on_object_spawn(
            600,
            0x0321,
            &status(
                600,
                vec![
                    name_stat("Alice"),
                    stat(StatType::SkinId, 5678),
                    stat(StatType::Texture1, 0x0100_00ff),
                    stat(StatType::Texture2, 0x0100_ff00),
                ],
            ),
            100,
        );

        t.on_damage(500, 600, 5000, 200);
        t.on_local_hit(500, 7, 1000, 1000, 350);
        t.on_object_status(500, &status(500, vec![stat(StatType::HP, 0)]), 500);
        let done = t.on_tick(500);
        assert_eq!(done.len(), 1);
        let alice = done[0]
            .participants
            .iter()
            .find(|p| p.name == "Alice")
            .expect("Alice tracked");
        // object_type stays the class id; skin/dyes are captured separately.
        assert_eq!(alice.object_type, 0x0321);
        assert_eq!(alice.skin_id, 5678);
        assert_eq!(alice.tex1, 0x0100_00ff);
        assert_eq!(alice.tex2, 0x0100_ff00);
    }
    #[test]
    fn wandering_boss_reentry_is_a_single_fight() {
        // A curated wandering boss (Calamity Crab) that roams out of view and
        // back under new object ids must be recorded as one fight, not several.
        let crab = 47387;
        let mut t = CombatTracker::new();
        t.on_map_change("Deadwater Docks", 7, 0);
        t.on_player_loaded(1000, 42);

        // First engagement, then it wanders off (removed) still alive at 100k HP.
        t.on_object_spawn(
            500,
            crab,
            &status(
                500,
                vec![stat(StatType::MaxHP, 128_000), stat(StatType::HP, 128_000)],
            ),
            100,
        );
        t.on_damage(500, 1000, 4000, 150);
        t.on_object_status(500, &status(500, vec![stat(StatType::HP, 100_000)]), 160);
        assert!(
            t.on_object_removed(500, 200).is_none(),
            "wandering boss suspended, not finalized"
        );

        // Re-entry under a NEW object id; more damage; wanders off again.
        t.on_object_spawn(
            501,
            crab,
            &status(
                501,
                vec![stat(StatType::MaxHP, 128_000), stat(StatType::HP, 100_000)],
            ),
            300,
        );
        t.on_damage(501, 1000, 3000, 350);
        assert!(
            t.on_object_removed(501, 400).is_none(),
            "still suspended on second despawn"
        );

        // Third entry: finally killed for real.
        t.on_object_spawn(
            502,
            crab,
            &status(
                502,
                vec![stat(StatType::MaxHP, 128_000), stat(StatType::HP, 100_000)],
            ),
            500,
        );
        t.on_damage(502, 1000, 5000, 550);
        t.on_object_status(502, &status(502, vec![stat(StatType::HP, 0)]), 560);
        let done = t.on_tick(600);

        assert_eq!(done.len(), 1, "one merged crab fight recorded, not three");
        let fight = &done[0];
        assert!(fight.killed, "final kill carries through the merge");
        assert_eq!(
            fight.boss_start_hp, 128_000,
            "keeps the original engagement's start HP"
        );
        // All three segments' damage is summed under the local player.
        assert_eq!(fight.total_damage(), 12_000);
    }

    #[test]
    fn concurrent_wandering_bosses_collapse_into_one_fight_on_kill() {
        // Defensive: if two instances of a wandering boss are ever in view at
        // once, a kill folds in the suspended one rather than emitting a
        // duplicate (the curated premise is a single roaming instance).
        let crab = 47387;
        let mut t = CombatTracker::new();
        t.on_map_change("Deadwater Docks", 7, 0);
        t.on_player_loaded(1000, 42);

        t.on_object_spawn(
            500,
            crab,
            &status(
                500,
                vec![stat(StatType::MaxHP, 128_000), stat(StatType::HP, 128_000)],
            ),
            100,
        );
        t.on_object_spawn(
            501,
            crab,
            &status(
                501,
                vec![stat(StatType::MaxHP, 128_000), stat(StatType::HP, 128_000)],
            ),
            100,
        );
        t.on_damage(500, 1000, 4000, 150);
        t.on_damage(501, 1000, 3000, 150);

        // One wanders off (suspended); the other is killed and finalized on tick.
        assert!(t.on_object_removed(500, 200).is_none());
        t.on_object_status(501, &status(501, vec![stat(StatType::HP, 0)]), 250);
        let done = t.on_tick(300);
        assert_eq!(
            done.len(),
            1,
            "concurrent instances collapse into one fight on kill"
        );
        assert!(done[0].killed);
        assert_eq!(done[0].total_damage(), 7000);
        // Nothing left suspended to leak at run end.
        assert!(t.on_map_change("Nexus", 0, 400).is_empty());
    }

    #[test]
    fn wandering_boss_that_never_returns_finalizes_once_at_run_end() {
        let crab = 47387;
        let mut t = CombatTracker::new();
        t.on_map_change("Deadwater Docks", 7, 0);
        t.on_player_loaded(1000, 42);

        t.on_object_spawn(
            500,
            crab,
            &status(
                500,
                vec![stat(StatType::MaxHP, 128_000), stat(StatType::HP, 128_000)],
            ),
            100,
        );
        t.on_damage(500, 1000, 4000, 150);
        assert!(
            t.on_object_removed(500, 200).is_none(),
            "suspended on despawn"
        );

        // Leaving the dungeon flushes the suspended crab exactly once, as escaped.
        let done = t.on_map_change("Nexus", 0, 300);
        assert_eq!(
            done.len(),
            1,
            "suspended wandering boss finalized at run end"
        );
        assert!(!done[0].killed, "it escaped -- never brought to 0 HP");
    }

    #[test]
    fn local_hits_are_self_pending() {
        let mut t = CombatTracker::new();
        t.on_map_change("Mad Lab", 1, 0);
        t.on_player_loaded(1000, 42);
        t.on_object_spawn(
            500,
            0x10E4,
            &status(
                500,
                vec![stat(StatType::MaxHP, 50_000), stat(StatType::HP, 50_000)],
            ),
            0,
        );
        // A second player is present so this exercises hit resolution only, not
        // the solo HP-gap reconciliation (which has its own tests).
        t.on_object_spawn(600, 0x0321, &status(600, vec![name_stat("Bob")]), 0);

        t.on_local_hit(500, 10, 1000, 1000, 10);
        t.on_local_hit(500, 20, 1000, 1000, 20);
        t.on_object_status(500, &status(500, vec![stat(StatType::HP, 0)]), 30);
        let done = t.on_tick(30);
        assert_eq!(done.len(), 1);
        let p = &done[0].participants;
        assert_eq!(p.len(), 1);
        assert!(p[0].is_local);
        assert_eq!(p[0].hits, 2);
        assert_eq!(p[0].damage, 0);
        assert_eq!(p[0].provenance, DamageProvenance::SelfPending);
    }

    // --- Self-damage fixtures ---------------------------------------

    #[test]
    fn map_change_seeds_rng_and_enables_self_damage() {
        let mut t = CombatTracker::new();
        assert!(!t.rng_seeded);
        assert!(!t.self_damage_available);
        t.pending_shots.insert(
            5,
            PendingShot {
                base_damage: 10,
                armor_piercing: false,
                ..Default::default()
            },
        );
        t.on_map_change("Mad Lab", 42, 0);
        assert!(t.rng_seeded);
        assert!(t.self_damage_available);
        assert!(t.pending_shots.is_empty(), "shots cleared on new instance");
    }

    #[test]
    fn disconnect_disables_self_damage() {
        let mut t = CombatTracker::new();
        t.on_map_change("Mad Lab", 42, 0);
        t.pending_shots.insert(
            7,
            PendingShot {
                base_damage: 10,
                armor_piercing: false,
                ..Default::default()
            },
        );
        t.on_disconnect(100);
        assert!(!t.rng_seeded);
        assert!(!t.self_damage_available);
        assert!(t.pending_shots.is_empty());
    }

    #[test]
    fn shot_without_seed_disables_self_damage() {
        let mut t = CombatTracker::new();
        // Force the "available but unseeded" edge (capture starting mid-map).
        t.self_damage_available = true;
        t.rng_seeded = false;
        t.on_local_shot(0x333, 0, 1, 0, 0.0, 0.0, 0.0, 0);
        assert!(
            !t.self_damage_available,
            "unseeded shot disables self-damage"
        );
    }

    #[test]
    fn shot_without_attack_stat_disables_self_damage() {
        let mut t = CombatTracker::new();
        t.on_map_change("Mad Lab", 42, 0);
        t.on_player_loaded(1000, 42);
        // Local player object exists but never reported an Attack stat.
        t.on_object_status(1000, &status(1000, vec![stat(StatType::Defense, 10)]), 10);
        t.on_local_shot(0x333, 0, 1, 10, 0.0, 0.0, 0.0, 10);
        assert!(
            !t.self_damage_available,
            "missing attack stat disables self-damage"
        );
    }

    #[test]
    fn resolved_local_hits_are_self_computed() {
        let mut t = CombatTracker::new();
        t.on_map_change("Mad Lab", 42, 0);
        t.on_player_loaded(1000, 42);
        t.on_object_spawn(
            500,
            0x10E4,
            &status(
                500,
                vec![
                    stat(StatType::MaxHP, 50_000),
                    stat(StatType::HP, 50_000),
                    stat(StatType::Defense, 0),
                ],
            ),
            0,
        );
        // A second player is present so this exercises hit resolution only, not
        // the solo HP-gap reconciliation (which has its own tests).
        t.on_object_spawn(600, 0x0321, &status(600, vec![name_stat("Bob")]), 0);

        // Simulate two rolled shots (asset roll path is covered in damage.rs tests).
        t.pending_shots.insert(
            10,
            PendingShot {
                base_damage: 1000,
                armor_piercing: false,
                ..Default::default()
            },
        );
        t.pending_shots.insert(
            20,
            PendingShot {
                base_damage: 500,
                armor_piercing: false,
                ..Default::default()
            },
        );

        t.on_local_hit(500, 10, 1000, 1000, 10);
        t.on_local_hit(500, 20, 1000, 1000, 20);
        t.on_object_status(500, &status(500, vec![stat(StatType::HP, 0)]), 30);
        let done = t.on_tick(30);
        assert_eq!(done.len(), 1);
        let p = &done[0].participants;
        assert_eq!(p.len(), 1);
        assert!(p[0].is_local);
        assert_eq!(p[0].hits, 2);
        assert_eq!(
            p[0].damage, 1500,
            "both shots resolved against zero defense"
        );
        assert_eq!(p[0].provenance, DamageProvenance::SelfComputed);
    }

    #[test]
    fn piercing_bullet_resolves_multiple_hits() {
        let mut t = CombatTracker::new();
        t.on_map_change("Mad Lab", 42, 0);
        t.on_player_loaded(1000, 42);
        t.on_object_spawn(
            500,
            0x10E4,
            &status(
                500,
                vec![
                    stat(StatType::MaxHP, 50_000),
                    stat(StatType::HP, 50_000),
                    stat(StatType::Defense, 0),
                ],
            ),
            0,
        );
        t.on_object_spawn(
            501,
            0x10E4,
            &status(
                501,
                vec![
                    stat(StatType::MaxHP, 50_000),
                    stat(StatType::HP, 50_000),
                    stat(StatType::Defense, 0),
                ],
            ),
            0,
        );

        // One bullet, kept in the pending table so it can hit several targets.
        t.pending_shots.insert(
            10,
            PendingShot {
                base_damage: 700,
                armor_piercing: false,
                ..Default::default()
            },
        );
        t.on_local_hit(500, 10, 1000, 1000, 10);
        t.on_local_hit(501, 10, 1000, 1000, 11);

        assert_eq!(
            t.fights
                .get(&500)
                .unwrap()
                .attackers
                .get(&1000)
                .unwrap()
                .damage,
            700
        );
        assert_eq!(
            t.fights
                .get(&501)
                .unwrap()
                .attackers
                .get(&1000)
                .unwrap()
                .damage,
            700
        );
    }

    #[test]
    fn unresolved_local_hit_is_estimated() {
        let mut t = CombatTracker::new();
        t.on_map_change("Mad Lab", 42, 0);
        t.on_player_loaded(1000, 42);
        t.on_object_spawn(
            500,
            0x10E4,
            &status(
                500,
                vec![
                    stat(StatType::MaxHP, 50_000),
                    stat(StatType::HP, 50_000),
                    stat(StatType::Defense, 0),
                ],
            ),
            0,
        );
        // A second player is present so this exercises hit resolution only, not
        // the solo HP-gap / masked reconciliation (which have their own tests).
        t.on_object_spawn(600, 0x0321, &status(600, vec![name_stat("Bob")]), 0);

        // One resolved shot, one hit with no matching rolled bullet.
        t.pending_shots.insert(
            10,
            PendingShot {
                base_damage: 1000,
                armor_piercing: false,
                ..Default::default()
            },
        );
        t.on_local_hit(500, 10, 1000, 1000, 10);
        t.on_local_hit(500, 99, 1000, 1000, 20);
        t.on_object_status(500, &status(500, vec![stat(StatType::HP, 0)]), 30);
        let done = t.on_tick(30);
        let p = &done[0].participants;
        assert_eq!(p[0].hits, 2);
        // The unresolved hit is estimated from the resolved hit's average (1000),
        // so the total is the resolved 1000 plus one estimated hit of 1000.
        assert_eq!(p[0].damage, 2000);
        assert_eq!(p[0].provenance, DamageProvenance::SelfEstimated);
    }

    #[test]
    fn defense_reduces_local_hit_damage() {
        let mut t = CombatTracker::new();
        t.on_map_change("Mad Lab", 42, 0);
        t.on_player_loaded(1000, 42);
        // Boss with 200 defense.
        t.on_object_spawn(
            500,
            0x10E4,
            &status(
                500,
                vec![
                    stat(StatType::MaxHP, 50_000),
                    stat(StatType::HP, 50_000),
                    stat(StatType::Defense, 200),
                ],
            ),
            0,
        );
        t.pending_shots.insert(
            10,
            PendingShot {
                base_damage: 1000,
                armor_piercing: false,
                ..Default::default()
            },
        );
        t.on_local_hit(500, 10, 1000, 1000, 10);
        // 1000 - 200 defense = 800 (well above the 10% floor of 200).
        assert_eq!(
            t.fights
                .get(&500)
                .unwrap()
                .attackers
                .get(&1000)
                .unwrap()
                .damage,
            800
        );
    }

    #[test]
    fn o3_guarded_damage_tracked_separately() {
        let mut t = CombatTracker::new();
        t.on_map_change("Oryx's Sanctuary", 42, 0);
        t.on_player_loaded(1000, 42);
        // O3 boss, defense 0 so resolved damage equals the rolled base.
        t.on_object_spawn(
            500,
            O3_BOSS_TYPE,
            &status(
                500,
                vec![
                    stat(StatType::MaxHP, 1_000_000),
                    stat(StatType::HP, 1_000_000),
                    stat(StatType::Defense, 0),
                ],
            ),
            0,
        );

        // Hit while not guarded: counts to total only.
        t.pending_shots.insert(
            10,
            PendingShot {
                base_damage: 1000,
                armor_piercing: false,
                ..Default::default()
            },
        );
        t.on_local_hit(500, 10, 1000, 1000, 10);

        // Boss raises his shield (guard animation): the next hit is "guarded".
        t.on_object_status(
            500,
            &status(500, vec![stat(StatType::Animation, O3_GUARD_ANIM[0])]),
            20,
        );
        t.pending_shots.insert(
            11,
            PendingShot {
                base_damage: 1000,
                armor_piercing: false,
                ..Default::default()
            },
        );
        t.on_local_hit(500, 11, 1000, 1000, 30);

        // Shield drops: guarded totals must not grow further.
        t.on_object_status(500, &status(500, vec![stat(StatType::Animation, 0)]), 40);
        t.pending_shots.insert(
            12,
            PendingShot {
                base_damage: 1000,
                armor_piercing: false,
                ..Default::default()
            },
        );
        t.on_local_hit(500, 12, 1000, 1000, 50);

        let a = t.fights.get(&500).unwrap().attackers.get(&1000).unwrap();
        assert_eq!(a.damage, 3000);
        assert_eq!(a.hits, 3);
        assert_eq!(a.guarded_damage, 1000);
        assert_eq!(a.guarded_hits, 1);
    }

    #[test]
    fn ignores_damage_to_non_boss() {
        let mut t = CombatTracker::new();
        t.on_map_change("Mad Lab", 1, 0);
        // Small enemy below the boss HP threshold.
        t.on_object_spawn(
            700,
            0x1000,
            &status(
                700,
                vec![stat(StatType::MaxHP, 500), stat(StatType::HP, 500)],
            ),
            0,
        );
        t.on_damage(700, 600, 100, 10);
        assert!(t.on_tick(10).is_empty());
        // No fight was created.
        assert!(t.fights.is_empty());
    }

    #[test]
    fn map_change_finalizes_active_fight() {
        let mut t = CombatTracker::new();
        t.on_map_change("Mad Lab", 1, 0);
        t.on_player_loaded(1000, 1);
        t.on_object_spawn(
            500,
            0x10E4,
            &status(
                500,
                vec![stat(StatType::MaxHP, 100_000), stat(StatType::HP, 100_000)],
            ),
            0,
        );
        t.on_object_spawn(600, 0x0400, &status(600, vec![name_stat("Bob")]), 0);
        t.on_damage(500, 600, 1000, 10);
        t.on_local_hit(500, 7, 1000, 1000, 12); // local engaged (untagged boss needs engagement)
        let done = t.on_map_change("Nexus", 0, 100);
        assert_eq!(done.len(), 1);
        assert!(!done[0].killed);
        assert_eq!(done[0].total_damage(), 1000);
    }

    #[test]
    fn removal_at_low_hp_counts_as_kill() {
        let mut t = CombatTracker::new();
        t.on_map_change("Realm", 1, 0);
        t.on_player_loaded(1000, 1);
        t.on_object_spawn(
            500,
            0x10E4,
            &status(
                500,
                vec![stat(StatType::MaxHP, 100_000), stat(StatType::HP, 100_000)],
            ),
            0,
        );
        t.on_damage(500, 600, 1000, 10);
        t.on_local_hit(500, 7, 1000, 1000, 12); // local engaged
                                                // Boss drops to a sliver of HP, then despawns without an explicit HP=0 tick.
        t.on_object_status(500, &status(500, vec![stat(StatType::HP, 500)]), 20);
        let done = t.on_object_removed(500, 30).expect("fight finalized");
        assert!(
            done.killed,
            "boss removed at <=10% HP should count as killed"
        );
    }

    #[test]
    fn boss_kill_finalizes_aggregated_adds_without_map_change() {
        // Killing the Legion Missionary must surface its aggregated orb rows on
        // the boss card immediately, not only when the map later changes.
        let mut t = CombatTracker::new();
        t.on_map_change("Realm", 1, 0);
        t.on_player_loaded(1000, 1);
        // The Missionary anchor (uncatalogued high-HP boss in tests).
        t.on_object_spawn(
            500,
            53013,
            &status(
                500,
                vec![stat(StatType::MaxHP, 200_000), stat(StatType::HP, 200_000)],
            ),
            0,
        );
        t.on_damage(500, 600, 1000, 10);
        t.on_local_hit(500, 7, 1000, 1000, 12); // local engaged on the boss
                                                // A Holy Orb add is fought during the encounter (routed to its aux row).
        t.on_object_spawn(
            700,
            53017,
            &status(
                700,
                vec![stat(StatType::MaxHP, 12_500), stat(StatType::HP, 12_500)],
            ),
            15,
        );
        t.on_damage(700, 600, 5000, 16);
        t.on_local_hit(700, 8, 1000, 1000, 17); // local engaged on the orb
                                                // The boss dies; no map change follows.
        t.on_object_status(500, &status(500, vec![stat(StatType::HP, 0)]), 25);
        let boss = t.on_object_removed(500, 30).expect("boss finalized");
        assert!(boss.killed);
        // The orb row surfaces on the next tick, still on the same map.
        let done = t.on_tick(40);
        assert!(
            done.iter().any(|cf| cf.boss_name == "Orb of Light"),
            "aggregated orb row must finalize on the boss kill, before any map change",
        );
    }

    #[test]
    fn phase_reset_boss_dipping_low_then_full_counts_as_kill() {
        // Lair of Draconis dragons drop to ~0, then a phase transition resets HP
        // to full under the same object id before the object is removed, leaving
        // the last-seen HP misleadingly high. The minimum HP proves the kill.
        let mut t = CombatTracker::new();
        t.on_map_change("Lair of Draconis", 1, 0);
        t.on_player_loaded(1000, 1);
        t.on_object_spawn(
            500,
            0xB176,
            &status(
                500,
                vec![stat(StatType::MaxHP, 146_250), stat(StatType::HP, 146_250)],
            ),
            0,
        );
        t.on_damage(500, 600, 1000, 10);
        t.on_local_hit(500, 7, 1000, 1000, 12); // local engaged
                                                // Dips well below 10%...
        t.on_object_status(500, &status(500, vec![stat(StatType::HP, 9_671)]), 20);
        // ...then the phase transition resets HP to full.
        t.on_object_status(500, &status(500, vec![stat(StatType::HP, 146_250)]), 30);
        t.on_object_status(500, &status(500, vec![stat(StatType::HP, 140_499)]), 40);
        let done = t.on_object_removed(500, 50).expect("fight finalized");
        assert!(
            done.killed,
            "boss that dipped <=10% before a phase reset should count as killed"
        );
    }

    #[test]
    fn self_destruct_boss_full_damage_counts_as_kill() {
        // Kogbold Expedition Engine (type 22109) explodes when killed: the object
        // is removed at high HP with no zero/<=10% tick, so the HP heuristic never
        // fires. A party that dealt ~all of its remaining HP still cleared it.
        let mut t = CombatTracker::new();
        t.on_map_change("Realm", 1, 0);
        t.on_player_loaded(1000, 1);
        t.on_object_spawn(
            500,
            22109,
            &status(
                500,
                vec![stat(StatType::MaxHP, 100_000), stat(StatType::HP, 100_000)],
            ),
            0,
        );
        t.on_damage(500, 600, 90_000, 10); // party deals ~all remaining HP
        t.on_local_hit(500, 7, 1000, 1000, 12); // local engaged
                                                // Self-destruct: object removed while HP is still high (no lethal tick).
        let done = t.on_object_removed(500, 30).expect("fight finalized");
        assert!(
            done.killed,
            "self-destruct boss cleared at full damage should count as killed"
        );
    }

    #[test]
    fn self_destruct_boss_partial_damage_stays_escaped() {
        // The same train can genuinely escape (reach the end of its track) when
        // the party fails: only partial HP was dealt, so it must stay Escaped.
        let mut t = CombatTracker::new();
        t.on_map_change("Realm", 1, 0);
        t.on_player_loaded(1000, 1);
        t.on_object_spawn(
            500,
            22109,
            &status(
                500,
                vec![stat(StatType::MaxHP, 100_000), stat(StatType::HP, 100_000)],
            ),
            0,
        );
        t.on_damage(500, 600, 50_000, 10); // only half the HP dealt
        t.on_local_hit(500, 7, 1000, 1000, 12); // local engaged
                                                // Removal suspends the fight (it may re-enter); a disconnect flushes it
                                                // as the escaped fight it is.
        assert!(
            t.on_object_removed(500, 30).is_none(),
            "un-killed removal is suspended"
        );
        let done = t.on_disconnect(100);
        assert_eq!(done.len(), 1, "suspended escape flushes on disconnect");
        assert!(
            !done[0].killed,
            "self-destruct boss that escaped at partial damage must stay escaped"
        );
    }

    #[test]
    fn realm_boss_resumes_same_object_id_and_combines_damage() {
        // Fight a realm boss, walk out of range (it despawns un-killed), then
        // return and finish it: the same object id must resume the same fight so
        // the two engagements' damage is combined into one record (issue: realm
        // re-engagement duplicated fights).
        let mut t = CombatTracker::new();
        t.on_map_change("Realm", 1, 0);
        t.on_player_loaded(1000, 1);
        t.on_object_spawn(
            500,
            0x10E4,
            &status(
                500,
                vec![stat(StatType::MaxHP, 100_000), stat(StatType::HP, 100_000)],
            ),
            0,
        );
        t.on_object_spawn(600, 0x0400, &status(600, vec![name_stat("Bob")]), 0);
        t.on_damage(500, 600, 30_000, 10);
        t.on_local_hit(500, 7, 1000, 1000, 12); // local engaged so the fight is kept
                                                // Walk out of range: the boss despawns un-killed and is suspended.
        assert!(
            t.on_object_removed(500, 20).is_none(),
            "un-killed despawn is suspended"
        );
        assert!(t.on_tick(30).is_empty(), "nothing surfaces while suspended");
        // Return within the window: the same object id re-enters and is finished.
        t.on_object_spawn(
            500,
            0x10E4,
            &status(
                500,
                vec![stat(StatType::MaxHP, 100_000), stat(StatType::HP, 40_000)],
            ),
            1000,
        );
        t.on_damage(500, 600, 40_000, 1010);
        t.on_object_status(500, &status(500, vec![stat(StatType::HP, 0)]), 1020);
        let done = t.on_tick(1020);
        assert_eq!(
            done.len(),
            1,
            "re-engagement resumes the same fight, not a new one"
        );
        assert!(done[0].killed);
        let bob = done[0]
            .participants
            .iter()
            .find(|p| p.name == "Bob")
            .expect("attacker Bob");
        assert_eq!(
            bob.damage, 70_000,
            "damage from both engagements is combined"
        );
    }

    #[test]
    fn suspended_boss_finalizes_as_escaped_after_resume_window() {
        // A realm boss walked away from and never returned within the window is
        // finalized as escaped so it still surfaces in history.
        let mut t = CombatTracker::new();
        t.on_map_change("Realm", 1, 0);
        t.on_player_loaded(1000, 1);
        t.on_object_spawn(
            500,
            0x10E4,
            &status(
                500,
                vec![stat(StatType::MaxHP, 100_000), stat(StatType::HP, 100_000)],
            ),
            0,
        );
        t.on_damage(500, 600, 30_000, 10);
        t.on_local_hit(500, 7, 1000, 1000, 12);
        assert!(
            t.on_object_removed(500, 20).is_none(),
            "suspended on despawn"
        );
        assert!(t.on_tick(1000).is_empty(), "still within the resume window");
        let done = t.on_tick(20 + BOSS_RESUME_WINDOW_MS);
        assert_eq!(done.len(), 1, "flushed as escaped once the window elapses");
        assert!(!done[0].killed);
    }

    #[test]
    fn reused_object_id_for_different_boss_type_evicts_stale_segment() {
        // If the server recycles a suspended boss's object id for a different
        // boss type, the stale segment is evicted (escaped) rather than merged.
        let mut t = CombatTracker::new();
        t.on_map_change("Realm", 1, 0);
        t.on_player_loaded(1000, 1);
        t.on_object_spawn(
            500,
            0x10E4,
            &status(
                500,
                vec![stat(StatType::MaxHP, 100_000), stat(StatType::HP, 100_000)],
            ),
            0,
        );
        t.on_damage(500, 600, 30_000, 10);
        t.on_local_hit(500, 7, 1000, 1000, 12);
        assert!(
            t.on_object_removed(500, 20).is_none(),
            "suspended on despawn"
        );
        // Same id, different boss type re-enters and is engaged.
        t.on_object_spawn(
            500,
            0x10E5,
            &status(
                500,
                vec![stat(StatType::MaxHP, 80_000), stat(StatType::HP, 80_000)],
            ),
            1000,
        );
        t.on_damage(500, 700, 10_000, 1010);
        t.on_local_hit(500, 8, 1000, 1000, 1012);
        // The evicted stale segment surfaces on the next tick as escaped.
        let done = t.on_tick(1020);
        assert_eq!(done.len(), 1, "stale segment evicted as its own fight");
        assert_eq!(done[0].boss_object_type, 0x10E4);
        assert!(!done[0].killed);
        // The fresh fight is still active under the same id.
        assert!(t.fights.contains_key(&500), "new-type fight started fresh");
    }

    #[test]
    fn marble_colossus_heal_back_splits_into_two_comings() {
        // Marble Colossus (type 45073) revives with a full HP pool after its
        // survival phase. The near-dead -> near-full HP jump must split the fight
        // into a pre-survival and post-survival segment, each recorded separately.
        let mut t = CombatTracker::new();
        t.on_map_change("Lost Halls", 1, 0);
        t.on_player_loaded(1000, 1);
        t.on_object_spawn(
            500,
            45073,
            &status(
                500,
                vec![stat(StatType::MaxHP, 500_000), stat(StatType::HP, 500_000)],
            ),
            0,
        );

        // Local player fights the pre-survival segment down to a sliver.
        t.on_damage(500, 1000, 5000, 10);
        t.on_object_status(500, &status(500, vec![stat(StatType::HP, 40_000)]), 20);
        // He heals back to full: this emits the finalized pre-survival segment.
        let split = t.on_object_status(500, &status(500, vec![stat(StatType::HP, 460_000)]), 30);
        assert_eq!(
            split.len(),
            1,
            "heal-back finalizes the pre-survival segment"
        );
        assert!(split[0].killed);
        assert!(
            split[0].boss_name.ends_with("(Pre-survival)"),
            "got {}",
            split[0].boss_name
        );
        // The pre-survival "kill" never reached 0 HP (heal-back), so its solo gap
        // is NOT reconciled: it reports exactly the damage observed.
        assert_eq!(
            split[0].total_damage(),
            5000,
            "pre-survival damage is independent"
        );

        // Post-survival is now active under the same object id; fight it to death.
        t.on_damage(500, 1000, 3000, 40);
        t.on_object_status(500, &status(500, vec![stat(StatType::HP, 0)]), 50);
        let done = t.on_tick(50);
        assert_eq!(done.len(), 1);
        assert!(done[0].killed);
        assert!(
            done[0].boss_name.ends_with("(Post-survival)"),
            "got {}",
            done[0].boss_name
        );
        // Post-survival is recorded as its own segment seeded from the boss's full
        // (rescaled) pool. This solo, fully-observed kill reached 0 HP, so the
        // unattributed remainder -- the local player's uncaptured ability damage --
        // is reconciled onto the local row up to the pool.
        let local = done[0]
            .participants
            .iter()
            .find(|p| p.is_local)
            .expect("local row");
        assert_eq!(local.provenance, DamageProvenance::SelfReconciled);
        assert_eq!(
            done[0].total_damage(),
            500_000,
            "post-survival gap reconciled to pool"
        );
    }

    #[test]
    fn phase_split_does_not_flush_aggregated_adds_early() {
        // The Marble Colossus pre-survival heal-back finalizes a segment flagged
        // "killed", but it is not the real death: the run's aggregated Core/Pillar
        // rows must NOT flush there -- only on the true
        // post-survival kill.
        let mut t = CombatTracker::new();
        t.on_map_change("Lost Halls", 1, 0);
        t.on_player_loaded(1000, 1);
        t.on_object_spawn(
            500,
            45073,
            &status(
                500,
                vec![stat(StatType::MaxHP, 500_000), stat(StatType::HP, 500_000)],
            ),
            0,
        );
        // A Marble Core add is fought (routed to the aggregated marble_core row).
        // Sub-boss-threshold HP keeps it out of the boss path so it aggregates.
        t.on_object_spawn(
            700,
            45116,
            &status(
                700,
                vec![stat(StatType::MaxHP, 5_000), stat(StatType::HP, 5_000)],
            ),
            5,
        );
        t.on_damage(700, 600, 3000, 6);
        t.on_local_hit(700, 8, 1000, 1000, 7);

        // Colossus pre-survival down to a sliver, then heal-back to full: splits.
        t.on_damage(500, 1000, 5000, 10);
        t.on_object_status(500, &status(500, vec![stat(StatType::HP, 40_000)]), 20);
        let split = t.on_object_status(500, &status(500, vec![stat(StatType::HP, 460_000)]), 30);
        assert!(
            split.iter().all(|cf| cf.boss_name != "Marble Core"),
            "aggregated core row must not flush on the phase split",
        );

        // Kill the post-survival segment: the core row now flushes onto the card.
        t.on_object_status(500, &status(500, vec![stat(StatType::HP, 0)]), 50);
        let mut names: Vec<String> = t.on_tick(60).into_iter().map(|cf| cf.boss_name).collect();
        names.extend(t.on_tick(70).into_iter().map(|cf| cf.boss_name));
        assert!(
            names.iter().any(|n| n == "Marble Core"),
            "aggregated core row flushes on the real (post-survival) kill, got {names:?}",
        );
    }

    #[test]
    fn marble_colossus_gradual_heal_back_still_splits() {
        // Regression: the survival-phase heal-back can arrive as several
        // rising HP ticks rather than one jump. Gating the split on the previous
        // tick would ratchet the baseline back above the threshold before HP
        // reached full, dropping the post-survival segment. Gating on the lowest
        // HP ever seen must still split.
        let mut t = CombatTracker::new();
        t.on_map_change("Lost Halls", 1, 0);
        t.on_player_loaded(1000, 1);
        t.on_object_spawn(
            500,
            45073,
            &status(
                500,
                vec![stat(StatType::MaxHP, 500_000), stat(StatType::HP, 500_000)],
            ),
            0,
        );

        // Pre-survival down to the ~10% survival trigger.
        t.on_damage(500, 1000, 5000, 10);
        t.on_object_status(500, &status(500, vec![stat(StatType::HP, 45_000)]), 20);
        // Heal-back climbs across multiple ticks: 9% -> 40% -> 70% -> 96%. The
        // split fires on the first tick clearing the 30% survival-exit gate; the
        // later ticks must not finalize a second segment.
        let s1 = t.on_object_status(500, &status(500, vec![stat(StatType::HP, 200_000)]), 30);
        let s2 = t.on_object_status(500, &status(500, vec![stat(StatType::HP, 350_000)]), 40);
        let s3 = t.on_object_status(500, &status(500, vec![stat(StatType::HP, 480_000)]), 50);
        let all: Vec<_> = s1.into_iter().chain(s2).chain(s3).collect();
        assert_eq!(
            all.len(),
            1,
            "gradual heal-back still finalizes one pre-survival segment"
        );
        assert!(
            all[0].boss_name.ends_with("(Pre-survival)"),
            "got {}",
            all[0].boss_name
        );

        // Post-survival is active and finalizes as its own segment on the kill.
        t.on_damage(500, 1000, 3000, 60);
        t.on_object_status(500, &status(500, vec![stat(StatType::HP, 0)]), 70);
        let done = t.on_tick(70);
        assert_eq!(done.len(), 1);
        assert!(
            done[0].boss_name.ends_with("(Post-survival)"),
            "got {}",
            done[0].boss_name
        );
    }

    #[test]
    fn marble_colossus_taunt_splits_and_uses_rescaled_pool() {
        // The survival-exit taunt ("...!" from the boss's own object id) is the
        // authoritative split signal: it fires while the boss is still at the
        // ~10% survival low, before any heal-back HP tick. The post-survival pool
        // must come from the boss's live (rescaled) max HP, not the fight-start
        // value -- players die/nexus during survival, so dungeon HP-scaling drops.
        let mut t = CombatTracker::new();
        t.on_map_change("Lost Halls", 1, 0);
        t.on_player_loaded(1000, 1);
        t.on_object_spawn(
            500,
            45073,
            &status(
                500,
                vec![stat(StatType::MaxHP, 500_000), stat(StatType::HP, 500_000)],
            ),
            0,
        );

        // Pre-survival down to the survival low.
        t.on_damage(500, 1000, 5000, 10);
        t.on_object_status(500, &status(500, vec![stat(StatType::HP, 40_000)]), 20);

        // Survival-exit taunt fires while still near 10% HP: split immediately,
        // before any heal-back tick is observed.
        let split = t.on_boss_text(500, "...!", 25);
        assert_eq!(split.len(), 1, "taunt finalizes the pre-survival segment");
        assert!(
            split[0].boss_name.ends_with("(Pre-survival)"),
            "got {}",
            split[0].boss_name
        );
        assert!(split[0].killed);

        // Second coming: the boss heals to full, but its max HP rescaled down to
        // 300k (players left during survival). The post pool must track that.
        t.on_object_status(
            500,
            &status(
                500,
                vec![stat(StatType::MaxHP, 300_000), stat(StatType::HP, 300_000)],
            ),
            30,
        );
        t.on_damage(500, 1000, 3000, 40);
        t.on_object_status(500, &status(500, vec![stat(StatType::HP, 0)]), 50);
        let done = t.on_tick(50);
        assert_eq!(done.len(), 1);
        assert!(
            done[0].boss_name.ends_with("(Post-survival)"),
            "got {}",
            done[0].boss_name
        );
        assert_eq!(
            done[0].boss_start_hp, 300_000,
            "post pool = rescaled full max"
        );
        assert_eq!(done[0].boss_max_hp, 300_000);
    }

    #[test]
    fn post_survival_pool_ignores_invulnerable_phase_hits() {
        // While the boss heals through its second coming it is Invulnerable, so
        // local shots that land resolve to 0 damage and are not counted as hits.
        // Those (dropped) hits must NOT freeze the post-survival pool: a
        // rescaled-down MaxHP packet arriving after them must still update the
        // denominator.
        let mut t = CombatTracker::new();
        t.on_map_change("Lost Halls", 1, 0);
        t.on_player_loaded(1000, 1);
        t.on_object_spawn(
            500,
            45073,
            &status(
                500,
                vec![
                    stat(StatType::MaxHP, 500_000),
                    stat(StatType::HP, 500_000),
                    stat(StatType::Defense, 0),
                ],
            ),
            0,
        );

        t.on_damage(500, 1000, 5000, 10);
        t.on_object_status(500, &status(500, vec![stat(StatType::HP, 40_000)]), 20);
        // Survival-exit taunt splits while still near the survival low.
        assert_eq!(t.on_boss_text(500, "...!", 25).len(), 1);

        // Heal climbs while Invulnerable; max still reads the pre-rescale pool.
        t.on_object_status(
            500,
            &status(
                500,
                vec![
                    stat(StatType::Condition, 0x0100_0000),
                    stat(StatType::MaxHP, 500_000),
                    stat(StatType::HP, 100_000),
                ],
            ),
            30,
        );
        // A shot lands during invulnerability: it deals no damage and must not
        // be counted as a hit or create a local attacker entry.
        t.self_damage_available = true;
        t.pending_shots.insert(
            10,
            PendingShot {
                base_damage: 1000,
                armor_piercing: false,
                ..Default::default()
            },
        );
        t.on_local_hit(500, 10, 1000, 1000, 35);
        assert_eq!(
            t.fights
                .get(&500)
                .unwrap()
                .attackers
                .get(&1000)
                .map(|a| a.hits)
                .unwrap_or(0),
            0,
            "invuln hit is not counted for the local player",
        );

        // The rescaled MaxHP now arrives (players died during survival). Because
        // no positive damage has landed yet, the pool must follow it down.
        t.on_object_status(
            500,
            &status(
                500,
                vec![
                    stat(StatType::Condition, 0x0100_0000),
                    stat(StatType::MaxHP, 300_000),
                    stat(StatType::HP, 300_000),
                ],
            ),
            40,
        );

        // Rage begins: real damage lands (no longer invulnerable), then the kill.
        t.on_object_status(500, &status(500, vec![stat(StatType::HP, 250_000)]), 45);
        t.on_damage(500, 1000, 3000, 46);
        t.on_object_status(500, &status(500, vec![stat(StatType::HP, 0)]), 50);
        let done = t.on_tick(50);
        assert_eq!(done.len(), 1);
        assert!(
            done[0].boss_name.ends_with("(Post-survival)"),
            "got {}",
            done[0].boss_name
        );
        assert_eq!(
            done[0].boss_start_hp, 300_000,
            "pool follows the rescale past invuln hits"
        );
        assert_eq!(done[0].boss_max_hp, 300_000);
    }

    #[test]
    fn marble_colossus_hp_fallback_splits_without_taunt() {
        // If the survival-exit taunt is culled, the HP heuristic still splits on
        // a heal-back climbing past 30% of max -- even when the >=90% peak tick
        // is never observed (the old gate required it and dropped the post
        // segment entirely).
        let mut t = CombatTracker::new();
        t.on_map_change("Lost Halls", 1, 0);
        t.on_player_loaded(1000, 1);
        t.on_object_spawn(
            500,
            45073,
            &status(
                500,
                vec![stat(StatType::MaxHP, 500_000), stat(StatType::HP, 500_000)],
            ),
            0,
        );

        t.on_damage(500, 1000, 5000, 10);
        t.on_object_status(500, &status(500, vec![stat(StatType::HP, 40_000)]), 20);
        // Only a ~60% heal tick is ever observed (the peak is culled): the split
        // must still fire.
        let split = t.on_object_status(500, &status(500, vec![stat(StatType::HP, 300_000)]), 30);
        assert_eq!(
            split.len(),
            1,
            "HP fallback splits without observing the >=90% peak"
        );
        assert!(
            split[0].boss_name.ends_with("(Pre-survival)"),
            "got {}",
            split[0].boss_name
        );

        // No rescale here: the post pool defaults to the full max HP.
        t.on_damage(500, 1000, 3000, 40);
        t.on_object_status(500, &status(500, vec![stat(StatType::HP, 0)]), 50);
        let done = t.on_tick(50);
        assert_eq!(done.len(), 1);
        assert!(
            done[0].boss_name.ends_with("(Post-survival)"),
            "got {}",
            done[0].boss_name
        );
        assert_eq!(done[0].boss_start_hp, 500_000, "post pool = full max HP");
    }

    #[test]
    fn boss_taunt_ignores_non_transition_and_wrong_object() {
        // The taunt split only fires for the exact survival-exit line, bound to
        // an active pre-survival fight's own object id.
        let mut t = CombatTracker::new();
        t.on_map_change("Lost Halls", 1, 0);
        t.on_player_loaded(1000, 1);
        t.on_object_spawn(
            500,
            45073,
            &status(
                500,
                vec![stat(StatType::MaxHP, 500_000), stat(StatType::HP, 500_000)],
            ),
            0,
        );
        t.on_damage(500, 1000, 5000, 10);
        t.on_object_status(500, &status(500, vec![stat(StatType::HP, 40_000)]), 20);
        // A different boss line must not split.
        assert!(t.on_boss_text(500, "Fear the halls!", 22).is_empty());
        // The transition taunt from an unrelated object id must not split.
        assert!(t.on_boss_text(999, "...!", 23).is_empty());
        // Still a single, un-split pre-survival fight in progress.
        assert_eq!(t.fights.len(), 1);
        assert!(!t.fights.get(&500).unwrap().split_done);
    }

    #[test]
    fn second_coming_is_not_flagged_partially_observed() {
        // Regression: a heal-back post-survival segment starts below max HP by
        // design. That must NOT be mistaken for the local player joining late.
        let mut t = CombatTracker::new();
        t.on_map_change("Lost Halls", 1, 0);
        t.on_player_loaded(1000, 1);
        t.on_object_spawn(
            500,
            45073,
            &status(
                500,
                vec![stat(StatType::MaxHP, 500_000), stat(StatType::HP, 500_000)],
            ),
            0,
        );
        t.on_object_spawn(600, 0x0321, &status(600, vec![name_stat("Alice")]), 0);

        // Pre-survival down to a sliver, then heal-back to near-full (below max).
        t.on_damage(500, 1000, 5000, 10);
        t.on_object_status(500, &status(500, vec![stat(StatType::HP, 40_000)]), 20);
        t.on_object_status(500, &status(500, vec![stat(StatType::HP, 460_000)]), 30);

        // Post-survival: a remote player deals damage; the local player was here
        // the whole time, so the remote figure stays Observed, not partial.
        t.on_damage(500, 600, 3000, 40);
        t.on_local_hit(500, 7, 1000, 1000, 45);
        t.on_object_status(500, &status(500, vec![stat(StatType::HP, 0)]), 50);
        let done = t.on_tick(50);
        assert_eq!(done.len(), 1);
        let alice = done[0]
            .participants
            .iter()
            .find(|p| p.name == "Alice")
            .expect("Alice tracked");
        assert_eq!(alice.provenance, DamageProvenance::Observed);
    }

    #[test]
    fn marble_core_damage_is_aggregated_into_one_summary() {
        // Marble Cores (type 45116) are not their own boss fights; damage to any of
        // them across the run folds into a single "Marble Core" summary entry.
        let mut t = CombatTracker::new();
        t.on_map_change("Lost Halls", 1, 0);
        t.on_player_loaded(1000, 1);
        // Two core objects, below the boss HP threshold so they take the aux path.
        t.on_object_spawn(
            700,
            45116,
            &status(
                700,
                vec![stat(StatType::MaxHP, 5_000), stat(StatType::HP, 5_000)],
            ),
            0,
        );
        t.on_object_spawn(
            701,
            45116,
            &status(
                701,
                vec![stat(StatType::MaxHP, 5_000), stat(StatType::HP, 5_000)],
            ),
            0,
        );
        // A remote player with a name so aux participant identity is snapshotted.
        t.on_object_spawn(600, 0x0400, &status(600, vec![name_stat("Bob")]), 0);

        t.on_damage(700, 1000, 4000, 10);
        t.on_damage(701, 600, 2000, 20);
        // Cores despawn before the run ends; snapshot must survive.
        t.on_object_removed(700, 25);
        t.on_object_removed(701, 25);

        let done = t.on_disconnect(100);
        let core = done
            .iter()
            .find(|f| f.boss_name == "Marble Core")
            .expect("core summary emitted");
        assert!(!core.killed, "aggregate summary is never a kill");
        assert_eq!(core.total_damage(), 6000);
        assert!(
            core.participants.iter().any(|p| p.name == "Bob"),
            "snapshotted remote survives despawn"
        );
        assert!(core.participants.iter().any(|p| p.is_local));
    }

    #[test]
    fn spectral_key_summary_counts_all_spawned_instances_and_sums_hp() {
        // All six Spectral Keys (type 23804) are counted toward the aggregate row
        // and their max HP summed, even though the local player damaged only one
        // and a remote only one -- four keys take no observed damage at all.
        let mut t = CombatTracker::new();
        t.on_map_change("Spectral Penitentiary", 1, 0);
        t.on_player_loaded(1000, 1);
        for id in 700..706 {
            t.on_object_spawn(
                id,
                23804,
                &status(
                    id,
                    vec![stat(StatType::MaxHP, 2_000), stat(StatType::HP, 2_000)],
                ),
                0,
            );
        }
        t.on_object_spawn(600, 0x0400, &status(600, vec![name_stat("Bob")]), 0);
        t.on_damage(700, 1000, 1500, 10); // local damages one key
        t.on_damage(701, 600, 1500, 20); // remote damages another
        for id in 700..706 {
            t.on_object_removed(id, 25);
        }

        let done = t.on_disconnect(100);
        let keys = done
            .iter()
            .find(|f| f.boss_name == "Spectral Key")
            .expect("spectral key summary emitted");
        assert_eq!(
            keys.aux_member_count,
            Some(6),
            "all six spawned keys counted"
        );
        assert_eq!(keys.boss_max_hp, 12_000, "summed max HP of all six keys");
        assert_eq!(
            keys.boss_start_hp, 12_000,
            "start HP mirrors the summed pool"
        );
    }

    #[test]
    fn lobotomik_sentipede_head_and_segments_fold_into_one_uncounted_row() {
        // The Sentipede form (LBT Transformation 4) is one head (23961) plus many
        // body-segment instances (4B 23934 / 4T 23935). All fold into a single
        // "Doctor Lobotomik" row drawn with the head sprite; damage to any part
        // sums into it, and the instance count is suppressed (no "xN").
        let mut t = CombatTracker::new();
        t.on_map_change("Spectral Penitentiary", 1, 0);
        t.on_player_loaded(1000, 1);
        t.on_object_spawn(
            700,
            23961,
            &status(
                700,
                vec![stat(StatType::MaxHP, 175_500), stat(StatType::HP, 175_500)],
            ),
            0,
        );
        for (i, id) in (701..706).enumerate() {
            let otype = if i % 2 == 0 { 23934 } else { 23935 };
            t.on_object_spawn(
                id,
                otype,
                &status(
                    id,
                    vec![stat(StatType::MaxHP, 43_875), stat(StatType::HP, 43_875)],
                ),
                0,
            );
        }
        t.on_damage(700, 1000, 5000, 10); // local hits the head
        t.on_damage(701, 1000, 3000, 20); // local hits a body segment
        for id in 700..706 {
            t.on_object_removed(id, 25);
        }

        let done = t.on_disconnect(100);
        let sentipede = done
            .iter()
            .find(|f| f.boss_name == "Doctor Lobotomik")
            .expect("sentipede summary emitted");
        assert_eq!(
            sentipede.boss_object_type, 23961,
            "row uses the head sprite"
        );
        assert_eq!(
            sentipede.aux_member_count, None,
            "instance count suffix suppressed"
        );
        assert_eq!(
            sentipede.total_damage(),
            8000,
            "head + segment damage summed"
        );
        // 175500 head + 5 * 43875 segments = 394875.
        assert_eq!(
            sentipede.boss_max_hp, 394_875,
            "summed max HP of head + segments"
        );
    }

    #[test]
    fn removal_at_high_hp_is_not_a_kill() {
        let mut t = CombatTracker::new();
        t.on_map_change("Realm", 1, 0);
        t.on_player_loaded(1000, 1);
        t.on_object_spawn(
            500,
            0x10E4,
            &status(
                500,
                vec![stat(StatType::MaxHP, 100_000), stat(StatType::HP, 100_000)],
            ),
            0,
        );
        t.on_damage(500, 600, 1000, 10);
        t.on_local_hit(500, 7, 1000, 1000, 12); // local engaged
                                                // Boss walks off screen at full HP - a despawn, not a kill. It is
                                                // suspended for possible re-entry, then flushed as escaped on disconnect.
        assert!(
            t.on_object_removed(500, 30).is_none(),
            "un-killed despawn is suspended"
        );
        let done = t.on_disconnect(100);
        assert_eq!(done.len(), 1, "suspended despawn flushes on disconnect");
        assert!(
            !done[0].killed,
            "boss removed at high HP should not count as killed"
        );
    }

    #[test]
    fn invincible_add_with_hits_but_no_damage_is_dropped() {
        let mut t = CombatTracker::new();
        t.on_map_change("Cursed Library", 1, 0);
        t.on_player_loaded(1000, 42);
        // Book Bomb: caught by the MaxHP>=10000 fallback but invincible.
        t.on_object_spawn(
            500,
            0xABA9,
            &status(
                500,
                vec![stat(StatType::MaxHP, 10_000), stat(StatType::HP, 10_000)],
            ),
            0,
        );
        // Local hits land but deal 0 damage (no rolled shot resolves to damage).
        t.pending_shots.insert(
            10,
            PendingShot {
                base_damage: 0,
                armor_piercing: false,
                ..Default::default()
            },
        );
        t.on_local_hit(500, 10, 1000, 1000, 10);
        let done = t.on_map_change("Nexus", 0, 100);
        assert!(
            done.is_empty(),
            "invincible add with 0 damage must not persist"
        );
    }

    #[test]
    fn untagged_setpiece_without_local_engagement_is_dropped() {
        // A high-HP enemy with no boss-like labels (only the HP fallback), killed
        // by remote players while the local player never engaged: teleport-past
        // clutter (e.g. Possessed Small Pumpkin), not our fight.
        let mut t = CombatTracker::new();
        t.on_map_change("Realm", 1, 0);
        t.on_player_loaded(1000, 1);
        t.on_object_spawn(
            500,
            0x10E4,
            &status(
                500,
                vec![stat(StatType::MaxHP, 20_000), stat(StatType::HP, 20_000)],
            ),
            0,
        );
        // Remote player kills it; local does nothing.
        t.on_damage(500, 600, 20_000, 10);
        t.on_object_status(500, &status(500, vec![stat(StatType::HP, 0)]), 20);
        let done = t.on_tick(20);
        assert!(
            done.is_empty(),
            "untagged setpiece we never engaged must not persist"
        );
    }

    #[test]
    fn untagged_setpiece_with_local_engagement_is_kept() {
        // Same untagged high-HP enemy, but the local player engaged it (e.g.
        // Artificial Sprite God we actually fought): keep it.
        let mut t = CombatTracker::new();
        t.on_map_change("Realm", 1, 0);
        t.on_player_loaded(1000, 1);
        t.on_object_spawn(
            500,
            0x10E4,
            &status(
                500,
                vec![stat(StatType::MaxHP, 20_000), stat(StatType::HP, 20_000)],
            ),
            0,
        );
        t.on_local_hit(500, 7, 1000, 1000, 10); // local engaged
        t.on_object_status(500, &status(500, vec![stat(StatType::HP, 0)]), 20);
        let done = t.on_tick(20);
        assert_eq!(done.len(), 1, "untagged enemy we engaged is recorded");
        assert!(done[0].participants.iter().any(|p| p.is_local));
    }

    #[test]
    fn local_summon_hit_is_computed_and_credits_player() {
        // A summon-only fight: the local player's summon (entity 200) does all
        // the damage. It must be resolved from the ServerPlayerShoot damage and
        // credited to the local player, and the fight kept.
        let mut t = CombatTracker::new();
        t.on_map_change("Sprite World", 1, 0);
        t.on_player_loaded(1000, 1);
        t.on_object_spawn(
            500,
            0x10E4,
            &status(
                500,
                vec![
                    stat(StatType::MaxHP, 12_000),
                    stat(StatType::HP, 12_000),
                    stat(StatType::Defense, 20),
                ],
            ),
            0,
        );
        // A second player is present so this exercises summon-hit resolution only,
        // not the solo HP-gap reconciliation (which has its own tests).
        t.on_object_spawn(600, 0x0321, &status(600, vec![name_stat("Bob")]), 0);

        // My summon fires (summoner == local), then its bullet hits the boss.
        t.on_ally_shot(200, 1000, 5, 1, 300, 0, 0);
        t.on_local_hit(500, 5, 200, 1000, 10);
        t.on_object_status(500, &status(500, vec![stat(StatType::HP, 0)]), 20);

        let done = t.on_tick(20);
        assert_eq!(done.len(), 1, "summon-only fight is kept");
        let p = &done[0].participants;
        assert_eq!(p.len(), 1);
        assert!(p[0].is_local);
        assert_eq!(p[0].hits, 1);
        // 300 - 20 defense = 280.
        assert_eq!(p[0].damage, 280);
        assert_eq!(p[0].provenance, DamageProvenance::SelfComputed);
    }

    #[test]
    fn multi_projectile_summon_shot_registers_all_bullets() {
        let mut t = CombatTracker::new();
        t.on_map_change("Sprite World", 1, 0);
        t.on_player_loaded(1000, 1);
        t.on_object_spawn(
            500,
            0x10E4,
            &status(
                500,
                vec![
                    stat(StatType::MaxHP, 12_000),
                    stat(StatType::HP, 12_000),
                    stat(StatType::Defense, 0),
                ],
            ),
            0,
        );
        // One shot emitting 3 consecutive bullet ids (5,6,7), each worth 100.
        t.on_ally_shot(200, 1000, 5, 3, 100, 0, 0);
        t.on_local_hit(500, 7, 200, 1000, 10); // the third projectile lands
        assert_eq!(
            t.fights
                .get(&500)
                .unwrap()
                .attackers
                .get(&1000)
                .unwrap()
                .damage,
            100
        );
    }

    #[test]
    fn local_server_shot_applies_defense_and_is_computed() {
        // A local server-fired bullet (weapon proc / ability volley) resolves via
        // pending_server_shots: non-AP damage has the target's defense applied and
        // the hit counts as fully self-computed (not partial).
        let mut t = CombatTracker::new();
        t.on_map_change("Realm", 1, 0);
        t.on_player_loaded(1000, 1);
        t.on_object_spawn(
            500,
            0x10E4,
            &status(
                500,
                vec![
                    stat(StatType::MaxHP, 12_000),
                    stat(StatType::HP, 12_000),
                    stat(StatType::Defense, 20),
                ],
            ),
            0,
        );
        // A second player is present so this exercises hit resolution only, not
        // the solo HP-gap reconciliation (which has its own tests).
        t.on_object_spawn(600, 0x0321, &status(600, vec![name_stat("Bob")]), 0);
        // Registered by on_ally_shot for a local server bullet (base 300, non-AP).
        t.pending_server_shots.insert(5, (300, false));
        t.on_local_hit(500, 5, 1000, 1000, 10); // shooter == local
        t.on_object_status(500, &status(500, vec![stat(StatType::HP, 0)]), 20);
        let done = t.on_tick(20);
        assert_eq!(done.len(), 1);
        let p = &done[0].participants;
        assert_eq!(p.len(), 1);
        assert!(p[0].is_local);
        assert_eq!(p[0].damage, 280); // 300 - 20 defense
        assert_eq!(p[0].provenance, DamageProvenance::SelfComputed);
    }

    #[test]
    fn local_server_shot_armor_piercing_ignores_defense() {
        let mut t = CombatTracker::new();
        t.on_map_change("Realm", 1, 0);
        t.on_player_loaded(1000, 1);
        t.on_object_spawn(
            500,
            0x10E4,
            &status(
                500,
                vec![
                    stat(StatType::MaxHP, 12_000),
                    stat(StatType::HP, 12_000),
                    stat(StatType::Defense, 50),
                ],
            ),
            0,
        );
        t.pending_server_shots.insert(5, (300, true)); // armor-piercing
        t.on_local_hit(500, 5, 1000, 1000, 10);
        assert_eq!(
            t.fights
                .get(&500)
                .unwrap()
                .attackers
                .get(&1000)
                .unwrap()
                .damage,
            300
        );
    }

    #[test]
    fn weapon_shot_supersedes_stale_server_shot_id() {
        // A later weapon shot reusing a server-shot's bullet id claims the id, so
        // the two maps stay disjoint and the weapon roll (not the server value)
        // resolves the hit.
        let mut t = CombatTracker::new();
        t.on_map_change("Realm", 1, 0);
        t.on_player_loaded(1000, 1);
        t.pending_server_shots.insert(5, (300, false));
        t.on_local_shot(2824, 0, 5, 10, 0.0, 0.0, 0.0, 10); // reuse id 5
        assert!(
            !t.pending_server_shots.contains_key(&5),
            "weapon shot evicts server entry"
        );
    }

    #[test]
    fn unresolvable_local_server_shot_evicts_stale_entries() {
        // A local server shot whose damage can't be valued (unknown projectile
        // asset in tests) must still claim its bullet ids, evicting any stale
        // entry so a recycled id can't resolve against an unrelated older shot.
        let mut t = CombatTracker::new();
        t.on_map_change("Realm", 1, 0);
        t.on_player_loaded(1000, 1);
        t.pending_shots.insert(
            5,
            PendingShot {
                base_damage: 999,
                ..Default::default()
            },
        );
        // owner == local, container 0 -> weapon_projectile returns None in tests.
        t.on_ally_shot(1000, 0, 5, 1, 300, 0, 0);
        assert!(
            !t.pending_shots.contains_key(&5),
            "stale weapon roll evicted"
        );
        assert!(
            !t.pending_server_shots.contains_key(&5),
            "no bogus server entry left"
        );
    }

    #[test]
    fn solo_masked_echo_reconciles_escaped_local() {
        // In a solo fight the server re-broadcasts the local player's own hits as
        // damage packets with the owner masked (the sentinel id). That masked total
        // is exact; the shot simulation only approximates it. Here the boss is
        // never killed (escaped), so the HP-gap reconciliation cannot apply -- only
        // the masked echo raises the figure.
        let mut t = CombatTracker::new();
        t.on_map_change("Sprite World", 1, 0);
        t.on_player_loaded(1000, 1);
        t.on_object_spawn(
            500,
            0x10E4,
            &status(
                500,
                vec![
                    stat(StatType::MaxHP, 12_000),
                    stat(StatType::HP, 12_000),
                    stat(StatType::Defense, 0),
                ],
            ),
            0,
        );
        // Six resolved weapon hits of 100 -> self-computed 600.
        for i in 0..6i16 {
            t.pending_shots.insert(
                i,
                PendingShot {
                    base_damage: 100,
                    armor_piercing: false,
                    ..Default::default()
                },
            );
            t.on_local_hit(500, i, 1000, 1000, 10 + i as i64);
        }
        // Server echoes eight masked direct hits summing to 1200 (> computed 600).
        for i in 0..8 {
            t.on_damage(500, SENTINEL_ATTACKER_ID, 150, 30 + i);
        }
        let done = t.on_disconnect(100);
        let me = done[0]
            .participants
            .iter()
            .find(|p| p.is_local)
            .expect("local");
        assert!(!done[0].killed, "boss escaped");
        assert_eq!(
            me.damage, 1200,
            "raised to the server's masked direct total"
        );
        assert_eq!(me.provenance, DamageProvenance::SelfServerReconciled);
    }

    #[test]
    fn masked_echo_ignored_when_other_player_present() {
        // A second player was seen, so the masked (owner-hidden) damage cannot be
        // assumed to be the local player's: the computed figure stands.
        let mut t = CombatTracker::new();
        t.on_map_change("Sprite World", 1, 0);
        t.on_player_loaded(1000, 1);
        t.on_object_spawn(
            500,
            0x10E4,
            &status(
                500,
                vec![
                    stat(StatType::MaxHP, 12_000),
                    stat(StatType::HP, 12_000),
                    stat(StatType::Defense, 0),
                ],
            ),
            0,
        );
        t.on_object_spawn(600, 0x0321, &status(600, vec![name_stat("Bob")]), 0);
        for i in 0..6i16 {
            t.pending_shots.insert(
                i,
                PendingShot {
                    base_damage: 100,
                    armor_piercing: false,
                    ..Default::default()
                },
            );
            t.on_local_hit(500, i, 1000, 1000, 10 + i as i64);
        }
        for i in 0..8 {
            t.on_damage(500, SENTINEL_ATTACKER_ID, 150, 30 + i);
        }
        let done = t.on_disconnect(100);
        let me = done[0]
            .participants
            .iter()
            .find(|p| p.is_local)
            .expect("local");
        assert_eq!(
            me.damage, 600,
            "masked echo ignored with another player present"
        );
        assert_eq!(me.provenance, DamageProvenance::SelfComputed);
    }

    #[test]
    fn sparse_masked_echo_does_not_reconcile() {
        // A single masked lump packet (e.g. a kill-credit) is not a per-hit-complete
        // stream, so it must not replace the computed figure.
        let mut t = CombatTracker::new();
        t.on_map_change("Sprite World", 1, 0);
        t.on_player_loaded(1000, 1);
        t.on_object_spawn(
            500,
            0x10E4,
            &status(
                500,
                vec![
                    stat(StatType::MaxHP, 99_000),
                    stat(StatType::HP, 99_000),
                    stat(StatType::Defense, 0),
                ],
            ),
            0,
        );
        for i in 0..6i16 {
            t.pending_shots.insert(
                i,
                PendingShot {
                    base_damage: 100,
                    armor_piercing: false,
                    ..Default::default()
                },
            );
            t.on_local_hit(500, i, 1000, 1000, 10 + i as i64);
        }
        // One lone masked packet carrying a large amount.
        t.on_damage(500, SENTINEL_ATTACKER_ID, 20_000, 40);
        let done = t.on_disconnect(100);
        let me = done[0]
            .participants
            .iter()
            .find(|p| p.is_local)
            .expect("local");
        assert_eq!(me.damage, 600, "sparse single-lump masked stream rejected");
        assert_eq!(me.provenance, DamageProvenance::SelfComputed);
    }

    #[test]
    fn masked_echo_still_allows_hp_gap_recovery_on_solo_kill() {
        // A solo instanced kill from full HP: the masked echo raises the direct
        // figure, and the HP-gap reconciliation must still run afterwards (the
        // masked row must not block it) to recover the non-direct remainder up to
        // the boss's full HP.
        let mut t = CombatTracker::new();
        t.on_map_change("Davy Jones' Locker", 1, 0);
        t.on_player_loaded(1000, 1);
        t.on_object_spawn(
            500,
            0x10E4,
            &status(
                500,
                vec![
                    stat(StatType::MaxHP, 12_000),
                    stat(StatType::HP, 12_000),
                    stat(StatType::Defense, 0),
                ],
            ),
            0,
        );
        // Six resolved weapon hits of 800 -> self-computed 4800.
        for i in 0..6i16 {
            t.pending_shots.insert(
                i,
                PendingShot {
                    base_damage: 800,
                    armor_piercing: false,
                    ..Default::default()
                },
            );
            t.on_local_hit(500, i, 1000, 1000, 10 + i as i64);
        }
        // Server echoes eight masked direct hits summing to 6000 (> computed 4800).
        for i in 0..8 {
            t.on_damage(500, SENTINEL_ATTACKER_ID, 750, 30 + i);
        }
        t.on_object_status(500, &status(500, vec![stat(StatType::HP, 0)]), 50);
        let done = t.on_tick(50);
        let me = done[0]
            .participants
            .iter()
            .find(|p| p.is_local)
            .expect("local");
        assert!(done[0].killed);
        assert_eq!(
            me.damage, 12_000,
            "HP-gap recovery still tops up to full boss HP"
        );
        assert_eq!(me.provenance, DamageProvenance::SelfReconciled);
    }

    #[test]
    fn solo_kill_reconciles_hp_gap_to_local() {
        // A solo, fully-observed kill where the local player's packet-derived
        // damage (280) is far below the boss HP (12000): the unaccounted HP loss
        // (server-fired ability/summon projectiles) is credited to the local
        // player so the total matches the boss HP exactly.
        let mut t = CombatTracker::new();
        t.on_map_change("Davy Jones' Locker", 1, 0);
        t.on_player_loaded(1000, 1);
        t.on_object_spawn(
            500,
            0x10E4,
            &status(
                500,
                vec![
                    stat(StatType::MaxHP, 12_000),
                    stat(StatType::HP, 12_000),
                    stat(StatType::Defense, 20),
                ],
            ),
            0,
        );
        t.pending_server_shots.insert(5, (300, false));
        t.on_local_hit(500, 5, 1000, 1000, 10); // 280 computed
        t.on_object_status(500, &status(500, vec![stat(StatType::HP, 0)]), 20);
        let done = t.on_tick(20);
        assert_eq!(done.len(), 1);
        assert!(done[0].reached_zero);
        assert!(!done[0].joined_late);
        let p = &done[0].participants;
        assert_eq!(p.len(), 1);
        assert!(p[0].is_local);
        assert_eq!(p[0].damage, 12_000, "full boss HP credited in a solo kill");
        assert_eq!(p[0].provenance, DamageProvenance::SelfReconciled);
    }

    #[test]
    fn solo_pending_kill_reconciles_to_full_hp() {
        // Even when no local hit could be valued (SelfPending), a solo full kill
        // credits the entire boss HP to the local player.
        let mut t = CombatTracker::new();
        t.on_map_change("Davy Jones' Locker", 1, 0);
        t.on_player_loaded(1000, 1);
        t.on_object_spawn(
            500,
            0x10E4,
            &status(
                500,
                vec![stat(StatType::MaxHP, 12_000), stat(StatType::HP, 12_000)],
            ),
            0,
        );
        t.on_local_hit(500, 99, 1000, 1000, 10); // no pending shot -> 0 dmg, hit only
        t.on_object_status(500, &status(500, vec![stat(StatType::HP, 0)]), 20);
        let done = t.on_tick(20);
        assert_eq!(done.len(), 1);
        let me = done[0]
            .participants
            .iter()
            .find(|p| p.is_local)
            .expect("local");
        assert_eq!(me.damage, 12_000);
        assert_eq!(me.provenance, DamageProvenance::SelfReconciled);
    }

    #[test]
    fn solo_kill_reconciles_when_last_hp_tick_is_near_zero() {
        // The boss dies but the final HP status observed is a few HP -- no 0-HP
        // tick arrives before the death removal. A confirmed solo kill whose
        // lowest observed HP is within 1% of max must still reconcile the
        // unattributed remainder to the local player.
        let mut t = CombatTracker::new();
        t.on_map_change("Davy Jones' Locker", 1, 0);
        t.on_player_loaded(1000, 1);
        t.on_object_spawn(
            500,
            0x10E4,
            &status(
                500,
                vec![
                    stat(StatType::MaxHP, 12_000),
                    stat(StatType::HP, 12_000),
                    stat(StatType::Defense, 20),
                ],
            ),
            0,
        );
        t.pending_server_shots.insert(5, (300, false));
        t.on_local_hit(500, 5, 1000, 1000, 10); // 280 computed
                                                // Last HP tick shows 100 (< 1% of 12000), then the object is removed.
        t.on_object_status(500, &status(500, vec![stat(StatType::HP, 100)]), 20);
        let done: Vec<_> = t.on_object_removed(500, 30).into_iter().collect();
        assert_eq!(done.len(), 1);
        assert!(done[0].killed);
        let p = &done[0].participants;
        assert_eq!(p.len(), 1);
        assert_eq!(
            p[0].damage, 12_000,
            "near-zero final tick still reconciles to full HP"
        );
        assert_eq!(p[0].provenance, DamageProvenance::SelfReconciled);
    }

    #[test]
    fn solo_removal_above_epsilon_does_not_reconcile() {
        // Removed at ~8% HP: dipped-lethal still marks it killed, but that is
        // above the 1% near-zero band, so the full-HP crediting must NOT fire and
        // the local figure stays its self-computed lower bound.
        let mut t = CombatTracker::new();
        t.on_map_change("Davy Jones' Locker", 1, 0);
        t.on_player_loaded(1000, 1);
        t.on_object_spawn(
            500,
            0x10E4,
            &status(
                500,
                vec![
                    stat(StatType::MaxHP, 12_000),
                    stat(StatType::HP, 12_000),
                    stat(StatType::Defense, 20),
                ],
            ),
            0,
        );
        t.pending_server_shots.insert(5, (300, false));
        t.on_local_hit(500, 5, 1000, 1000, 10); // 280 computed
        t.on_object_status(500, &status(500, vec![stat(StatType::HP, 1_000)]), 20); // ~8%
        let done: Vec<_> = t.on_object_removed(500, 30).into_iter().collect();
        assert_eq!(done.len(), 1);
        let p = &done[0].participants;
        assert_eq!(
            p[0].damage, 280,
            "above-epsilon removal is not credited full HP"
        );
        assert_eq!(p[0].provenance, DamageProvenance::SelfComputed);
    }

    #[test]
    fn scaling_stats_do_not_double_count_boosts() {
        // The server already folds each boost component (ids 46-53) into the
        // base stat (ids 20-28), so effective scaling stats must equal the base
        // value. Summing the boost again would double-count it. This drives the
        // stats through the real status path, which also proves the boost stat
        // IDs are ignored (no longer parsed).
        use crate::assets::ScalingStat;
        let mut t = CombatTracker::new();
        t.on_player_loaded(1000, 1);
        t.on_object_spawn(1000, 0x0321, &status(1000, vec![name_stat("You")]), 0);
        t.on_object_status(
            1000,
            &status(
                1000,
                vec![
                    stat(StatType::Attack, 65),
                    stat(StatType::AttackBoost, 15),
                    stat(StatType::Defense, 40),
                    stat(StatType::DefenseBoost, 10),
                    stat(StatType::Wisdom, 75),
                    stat(StatType::WisdomBoost, 20),
                    stat(StatType::Dexterity, 65),
                    stat(StatType::DexterityBoost, 15),
                    stat(StatType::MaxMP, 400),
                    stat(StatType::MaxMPBoost, 100),
                ],
            ),
            10,
        );
        let local = t.objects.get(&1000).expect("local");
        assert_eq!(
            local.attacker_stats().attack,
            65,
            "attack uses base id20 (boost already folded)"
        );
        assert_eq!(local.effective_scaling_stat(ScalingStat::Attack), Some(65));
        assert_eq!(local.effective_scaling_stat(ScalingStat::Defense), Some(40));
        assert_eq!(local.effective_scaling_stat(ScalingStat::Wisdom), Some(75));
        assert_eq!(
            local.effective_scaling_stat(ScalingStat::Dexterity),
            Some(65)
        );
        assert_eq!(local.effective_scaling_stat(ScalingStat::MaxMp), Some(400));
    }

    #[test]
    fn group_presence_blocks_reconciliation() {
        // A second player was present (even dealing no observed damage), so the
        // run is not solo: the local figure stays its packet-derived lower bound.
        let mut t = CombatTracker::new();
        t.on_map_change("Davy Jones' Locker", 1, 0);
        t.on_player_loaded(1000, 1);
        t.on_object_spawn(
            500,
            0x10E4,
            &status(
                500,
                vec![
                    stat(StatType::MaxHP, 12_000),
                    stat(StatType::HP, 12_000),
                    stat(StatType::Defense, 20),
                ],
            ),
            0,
        );
        // Another player object appears (records into seen_player_ids).
        t.on_object_spawn(600, 0x0321, &status(600, vec![name_stat("Bob")]), 0);
        t.pending_server_shots.insert(5, (300, false));
        t.on_local_hit(500, 5, 1000, 1000, 10); // 280 computed
        t.on_object_status(500, &status(500, vec![stat(StatType::HP, 0)]), 20);
        let done = t.on_tick(20);
        assert_eq!(done.len(), 1);
        assert!(
            done[0].reached_zero,
            "boss was fully killed; only group presence blocks it"
        );
        let me = done[0]
            .participants
            .iter()
            .find(|p| p.is_local)
            .expect("local");
        assert_eq!(
            me.damage, 280,
            "no reconciliation when another player is present"
        );
        assert_eq!(me.provenance, DamageProvenance::SelfComputed);
    }

    #[test]
    fn joined_late_solo_kill_not_reconciled() {
        // The boss was already below full HP on first sight, so its start HP is
        // not the whole damage pool; reconciliation must not fire.
        let mut t = CombatTracker::new();
        t.on_map_change("Davy Jones' Locker", 1, 0);
        t.on_player_loaded(1000, 1);
        t.on_object_spawn(
            500,
            0x10E4,
            &status(
                500,
                vec![
                    stat(StatType::MaxHP, 12_000),
                    stat(StatType::HP, 6_000),
                    stat(StatType::Defense, 20),
                ],
            ),
            0,
        );
        t.pending_server_shots.insert(5, (300, false));
        t.on_local_hit(500, 5, 1000, 1000, 10); // 280 computed
        t.on_object_status(500, &status(500, vec![stat(StatType::HP, 0)]), 20);
        let done = t.on_tick(20);
        assert_eq!(done.len(), 1);
        assert!(done[0].joined_late);
        let me = done[0]
            .participants
            .iter()
            .find(|p| p.is_local)
            .expect("local");
        assert_eq!(me.damage, 280);
        assert_eq!(me.provenance, DamageProvenance::SelfComputed);
    }

    #[test]
    fn dipped_but_not_zeroed_kill_not_reconciled() {
        // A boss killed via the <=10% dipped-lethal despawn (HP never observed at
        // 0) is not eligible: we cannot prove the local player removed all its HP.
        let mut t = CombatTracker::new();
        t.on_map_change("Davy Jones' Locker", 1, 0);
        t.on_player_loaded(1000, 1);
        t.on_object_spawn(
            500,
            0x10E4,
            &status(
                500,
                vec![
                    stat(StatType::MaxHP, 12_000),
                    stat(StatType::HP, 12_000),
                    stat(StatType::Defense, 20),
                ],
            ),
            0,
        );
        t.pending_server_shots.insert(5, (300, false));
        t.on_local_hit(500, 5, 1000, 1000, 10); // 280 computed
        t.on_object_status(500, &status(500, vec![stat(StatType::HP, 600)]), 15); // 5%
        let cf = t
            .on_object_removed(500, 30)
            .expect("dipped-lethal despawn scores a kill");
        assert!(cf.killed);
        assert!(!cf.reached_zero);
        let me = cf.participants.iter().find(|p| p.is_local).expect("local");
        assert_eq!(me.damage, 280);
        assert_eq!(me.provenance, DamageProvenance::SelfComputed);
    }

    #[test]
    fn realm_solo_kill_not_reconciled() {
        // Same shape as the solo dungeon kill, but fought in the open Realm where
        // an off-screen player could damage the boss with a server-fired ability
        // yet never spawn as an object we see. The gap is NOT credited locally.
        let mut t = CombatTracker::new();
        t.on_map_change("Realm", 1, 0);
        t.on_player_loaded(1000, 1);
        t.on_object_spawn(
            500,
            0x10E4,
            &status(
                500,
                vec![
                    stat(StatType::MaxHP, 12_000),
                    stat(StatType::HP, 12_000),
                    stat(StatType::Defense, 20),
                ],
            ),
            0,
        );
        t.pending_server_shots.insert(5, (300, false));
        t.on_local_hit(500, 5, 1000, 1000, 10); // 280 computed
        t.on_object_status(500, &status(500, vec![stat(StatType::HP, 0)]), 20);
        let done = t.on_tick(20);
        assert_eq!(done.len(), 1);
        assert!(done[0].reached_zero);
        let me = done[0]
            .participants
            .iter()
            .find(|p| p.is_local)
            .expect("local");
        assert_eq!(
            me.damage, 280,
            "no reconciliation outside instanced dungeons"
        );
        assert_eq!(me.provenance, DamageProvenance::SelfComputed);
    }

    #[test]
    fn remote_summon_damage_redirects_to_owner() {
        // A remote player's summon deals damage via DamagePacket keyed to the
        // summon entity; it must be credited to the owning player, not surfaced
        // as an Unknown/pet row.
        let mut t = CombatTracker::new();
        t.on_map_change("Realm", 1, 0);
        t.on_player_loaded(1000, 1);
        t.on_object_spawn(
            500,
            0x10E4,
            &status(
                500,
                vec![stat(StatType::MaxHP, 100_000), stat(StatType::HP, 100_000)],
            ),
            0,
        );
        // Bob (600) is a remote player; his summon is entity 700.
        t.on_object_spawn(600, 0x0321, &status(600, vec![name_stat("Bob")]), 0);
        t.on_ally_shot(700, 600, 9, 1, 500, 0, 0);
        // The local player also engages so the fight is retained.
        t.on_local_hit(500, 1, 1000, 1000, 5);
        // Summon's DamagePacket arrives keyed to entity 700.
        t.on_damage(500, 700, 400, 10);
        t.on_object_status(500, &status(500, vec![stat(StatType::HP, 0)]), 20);

        let done = t.on_tick(20);
        assert_eq!(done.len(), 1);
        let bob = done[0]
            .participants
            .iter()
            .find(|p| p.name == "Bob")
            .expect("Bob credited");
        assert_eq!(bob.damage, 400);
        assert!(
            !done[0].participants.iter().any(|p| p.object_id == 700),
            "no summon-entity row"
        );
    }

    #[test]
    fn normal_player_shot_is_not_treated_as_summon() {
        // A ServerPlayerShoot for a normal player's own shot reports summoner 0
        // (or itself); it must not create an ownership remap.
        let mut t = CombatTracker::new();
        t.on_map_change("Realm", 1, 0);
        t.on_player_loaded(1000, 1);
        t.on_ally_shot(600, 0, 3, 1, 200, 0, 0);
        t.on_ally_shot(601, 601, 4, 1, 200, 0, 0);
        assert!(
            t.summon_owners.is_empty(),
            "normal shots do not register ownership"
        );
    }

    #[test]
    fn summon_ownership_cleared_on_object_removal() {
        let mut t = CombatTracker::new();
        t.on_map_change("Realm", 1, 0);
        t.on_player_loaded(1000, 1);
        t.on_ally_shot(700, 600, 9, 1, 500, 0, 0);
        assert_eq!(t.summon_owners.get(&700), Some(&600));
        t.on_object_removed(700, 10);
        assert!(
            !t.summon_owners.contains_key(&700),
            "stale ownership removed on despawn"
        );
    }

    #[test]
    fn solo_kill_damage_is_capped_to_boss_hp() {
        // A solo kill whose rolled damage overshoots the boss HP (the death-tick
        // volley overkills) is trimmed so the total equals the boss HP exactly.
        let mut t = CombatTracker::new();
        t.on_map_change("Sprite World", 1, 0);
        t.on_player_loaded(1000, 1);
        t.on_object_spawn(
            500,
            0x10E4,
            &status(
                500,
                vec![stat(StatType::MaxHP, 12_000), stat(StatType::HP, 12_000)],
            ),
            0,
        );
        // One big resolved hit worth more than the boss's HP.
        t.pending_shots.insert(
            10,
            PendingShot {
                base_damage: 20_000,
                armor_piercing: false,
                ..Default::default()
            },
        );
        t.on_local_hit(500, 10, 1000, 1000, 10);
        t.on_object_status(500, &status(500, vec![stat(StatType::HP, 0)]), 20);
        let done = t.on_tick(20);
        assert_eq!(done.len(), 1);
        assert_eq!(
            done[0].total_damage(),
            12_000,
            "overkill trimmed to boss HP"
        );
        assert_eq!(
            done[0].participants[0].provenance,
            DamageProvenance::SelfComputed
        );
    }

    #[test]
    fn overkill_cap_preserves_group_shares() {
        // 9000 + 3000 = 12000 raw, capped to a 10000 HP pool: shares preserved.
        let mut ps = vec![
            FightParticipant {
                object_id: 1,
                object_type: 0,
                skin_id: 0,
                tex1: 0,
                tex2: 0,
                name: "A".into(),
                equipment: [-1; 4],
                equipment_enchants: Default::default(),
                damage: 9000,
                hits: 10,
                is_local: false,
                provenance: DamageProvenance::Observed,
                end_status: ParticipantEndStatus::Present,
                pet: None,
                damage_taken: None,
                damage_taken_provenance: DamageTakenProvenance::Observed,
                damage_blocked: None,
                guarded_damage: None,
                guarded_hits: None,
            },
            FightParticipant {
                object_id: 2,
                object_type: 0,
                skin_id: 0,
                tex1: 0,
                tex2: 0,
                name: "B".into(),
                equipment: [-1; 4],
                equipment_enchants: Default::default(),
                damage: 3000,
                hits: 5,
                is_local: false,
                provenance: DamageProvenance::Observed,
                end_status: ParticipantEndStatus::Present,
                pet: None,
                damage_taken: None,
                damage_taken_provenance: DamageTakenProvenance::Observed,
                damage_blocked: None,
                guarded_damage: None,
                guarded_hits: None,
            },
        ];
        cap_overkill(&mut ps, 10_000);
        assert_eq!(ps[0].damage, 7500);
        assert_eq!(ps[1].damage, 2500);
        assert_eq!(ps.iter().map(|p| p.damage).sum::<i64>(), 10_000);
    }

    #[test]
    fn overkill_cap_scales_guarded_within_capped_damage() {
        // Guarded damage is a subset of total; after capping it must not exceed
        // the (now smaller) capped total.
        let mut ps = vec![
            FightParticipant {
                object_id: 1,
                object_type: 0,
                skin_id: 0,
                tex1: 0,
                tex2: 0,
                name: "A".into(),
                equipment: [-1; 4],
                equipment_enchants: Default::default(),
                damage: 9000,
                hits: 10,
                is_local: false,
                provenance: DamageProvenance::Observed,
                end_status: ParticipantEndStatus::Present,
                pet: None,
                damage_taken: None,
                damage_taken_provenance: DamageTakenProvenance::Observed,
                damage_blocked: None,
                guarded_damage: Some(9000),
                guarded_hits: Some(10),
            },
            FightParticipant {
                object_id: 2,
                object_type: 0,
                skin_id: 0,
                tex1: 0,
                tex2: 0,
                name: "B".into(),
                equipment: [-1; 4],
                equipment_enchants: Default::default(),
                damage: 3000,
                hits: 5,
                is_local: false,
                provenance: DamageProvenance::Observed,
                end_status: ParticipantEndStatus::Present,
                pet: None,
                damage_taken: None,
                damage_taken_provenance: DamageTakenProvenance::Observed,
                damage_blocked: None,
                guarded_damage: Some(0),
                guarded_hits: Some(0),
            },
        ];
        cap_overkill(&mut ps, 10_000);
        assert_eq!(ps[0].damage, 7500);
        assert_eq!(ps[0].guarded_damage, Some(7500));
        assert!(ps[0].guarded_damage.unwrap() <= ps[0].damage);
        assert_eq!(ps[1].guarded_damage, Some(0));
    }

    #[test]
    fn overkill_cap_is_noop_when_under_hp() {
        // Crowded fight where we only saw part of the HP: never inflate.
        let mut ps = vec![
            FightParticipant {
                object_id: 1,
                object_type: 0,
                skin_id: 0,
                tex1: 0,
                tex2: 0,
                name: "A".into(),
                equipment: [-1; 4],
                equipment_enchants: Default::default(),
                damage: 3000,
                hits: 10,
                is_local: false,
                provenance: DamageProvenance::Observed,
                end_status: ParticipantEndStatus::Present,
                pet: None,
                damage_taken: None,
                damage_taken_provenance: DamageTakenProvenance::Observed,
                damage_blocked: None,
                guarded_damage: None,
                guarded_hits: None,
            },
            FightParticipant {
                object_id: 2,
                object_type: 0,
                skin_id: 0,
                tex1: 0,
                tex2: 0,
                name: "B".into(),
                equipment: [-1; 4],
                equipment_enchants: Default::default(),
                damage: 2000,
                hits: 5,
                is_local: false,
                provenance: DamageProvenance::Observed,
                end_status: ParticipantEndStatus::Present,
                pet: None,
                damage_taken: None,
                damage_taken_provenance: DamageTakenProvenance::Observed,
                damage_blocked: None,
                guarded_damage: None,
                guarded_hits: None,
            },
        ];
        cap_overkill(&mut ps, 100_000);
        assert_eq!(ps[0].damage, 3000);
        assert_eq!(ps[1].damage, 2000);
    }

    #[test]
    fn overkill_cap_distributes_remainder_exactly() {
        // Odd split that does not divide evenly still sums to the cap.
        let mut ps = vec![
            FightParticipant {
                object_id: 1,
                object_type: 0,
                skin_id: 0,
                tex1: 0,
                tex2: 0,
                name: "A".into(),
                equipment: [-1; 4],
                equipment_enchants: Default::default(),
                damage: 1,
                hits: 1,
                is_local: false,
                provenance: DamageProvenance::Observed,
                end_status: ParticipantEndStatus::Present,
                pet: None,
                damage_taken: None,
                damage_taken_provenance: DamageTakenProvenance::Observed,
                damage_blocked: None,
                guarded_damage: None,
                guarded_hits: None,
            },
            FightParticipant {
                object_id: 2,
                object_type: 0,
                skin_id: 0,
                tex1: 0,
                tex2: 0,
                name: "B".into(),
                equipment: [-1; 4],
                equipment_enchants: Default::default(),
                damage: 1,
                hits: 1,
                is_local: false,
                provenance: DamageProvenance::Observed,
                end_status: ParticipantEndStatus::Present,
                pet: None,
                damage_taken: None,
                damage_taken_provenance: DamageTakenProvenance::Observed,
                damage_blocked: None,
                guarded_damage: None,
                guarded_hits: None,
            },
            FightParticipant {
                object_id: 3,
                object_type: 0,
                skin_id: 0,
                tex1: 0,
                tex2: 0,
                name: "C".into(),
                equipment: [-1; 4],
                equipment_enchants: Default::default(),
                damage: 1,
                hits: 1,
                is_local: false,
                provenance: DamageProvenance::Observed,
                end_status: ParticipantEndStatus::Present,
                pet: None,
                damage_taken: None,
                damage_taken_provenance: DamageTakenProvenance::Observed,
                damage_blocked: None,
                guarded_damage: None,
                guarded_hits: None,
            },
        ];
        cap_overkill(&mut ps, 2);
        assert_eq!(
            ps.iter().map(|p| p.damage).sum::<i64>(),
            2,
            "remainder distributed, sum exact"
        );
    }

    #[test]
    fn unresolved_attackers_are_dropped() {
        let mut ps = vec![
            FightParticipant {
                object_id: 1,
                object_type: 775,
                skin_id: 0,
                tex1: 0,
                tex2: 0,
                name: "Alice".into(),
                equipment: [-1; 4],
                equipment_enchants: Default::default(),
                damage: 500,
                hits: 3,
                is_local: false,
                provenance: DamageProvenance::Observed,
                end_status: ParticipantEndStatus::Present,
                pet: None,
                damage_taken: None,
                damage_taken_provenance: DamageTakenProvenance::Observed,
                damage_blocked: None,
                guarded_damage: None,
                guarded_hits: None,
            },
            FightParticipant {
                object_id: 2,
                object_type: 0,
                skin_id: 0,
                tex1: 0,
                tex2: 0,
                name: "Unknown".into(),
                equipment: [-1; 4],
                equipment_enchants: Default::default(),
                damage: 200,
                hits: 1,
                is_local: false,
                provenance: DamageProvenance::Unresolved,
                end_status: ParticipantEndStatus::Present,
                pet: None,
                damage_taken: None,
                damage_taken_provenance: DamageTakenProvenance::Observed,
                damage_blocked: None,
                guarded_damage: None,
                guarded_hits: None,
            },
            FightParticipant {
                object_id: 3,
                object_type: 0,
                skin_id: 0,
                tex1: 0,
                tex2: 0,
                name: "Unknown".into(),
                equipment: [-1; 4],
                equipment_enchants: Default::default(),
                damage: 300,
                hits: 2,
                is_local: false,
                provenance: DamageProvenance::Unresolved,
                end_status: ParticipantEndStatus::Present,
                pet: None,
                damage_taken: None,
                damage_taken_provenance: DamageTakenProvenance::Observed,
                damage_blocked: None,
                guarded_damage: None,
                guarded_hits: None,
            },
        ];
        drop_unresolved(&mut ps);
        assert_eq!(
            ps.len(),
            1,
            "unresolved rows removed, only the named player remains"
        );
        assert!(ps
            .iter()
            .all(|p| p.provenance != DamageProvenance::Unresolved));
        assert_eq!(ps[0].name, "Alice");
        assert_eq!(ps[0].damage, 500);
    }

    #[test]
    fn unresolved_local_damage_is_kept() {
        // A local SelfPartial/SelfPending row must never be dropped.
        let mut ps = vec![
            FightParticipant {
                object_id: 9,
                object_type: 0,
                skin_id: 0,
                tex1: 0,
                tex2: 0,
                name: "You".into(),
                equipment: [-1; 4],
                equipment_enchants: Default::default(),
                damage: 400,
                hits: 5,
                is_local: true,
                provenance: DamageProvenance::SelfPartial,
                end_status: ParticipantEndStatus::Present,
                pet: None,
                damage_taken: None,
                damage_taken_provenance: DamageTakenProvenance::Observed,
                damage_blocked: None,
                guarded_damage: None,
                guarded_hits: None,
            },
            FightParticipant {
                object_id: 2,
                object_type: 0,
                skin_id: 0,
                tex1: 0,
                tex2: 0,
                name: "Unknown".into(),
                equipment: [-1; 4],
                equipment_enchants: Default::default(),
                damage: 200,
                hits: 1,
                is_local: false,
                provenance: DamageProvenance::Unresolved,
                end_status: ParticipantEndStatus::Present,
                pet: None,
                damage_taken: None,
                damage_taken_provenance: DamageTakenProvenance::Observed,
                damage_blocked: None,
                guarded_damage: None,
                guarded_hits: None,
            },
        ];
        drop_unresolved(&mut ps);
        assert_eq!(ps.len(), 1);
        assert!(ps[0].is_local);
    }

    #[test]
    fn sanitize_strips_hex_name_suffix() {
        assert_eq!(sanitize_player_name("Knekten,fe2,a19f"), "Knekten");
        assert_eq!(sanitize_player_name("Lovens"), "Lovens");
    }

    #[test]
    fn normalize_realm_token_to_readable_label() {
        assert_eq!(normalize_dungeon("{s.rotmg}"), "Realm");
        assert_eq!(normalize_dungeon(""), "Realm");
        assert_eq!(normalize_dungeon("Mad Lab"), "Mad Lab");
    }

    #[test]
    fn groupable_dungeon_excludes_hubs_and_open_world() {
        // Real dungeons group.
        assert!(is_groupable_dungeon("Mad Lab"));
        assert!(is_groupable_dungeon("Oryx's Sanctuary"));
        assert!(is_groupable_dungeon("The Nest"));
        // Hubs, social spaces and the open world stay standalone.
        assert!(!is_groupable_dungeon("Realm"));
        assert!(!is_groupable_dungeon("Nexus"));
        assert!(!is_groupable_dungeon("Vault"));
        assert!(!is_groupable_dungeon("Guild Hall"));
        assert!(!is_groupable_dungeon("Pet Yard"));
        assert!(!is_groupable_dungeon("Marketplace"));
        assert!(!is_groupable_dungeon(""));
        assert!(!is_groupable_dungeon("{s.rotmg}"));
    }

    #[test]
    fn remote_damage_taken_is_observed_and_exact() {
        let mut t = CombatTracker::new();
        t.on_map_change("Mad Lab", 42, 0);
        t.on_player_loaded(1000, 42);
        t.on_object_spawn(
            500,
            0x10E4,
            &status(
                500,
                vec![stat(StatType::MaxHP, 50_000), stat(StatType::HP, 50_000)],
            ),
            0,
        );
        t.on_object_spawn(600, 0x0321, &status(600, vec![name_stat("Alice")]), 0);

        // Local engages the boss (keeps the fight), Alice deals and takes damage.
        t.pending_shots.insert(
            10,
            PendingShot {
                base_damage: 1000,
                armor_piercing: false,
                ..Default::default()
            },
        );
        t.on_local_hit(500, 10, 1000, 1000, 10);
        t.on_damage(500, 600, 5000, 20);
        // Boss hits Alice for an exact, post-defense amount.
        t.on_damage(600, 500, 300, 30);
        assert_eq!(
            t.fights
                .get(&500)
                .unwrap()
                .attackers
                .get(&600)
                .unwrap()
                .damage_taken,
            300,
        );

        t.on_object_status(500, &status(500, vec![stat(StatType::HP, 0)]), 40);
        let done = t.on_tick(40);
        let alice = done[0]
            .participants
            .iter()
            .find(|p| p.name == "Alice")
            .expect("Alice");
        assert_eq!(alice.damage_taken, Some(300));
        assert_eq!(
            alice.damage_taken_provenance,
            DamageTakenProvenance::Observed
        );
    }

    #[test]
    fn damage_to_local_player_is_not_credited_as_taken() {
        // The local player's own damage taken never arrives in a DamagePacket, so
        // a packet targeting the local id must be ignored (no double count).
        let mut t = CombatTracker::new();
        t.on_map_change("Mad Lab", 42, 0);
        t.on_player_loaded(1000, 42);
        t.on_object_spawn(
            500,
            0x10E4,
            &status(
                500,
                vec![stat(StatType::MaxHP, 50_000), stat(StatType::HP, 50_000)],
            ),
            0,
        );
        t.pending_shots.insert(
            10,
            PendingShot {
                base_damage: 1000,
                armor_piercing: false,
                ..Default::default()
            },
        );
        t.on_local_hit(500, 10, 1000, 1000, 10);
        // Boss "damages" the local player via a DamagePacket: must be dropped.
        t.on_damage(1000, 500, 777, 20);
        assert!(
            !t.fights.contains_key(&1000),
            "no fight spawned for the local player"
        );

        t.on_object_status(500, &status(500, vec![stat(StatType::HP, 0)]), 30);
        let done = t.on_tick(30);
        let local = done[0]
            .participants
            .iter()
            .find(|p| p.is_local)
            .expect("local");
        assert_eq!(local.damage_taken, None);
    }

    #[test]
    fn ambiguous_minion_taken_is_dropped() {
        // Two live boss fights: an unknown minion attacker can't be attributed to
        // one of them, so the hit is dropped rather than fanned out.
        let mut t = CombatTracker::new();
        t.on_map_change("Mad Lab", 42, 0);
        t.on_player_loaded(1000, 42);
        t.on_object_spawn(
            500,
            0x10E4,
            &status(
                500,
                vec![stat(StatType::MaxHP, 50_000), stat(StatType::HP, 50_000)],
            ),
            0,
        );
        t.on_object_spawn(
            501,
            0x10E4,
            &status(
                501,
                vec![stat(StatType::MaxHP, 50_000), stat(StatType::HP, 50_000)],
            ),
            0,
        );
        t.on_object_spawn(600, 0x0321, &status(600, vec![name_stat("Alice")]), 0);
        t.on_damage(500, 600, 1000, 10);
        t.on_damage(501, 600, 1000, 10);
        // Unknown attacker 999 hits Alice while both boss fights are live.
        t.on_damage(600, 999, 400, 20);
        assert_eq!(
            t.fights
                .get(&500)
                .unwrap()
                .attackers
                .get(&600)
                .unwrap()
                .damage_taken,
            0,
            "ambiguous minion damage must not be credited",
        );
    }

    #[test]
    fn local_player_hit_estimates_damage_taken() {
        let mut t = CombatTracker::new();
        t.on_map_change("Mad Lab", 42, 0);
        t.on_player_loaded(1000, 42);
        // Local player object with a known defense stat.
        t.on_object_spawn(
            1000,
            0x0321,
            &status(1000, vec![name_stat("You"), stat(StatType::Defense, 50)]),
            0,
        );
        t.on_object_spawn(
            500,
            0x10E4,
            &status(
                500,
                vec![stat(StatType::MaxHP, 50_000), stat(StatType::HP, 50_000)],
            ),
            0,
        );
        t.pending_shots.insert(
            10,
            PendingShot {
                base_damage: 1000,
                armor_piercing: false,
                ..Default::default()
            },
        );
        t.on_local_hit(500, 10, 1000, 1000, 10);
        // Enemy projectile (pre-defense 100) then the local player is hit by it.
        t.on_enemy_shot(500, 7, 0, 100, 255);
        t.on_player_hit(7, 500, 20);

        t.on_object_status(500, &status(500, vec![stat(StatType::HP, 0)]), 30);
        let done = t.on_tick(30);
        let local = done[0]
            .participants
            .iter()
            .find(|p| p.is_local)
            .expect("local");
        // 100 pre-defense minus 50 defense = 50 (above the 10% floor).
        assert_eq!(local.damage_taken, Some(50));
        assert_eq!(
            local.damage_taken_provenance,
            DamageTakenProvenance::Estimated
        );
        // With no extra mitigation, blocked is just the defense contribution.
        assert_eq!(local.damage_blocked, Some(50));
    }

    #[test]
    fn armor_piercing_enemy_shot_ignores_defense() {
        let mut t = CombatTracker::new();
        t.on_map_change("Mad Lab", 42, 0);
        t.on_player_loaded(1000, 42);
        // Local player with 50 defense.
        t.on_object_spawn(
            1000,
            0x0321,
            &status(1000, vec![name_stat("You"), stat(StatType::Defense, 50)]),
            0,
        );
        t.on_object_spawn(
            500,
            0x10E4,
            &status(
                500,
                vec![stat(StatType::MaxHP, 50_000), stat(StatType::HP, 50_000)],
            ),
            0,
        );
        t.pending_shots.insert(
            10,
            PendingShot {
                base_damage: 1000,
                armor_piercing: false,
                ..Default::default()
            },
        );
        t.on_local_hit(500, 10, 1000, 1000, 10);
        // Armor-piercing enemy projectile (pre-defense 100): resolved at shoot
        // time. Injected directly since the test asset manager has no data.
        t.pending_enemy_shots.insert((500, 7), (100, true));
        t.on_player_hit(7, 500, 20);

        t.on_object_status(500, &status(500, vec![stat(StatType::HP, 0)]), 30);
        let done = t.on_tick(30);
        let local = done[0]
            .participants
            .iter()
            .find(|p| p.is_local)
            .expect("local");
        // Armor-piercing: defense is bypassed, full 100 taken, nothing blocked.
        assert_eq!(local.damage_taken, Some(100));
        assert_eq!(local.damage_blocked, Some(0));
    }

    #[test]
    fn local_damage_blocked_includes_enchant_and_armor_of_nil() {
        let mut t = CombatTracker::new();
        t.on_map_change("Mad Lab", 42, 0);
        t.on_player_loaded(1000, 42);
        t.on_object_spawn(
            1000,
            0x0321,
            &status(1000, vec![name_stat("You"), stat(StatType::Defense, 50)]),
            0,
        );
        // Equip Armor of Nil (15% proc) with a Damage Resistance IV (5%) enchant.
        {
            let local = t.objects.get_mut(&1000).expect("local object");
            local.equipment[ARMOR_SLOT] = ARMOR_OF_NIL_ITEM_ID;
            local.equipment_enchants[ARMOR_SLOT] = vec![1559];
        }
        t.on_object_spawn(
            500,
            0x10E4,
            &status(
                500,
                vec![stat(StatType::MaxHP, 50_000), stat(StatType::HP, 50_000)],
            ),
            0,
        );
        t.pending_shots.insert(
            10,
            PendingShot {
                base_damage: 1000,
                armor_piercing: false,
                ..Default::default()
            },
        );
        t.on_local_hit(500, 10, 1000, 1000, 10);
        t.on_enemy_shot(500, 7, 0, 100, 255);
        t.on_player_hit(7, 500, 20);

        t.on_object_status(500, &status(500, vec![stat(StatType::HP, 0)]), 30);
        let done = t.on_tick(30);
        let local = done[0]
            .participants
            .iter()
            .find(|p| p.is_local)
            .expect("local");
        // 100 - 50 def = 50; enchant 5% then Armor of Nil 15%:
        // 50 * 0.95 * 0.85 = 40.375 -> 40 taken, 60 blocked.
        assert_eq!(local.damage_taken, Some(40));
        assert_eq!(local.damage_blocked, Some(60));
    }

    #[test]
    fn local_player_hit_without_valued_shot_is_partial() {
        let mut t = CombatTracker::new();
        t.on_map_change("Mad Lab", 42, 0);
        t.on_player_loaded(1000, 42);
        t.on_object_spawn(
            1000,
            0x0321,
            &status(1000, vec![name_stat("You"), stat(StatType::Defense, 50)]),
            0,
        );
        t.on_object_spawn(
            500,
            0x10E4,
            &status(
                500,
                vec![stat(StatType::MaxHP, 50_000), stat(StatType::HP, 50_000)],
            ),
            0,
        );
        t.pending_shots.insert(
            10,
            PendingShot {
                base_damage: 1000,
                armor_piercing: false,
                ..Default::default()
            },
        );
        t.on_local_hit(500, 10, 1000, 1000, 10);
        // Player is hit by a bullet that was never registered (no EnemyShoot).
        t.on_player_hit(7, 500, 20);

        t.on_object_status(500, &status(500, vec![stat(StatType::HP, 0)]), 30);
        let done = t.on_tick(30);
        let local = done[0]
            .participants
            .iter()
            .find(|p| p.is_local)
            .expect("local");
        assert_eq!(local.damage_taken, Some(0), "lower bound of zero, not None");
        assert_eq!(
            local.damage_taken_provenance,
            DamageTakenProvenance::Partial
        );
    }

    // A Moonlight Village dancer floors at 1 HP (invulnerable) and is never
    // removed, so without a completion signal it finalizes as Escaped.
    #[test]
    fn mv_dancer_without_marker_stays_escaped() {
        let mut t = CombatTracker::new();
        t.on_map_change("Moonlight Village", 42, 0);
        t.on_player_loaded(1000, 42);
        // Sage Genji (20450): high HP so the HP fallback treats it as a boss.
        t.on_object_spawn(
            500,
            20450,
            &status(
                500,
                vec![stat(StatType::MaxHP, 360_000), stat(StatType::HP, 360_000)],
            ),
            100,
        );
        t.pending_shots.insert(
            10,
            PendingShot {
                base_damage: 1000,
                armor_piercing: false,
                ..Default::default()
            },
        );
        t.on_local_hit(500, 10, 1000, 1000, 200);
        // Floors at 1 HP -- never 0, so the kill heuristic never fires.
        t.on_object_status(500, &status(500, vec![stat(StatType::HP, 1)]), 300);
        let done = t.on_map_change("Nexus", 0, 500);
        assert_eq!(done.len(), 1);
        assert!(!done[0].killed, "dancer at 1 HP with no marker is Escaped");
    }

    // The MV Dungeon Complete marker (20658) scores all three dancers as
    // completed even though they go invulnerable rather than dying.
    #[test]
    fn mv_dungeon_complete_marker_completes_dancer() {
        let mut t = CombatTracker::new();
        t.on_map_change("Moonlight Village", 42, 0);
        t.on_player_loaded(1000, 42);
        t.on_object_spawn(
            500,
            20450,
            &status(
                500,
                vec![stat(StatType::MaxHP, 360_000), stat(StatType::HP, 360_000)],
            ),
            100,
        );
        t.on_local_hit(500, 7, 1000, 1000, 200);
        t.on_object_status(500, &status(500, vec![stat(StatType::HP, 1)]), 300);
        // Completion marker spawns (invisible, 100 HP).
        t.on_object_spawn(
            15598,
            20658,
            &status(
                15598,
                vec![stat(StatType::MaxHP, 100), stat(StatType::HP, 100)],
            ),
            400,
        );
        // The live fight is flagged immediately; a tick finalizes it as a kill.
        let done = t.on_tick(450);
        assert_eq!(done.len(), 1);
        assert!(
            done[0].killed,
            "dancer completed via MV Dungeon Complete marker"
        );
    }

    // The MV Umi Complete marker (49339) completes Umi, who heals back to full
    // and never dies.
    #[test]
    fn mv_umi_complete_marker_completes_umi() {
        let mut t = CombatTracker::new();
        t.on_map_change("Moonlight Village", 42, 0);
        t.on_player_loaded(1000, 42);
        // Kitsune Umi (20493) seen at full HP, only ever dips to 50%.
        t.on_object_spawn(
            254,
            20493,
            &status(
                254,
                vec![stat(StatType::MaxHP, 360_000), stat(StatType::HP, 360_000)],
            ),
            100,
        );
        t.on_local_hit(254, 7, 1000, 1000, 200);
        t.on_object_status(254, &status(254, vec![stat(StatType::HP, 180_000)]), 300);
        t.on_object_spawn(
            15599,
            49339,
            &status(
                15599,
                vec![stat(StatType::MaxHP, 100), stat(StatType::HP, 100)],
            ),
            400,
        );
        let done = t.on_map_change("Nexus", 0, 500);
        assert_eq!(done.len(), 1);
        assert!(done[0].killed, "Umi completed via MV Umi Complete marker");
    }

    // A marker only completes its own mapped boss types: the Umi marker must not
    // complete an in-progress dancer.
    #[test]
    fn mv_marker_only_completes_mapped_types() {
        let mut t = CombatTracker::new();
        t.on_map_change("Moonlight Village", 42, 0);
        t.on_player_loaded(1000, 42);
        t.on_object_spawn(
            500,
            20450,
            &status(
                500,
                vec![stat(StatType::MaxHP, 360_000), stat(StatType::HP, 360_000)],
            ),
            100,
        );
        t.pending_shots.insert(
            10,
            PendingShot {
                base_damage: 1000,
                armor_piercing: false,
                ..Default::default()
            },
        );
        t.on_local_hit(500, 10, 1000, 1000, 200);
        t.on_object_status(500, &status(500, vec![stat(StatType::HP, 1)]), 300);
        // Only the Umi marker fires -- the dancer is not one of its targets.
        t.on_object_spawn(
            15599,
            49339,
            &status(
                15599,
                vec![stat(StatType::MaxHP, 100), stat(StatType::HP, 100)],
            ),
            400,
        );
        let done = t.on_map_change("Nexus", 0, 500);
        assert_eq!(done.len(), 1);
        assert!(
            !done[0].killed,
            "dancer not completed by an unrelated marker"
        );
    }

    // The completed-boss flag is scoped to the run: a marker in one map does not
    // complete a same-type boss in a later map instance.
    #[test]
    fn mv_completion_flag_clears_on_map_change() {
        let mut t = CombatTracker::new();
        t.on_map_change("Moonlight Village", 42, 0);
        t.on_player_loaded(1000, 42);
        t.on_object_spawn(
            15598,
            20658,
            &status(
                15598,
                vec![stat(StatType::MaxHP, 100), stat(StatType::HP, 100)],
            ),
            100,
        );
        // New run: a dancer here must not inherit the previous run's completion.
        t.on_map_change("Moonlight Village", 43, 1000);
        t.on_object_spawn(
            500,
            20450,
            &status(
                500,
                vec![stat(StatType::MaxHP, 360_000), stat(StatType::HP, 360_000)],
            ),
            1100,
        );
        t.pending_shots.insert(
            10,
            PendingShot {
                base_damage: 1000,
                armor_piercing: false,
                ..Default::default()
            },
        );
        t.on_local_hit(500, 10, 1000, 1000, 1200);
        t.on_object_status(500, &status(500, vec![stat(StatType::HP, 1)]), 1300);
        let done = t.on_map_change("Nexus", 0, 1500);
        assert_eq!(done.len(), 1);
        assert!(
            !done[0].killed,
            "completion flag does not carry across runs"
        );
    }

    // After a marker finalizes a dancer, lingering fire on the still-present
    // invulnerable boss object must not spawn a duplicate Completed fight.
    #[test]
    fn mv_marker_does_not_duplicate_on_lingering_fire() {
        let mut t = CombatTracker::new();
        t.on_map_change("Moonlight Village", 42, 0);
        t.on_player_loaded(1000, 42);
        t.on_object_spawn(
            500,
            20450,
            &status(
                500,
                vec![stat(StatType::MaxHP, 360_000), stat(StatType::HP, 360_000)],
            ),
            100,
        );
        t.on_local_hit(500, 7, 1000, 1000, 200);
        t.on_object_status(500, &status(500, vec![stat(StatType::HP, 1)]), 300);
        // Marker fires, then a tick finalizes the flagged fight as a kill.
        t.on_object_spawn(
            15598,
            20658,
            &status(
                15598,
                vec![stat(StatType::MaxHP, 100), stat(StatType::HP, 100)],
            ),
            400,
        );
        let first = t.on_tick(450);
        assert_eq!(first.len(), 1, "one completed dancer from the tick");
        assert!(first[0].killed);
        // The invulnerable dancer object is still present; a late hit must not
        // create a second fight for the same completed boss type.
        t.pending_shots.insert(
            11,
            PendingShot {
                base_damage: 1000,
                armor_piercing: false,
                ..Default::default()
            },
        );
        t.on_local_hit(500, 11, 1000, 1000, 500);
        let rest = t.on_map_change("Nexus", 0, 600);
        assert!(
            rest.is_empty(),
            "no duplicate dancer from lingering fire, got {:?}",
            rest.len()
        );
    }

    // A summoner run re-detects a dancer under many object ids before the
    // completion marker fires. Every re-detection folds into one fight so the DB
    // records a single row, not one per id.
    #[test]
    fn mv_dancer_redetected_under_many_ids_yields_one_record() {
        let mut t = CombatTracker::new();
        t.on_map_change("Moonlight Village", 42, 0);
        t.on_player_loaded(1000, 42);
        t.on_object_spawn(600, 0x0400, &status(600, vec![name_stat("Bob")]), 0);
        let hp = || vec![stat(StatType::MaxHP, 360_000), stat(StatType::HP, 360_000)];
        for id in [500, 501, 502] {
            let base = (id - 500) as i64 * 1000;
            t.on_object_spawn(id, 20450, &status(id, hp()), base);
            t.on_damage(id, 600, 10_000, base + 10);
            t.on_local_hit(id, 7, 1000, 1000, base + 12);
            // Every detection but the last despawns (suspended, not finalized).
            if id != 502 {
                assert!(
                    t.on_object_removed(id, base + 20).is_none(),
                    "re-detection is suspended"
                );
                assert!(t.on_tick(base + 30).is_empty(), "nothing surfaces mid-run");
            }
        }
        // MV Dungeon Complete marker fires while the last detection is live.
        t.on_object_spawn(
            15598,
            20658,
            &status(
                15598,
                vec![stat(StatType::MaxHP, 100), stat(StatType::HP, 100)],
            ),
            3000,
        );
        let done = t.on_tick(3050);
        assert_eq!(
            done.len(),
            1,
            "one record for all re-detections, got {}",
            done.len()
        );
        assert!(done[0].killed);
        assert_eq!(done[0].boss_object_type, 20450);
        let bob = done[0]
            .participants
            .iter()
            .find(|p| p.name == "Bob")
            .expect("Bob");
        assert_eq!(
            bob.damage, 30_000,
            "damage from every re-detection is combined"
        );
    }

    // Two object ids of the same dancer briefly live at once when the marker
    // fires still consolidate into a single record.
    #[test]
    fn mv_concurrent_same_type_ids_consolidate_to_one_record() {
        let mut t = CombatTracker::new();
        t.on_map_change("Moonlight Village", 42, 0);
        t.on_player_loaded(1000, 42);
        t.on_object_spawn(600, 0x0400, &status(600, vec![name_stat("Bob")]), 0);
        let hp = vec![stat(StatType::MaxHP, 360_000), stat(StatType::HP, 360_000)];
        t.on_object_spawn(500, 20450, &status(500, hp.clone()), 0);
        t.on_damage(500, 600, 10_000, 10);
        t.on_local_hit(500, 7, 1000, 1000, 12);
        t.on_object_spawn(501, 20450, &status(501, hp.clone()), 20);
        t.on_damage(501, 600, 5_000, 30);
        t.on_local_hit(501, 8, 1000, 1000, 32);
        assert_eq!(
            t.fights
                .values()
                .filter(|f| f.boss_object_type == 20450)
                .count(),
            2,
            "two live same-type fights coexist",
        );
        t.on_object_spawn(
            15598,
            20658,
            &status(
                15598,
                vec![stat(StatType::MaxHP, 100), stat(StatType::HP, 100)],
            ),
            40,
        );
        let done = t.on_tick(50);
        assert_eq!(
            done.len(),
            1,
            "two concurrent ids consolidated to one record"
        );
        assert!(done[0].killed);
        let bob = done[0]
            .participants
            .iter()
            .find(|p| p.name == "Bob")
            .expect("Bob");
        assert_eq!(bob.damage, 15_000);
    }

    // Re-detections with no completion (player leaves MV mid-fight) collapse to a
    // single escaped record rather than one per id.
    #[test]
    fn mv_redetections_without_marker_yield_one_escaped_record() {
        let mut t = CombatTracker::new();
        t.on_map_change("Moonlight Village", 42, 0);
        t.on_player_loaded(1000, 42);
        t.on_object_spawn(600, 0x0400, &status(600, vec![name_stat("Bob")]), 0);
        let hp = || vec![stat(StatType::MaxHP, 360_000), stat(StatType::HP, 360_000)];
        for id in [500, 501, 502] {
            let base = (id - 500) as i64 * 1000;
            t.on_object_spawn(id, 20450, &status(id, hp()), base);
            t.on_damage(id, 600, 10_000, base + 10);
            t.on_local_hit(id, 7, 1000, 1000, base + 12);
            assert!(
                t.on_object_removed(id, base + 20).is_none(),
                "re-detection is suspended"
            );
        }
        // Leave MV without completing: the single suspended fight surfaces once.
        let done = t.on_map_change("Nexus", 0, 5000);
        let mv: Vec<_> = done
            .iter()
            .filter(|f| f.boss_object_type == 20450)
            .collect();
        assert_eq!(
            mv.len(),
            1,
            "one escaped record for all re-detections, got {}",
            mv.len()
        );
        assert!(!mv[0].killed);
        let bob = mv[0]
            .participants
            .iter()
            .find(|p| p.name == "Bob")
            .expect("Bob");
        assert_eq!(bob.damage, 30_000);
    }
}
