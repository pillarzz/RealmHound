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
use crate::assets::SummonTrigger;
use crate::loot::class_ids;
use crate::protocol::data::{ObjectStatusData, StatType};
use crate::stats::derived::pet_ability_deals_damage;

use super::damage::{self, AttackerStats, Rng, DEFAULT_EXALT_BONUS};
use super::types::{
    AbilityActivation, BoostWindow, CompletedFight, DamageProvenance, DamageTakenProvenance,
    FightParticipant, ParticipantEndStatus, PetInfo, SummonContribution, WeaponContribution,
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
/// Primary condition bits surfaced in the local player's boost log. Only bits
/// whose meaning is known here are named; any other bit change is ignored rather
/// than shown as a raw mask.
const BOOST_CONDITIONS: &[(i32, &str)] = &[
    (0x0000_0004, "Weak"),         // damage x0.5
    (0x0004_0000, "Damaging"),     // damage x1.25
    (COND_INVISIBLE, "Invisible"), // cloaked (rogue sneak)
    (COND_INVULNERABLE, "Invulnerable"),
];
/// How long after a local `UseItem` a stat change is attributed to that item
/// ("consumable (Potion of Attack)") rather than left unexplained. The server's
/// stat update trails the use by a packet or two, so this is a small window.
const BOOST_SOURCE_HINT_MS: i64 = 1_500;
/// How long after a local shot an on-shoot proc's stat change may arrive and still
/// be attributed to that proc. The server applies the buff in the same tick as the
/// hit it triggered, so this only covers packet ordering.
const BOOST_PROC_HINT_MS: i64 = 800;

/// The stat a boost-log label describes, for verifying an inferred source against
/// the game data. `None` for condition labels (`Damaging`, `Weak`, ...), which are
/// matched against an item's declared conditions instead. `ATT boost` (stat 48) is
/// the companion of `Attack` (stat 20) and is declared as `ATT` too.
///
/// Public so the UI can apply the same check to rows recorded before the tracker
/// verified its sources.
pub fn stat_kind_for_boost_label(label: &str) -> Option<crate::assets::StatKind> {
    use crate::assets::StatKind;
    match label {
        "Attack" | "ATT boost" => Some(StatKind::Attack),
        "Defense" => Some(StatKind::Defense),
        "Speed" => Some(StatKind::Speed),
        "Dexterity" => Some(StatKind::Dexterity),
        "Vitality" => Some(StatKind::Vitality),
        "Wisdom" => Some(StatKind::Wisdom),
        "Max HP" => Some(StatKind::MaxHp),
        "Max MP" => Some(StatKind::MaxMp),
        _ => None,
    }
}
/// Attribution window for one ability activation, in milliseconds: local damage
/// packets inside it are credited to that activation. A heuristic -- the wire
/// carries no effect id -- capped so overlapping activations cannot double-count.
const ABILITY_ATTRIBUTION_WINDOW_MS: i64 = 2_000;
/// How long after an activation an ability-spawned summon can still be landing
/// hits. Measured on live captures: a Flowering Kimono's spirits put 123 of 125
/// bulletless hits within 5s of one of the player's own activations, in a fight
/// whose activations were up to 15s apart, while a pet out at the same time
/// attacks on its own clock (an Electric pet landed all of its hits 4.5-6s after
/// the last activation, and in a fight with no activations at all). Sage's
/// Wakibiki's tooltip states the matching lifetime -- "Lifetime: 6 seconds",
/// "Cooldown: 1.5 seconds" -- so the window is the creature's own life rather
/// than an arbitrary grace period.
const SUMMON_SPAWN_ATTRIBUTION_MS: i64 = 6_000;
/// How far a summoned creature can be from a target and still be the one that hit
/// it. Measured in game: a spirit reaches an enemy 5 tiles away and cannot reach
/// one at 6. The item tooltip's "3 squares" is the radius of its *slow*, not of
/// its zap. This is a disambiguator, not a hit model -- the packet already says
/// how much damage landed -- so its only job is to pick between creatures of the
/// player's that are out at the same moment (a Kimono and a Wakibiki spirit
/// "synergize", so more than one can be alive at once). Being wrong here moves a
/// hit between two of the player's own rows, never in or out of the total.
const SUMMON_ATTACK_MAX_DIST: f32 = 5.0;
/// How long a tracked summoned creature stays a candidate with no further sight
/// of it. Bounds the map when a removal packet never arrives.
const LOCAL_SUMMON_MAX_AGE_MS: i64 = 60_000;
/// Whether an object type is a mechanic that damages the boss *for* the player: a
/// pushed barrel, a cannonball.
///
/// Curated, because nothing in the object data says "this damages the boss when
/// pushed". The two Parasite Chambers barrels are pinned by id so the behaviour does
/// not depend on the asset catalog being loaded, and names are matched as well so a
/// reskin or a new barrel-like object works without a code change. Anything else
/// leaving the world at the same tick as a hit -- a trap, a wall piece, a mob -- is
/// not a source, however plausible the coincidence. Background: issue #74.
fn is_boss_damaging_mechanic(object_type: i32) -> bool {
    matches!(object_type, 0x0786 | 0x0796)
        || get_asset_manager()
            .object_name(object_type)
            .map(|name| {
                let lower = name.to_lowercase();
                ["barrel", "cannon", "gunpowder"]
                    .iter()
                    .any(|key| lower.contains(key))
            })
            .unwrap_or(false)
}

/// The object type a mechanic's row is named and drawn by.
///
/// The boss room has two barrels and the game gives them different object types
/// (`0x0786` and `0x0796`); only one of them has a texture in the sprite atlas, so
/// reporting them as they are showed the same name twice, once without a sprite. They
/// are one mechanic, so they are one row -- count and total damage together, drawn by
/// the type that renders.
fn mechanic_row_type(object_type: i32) -> i32 {
    match object_type {
        0x0796 => 0x0786,
        other => other,
    }
}

/// How close in time an object's removal has to be to a packet for it to be the
/// packet's source. Measured at the same tick on a barrel-only capture; the window is
/// wider than one tick because a live tick can straddle the two packets.
const MECHANIC_REMOVAL_WINDOW_MS: i64 = 800;
/// How close an object has to leave the world, relative to the hit's target, to be
/// that hit's source. A pushed barrel explodes at the boss; anything vanishing on
/// the far side of the instance is not this packet's source.
const MECHANIC_REMOVAL_MAX_DIST: f32 = 10.0;
/// Source label for a transition the character cache reported directly rather
/// than a numeric stat the wire moved: a Blood Ritual or Crucible switch. Only
/// these are seeded into a fight's windows when they were already active before
/// it, since a stat's first reading (VIT arriving at 75) is not a boost.
const CHARACTER_STATE_SOURCE: &str = "character state";
/// Cap on the rolling local-event logs the performance sections are rebuilt
/// from. A long session keeps the most recent entries; the alternative (growing
/// without bound) is worse than losing detail from very old fights.
const MAX_LOCAL_EVENT_LOG: usize = 4_096;
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
/// slightly before or after the removal packet in the same/adjacent tick). Also
/// used by the combat-history database when applying a stored death broadcast to
/// a fight that was already written.
pub(crate) const GRAVE_MATCH_WINDOW_MS: i64 = 15_000;
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

/// A server death broadcast observed for another player, handed to the caller to
/// persist so a death announced after its fight was written can still be applied.
#[derive(Debug, Clone)]
pub struct DeathBroadcast {
    /// The victim's name as broadcast.
    pub player: String,
    /// Who killed them (best-effort context from the broadcast).
    pub killer: String,
    /// When the broadcast was received (unix ms).
    pub time: i64,
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

/// How many Spectral Penitentiary mini-bosses must have taunted (cleared every
/// objective) for Soulwarden Murcian -- and therefore the dungeon -- to be in
/// hard mode. His fight is hard mode only when both of the run's mini-bosses
/// were: one is not enough, so a single hard-mode mini-boss leaves the run
/// regular.
const SPECTRAL_HM_BOSSES_REQUIRED: usize = 2;

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
    /// AttackBoost stat (id 48). Not part of the damage math (stat 20 already
    /// carries the effective value); tracked so the local player's boost log can
    /// show an ATT buff landing even when the base stat is unchanged.
    attack_boost: i32,
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
    /// Vitality (id 26); the server already folds VitalityBoost (id 51) into
    /// it. Scales VIT-scaled gear (e.g. the Cloak of the Darkened Sun's Lethal
    /// Strike).
    vitality: i32,
    /// Speed (id 22); the server already folds SpeedBoost (id 50) into it.
    /// Scales SPD-scaled gear (e.g. Sharktooth Sigil's Lethal Strike).
    speed: i32,
    /// Whether a base Wisdom stat has been observed (avoids a 0-WIS default
    /// under-applying the Lethal Strike bonus before the first stat arrives).
    wisdom_seen: bool,
    /// Whether an Attack stat has been observed for this object (so the local
    /// player's damage multiplier is not computed from a zero default).
    attack_seen: bool,
    /// Whether an HP stat has been observed for this object. An object whose
    /// pool the server never reports keeps the zero default, which must not be
    /// read as a destroyed (or damaged) one.
    hp_seen: bool,
    /// Whether a Defense stat has been observed (so the local player's damage
    /// taken is not estimated from a zero default).
    defense_seen: bool,
    /// Whether a Vitality stat has been observed (so a VIT-scaled Lethal Strike
    /// bonus is not computed from a zero default).
    vitality_seen: bool,
    /// Whether a Speed stat has been observed (see `vitality_seen`).
    speed_seen: bool,
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
            attack_boost: 0,
            defense: 0,
            condition: 0,
            condition_new: 0,
            animation: 0,
            exalt_bonus: DEFAULT_EXALT_BONUS,
            wisdom: 0,
            max_mp: 0,
            dexterity: 0,
            vitality: 0,
            speed: 0,
            wisdom_seen: false,
            attack_seen: false,
            hp_seen: false,
            defense_seen: false,
            vitality_seen: false,
            speed_seen: false,
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
            ScalingStat::Vitality => self.vitality_seen.then_some(self.vitality),
            ScalingStat::Speed => self.speed_seen.then_some(self.speed),
            ScalingStat::MaxMp => Some(self.max_mp),
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
            // Mastery and the seasonal challenge multipliers are character data,
            // not object stats, so they are not known here; `on_local_shot` fills
            // them from the tracker (see `set_shot_damage_bonus`).
            ..Default::default()
        }
    }
}

/// A local shot awaiting its `EnemyHit` confirmation, keyed by bullet id. The
/// base (pre-defense) damage is rolled at shoot time; defense is applied per hit
/// against the target's live stats (a piercing bullet may hit several targets).
#[derive(Debug, Clone, Default)]
struct PendingShot {
    base_damage: i32,
    armor_piercing: bool,
    /// The loadout this shot is attributed to: the player's gear when it was
    /// fired, with the weapon slot taken from the `PlayerShoot` packet (the
    /// authoritative firing weapon, so a lagging stat update cannot mislabel it).
    ///
    /// Damage is bucketed by the gear that fired it, not by whatever is equipped
    /// when the `EnemyHit` is processed. A bullet can be in flight across a swap,
    /// and a loot-bonus weapon equipped for the last moments of a fight would
    /// otherwise collect every shot still in the air (the loot swapout then reads
    /// as the main damage weapon).
    shot_gear: ([i32; 4], [Vec<u16>; 4]),
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
    /// Summon/pet-sourced subset of `damage`/`hits`. The owner's row keeps the
    /// total (the game attributes pet damage to the player), but the split lets
    /// the summary show the pet's own line and keeps pet hits out of the owner's
    /// per-hit average and per-loadout buckets.
    summon_damage: i64,
    summon_hits: u64,
    /// Summon hits whose shoot packet was missed, so no base damage was known.
    unresolved_summon_hits: u64,
    /// Per-summon contribution, keyed by the summon/pet object id.
    summons: HashMap<i32, SummonAccum>,
    /// Damage the server itself attributed to this participant inside a local
    /// damage packet: an ability/DoT/proc figure the client did not compute.
    /// Only ever non-zero for the local player (every remote row arrives solely
    /// this way). Deliberately kept out of `damage_by_equipment` so the weapon
    /// rows stay purely self-computed instead of claiming a packet's damage.
    packet_damage: i64,
    packet_hits: u64,
}

/// Contribution of one summon or pet: a shooting entity owned by a participant.
#[derive(Debug, Clone, Default)]
struct SummonAccum {
    damage: i64,
    hits: u64,
    /// Pet sprite / skin id (`StatType::PetType`); 0 when not a pet.
    pet_type: i32,
    /// Whether the entity carried a pet stat, i.e. it is a pet and not a
    /// summon spawned by an item or ability.
    is_pet: bool,
    /// The entity's own object type, which names and draws the row when it is a
    /// summon: a summoned creature's type is its identity, where a pet's is its
    /// skin under `pet_type`. 0 until the entity is observed.
    object_type: i32,
    /// The item that created it and the enchants that item had, resolved from the
    /// local player's loadout and activation log (see
    /// [`CombatTracker::summon_source_item`]). 0 until resolved.
    source_item: i32,
    source_enchants: Vec<u16>,
}

/// One boost-relevant stat/condition transition on the local player, in absolute
/// time. Per-fight windows are rebuilt from consecutive transitions of the same
/// label at finalize.
#[derive(Debug, Clone)]
struct LocalStatChange {
    time_ms: i64,
    /// Stat or condition name; also the key a window is closed on.
    label: &'static str,
    from: i32,
    to: i32,
    /// Best-effort source hint (an item used just before, "expired", or unknown).
    source: String,
    /// Object id of the item `source` names, when one was resolved.
    source_item: Option<i32>,
}

/// One activation of the local player's ability-slot item (absolute time).
#[derive(Debug, Clone)]
struct LocalAbilityUse {
    time_ms: i64,
    item_id: i32,
    enchants: Vec<u16>,
    /// How long after this activation the item's own declared server-applied
    /// effect can still be producing the bulletless `DamagePacket`s (see
    /// [`AbilityEffects::server_applied_damage_window_ms`]), resolved once at
    /// activation time. `None` when the item declares no such effect, i.e. when a
    /// bulletless packet could not have come from it.
    server_effect_window_ms: Option<i64>,
}

/// One item in the local player's loadout that declares a summon of the creature
/// being resolved.
#[derive(Debug, Clone, Copy)]
struct SummonCandidate {
    /// Equipment slot it sits in, which is where its enchants are read from.
    slot: usize,
    item: i32,
    trigger: SummonTrigger,
    /// The spawning element's own `lifeTime`, 0 when it declares none.
    lifetime_ms: i64,
}

/// Decide which of the local player's items created a summoned creature (see
/// [`CombatTracker::summon_source_item`]), returning the item and the enchants it
/// had.
fn pick_summon_source(
    candidates: &[SummonCandidate],
    uses: &VecDeque<LocalAbilityUse>,
    equipment_enchants: &[Vec<u16>],
    at_ms: i64,
) -> Option<(i32, Vec<u16>)> {
    let mut best: Option<(i64, i32, Vec<u16>)> = None;
    for &SummonCandidate {
        slot,
        item,
        trigger,
        lifetime_ms,
    } in candidates
    {
        let lifetime = if lifetime_ms > 0 {
            lifetime_ms
        } else {
            SUMMON_SPAWN_ATTRIBUTION_MS
        };
        let fired = uses
            .iter()
            .filter(|u| match trigger {
                SummonTrigger::Use => u.item_id == item,
                // An ability-activated summon is created by the *declaring* item
                // (the Kimono), not by whichever ability happened to be pressed,
                // so any activation dates it.
                SummonTrigger::OnAbilityUse => true,
                _ => false,
            })
            .filter(|u| u.time_ms <= at_ms && at_ms - u.time_ms <= lifetime)
            .max_by_key(|u| u.time_ms);
        let Some(use_) = fired else { continue };
        // The enchants belong to the *declaring* item, which is the item whose
        // slot is being read -- never to the ability that happened to be pressed
        // (a Kimono's spirits come from an activation of something else entirely,
        // and taking that item's enchants would split one Kimono across two rows).
        let enchants = equipment_enchants.get(slot).cloned().unwrap_or_default();
        if best.as_ref().map_or(true, |(at, _, _)| use_.time_ms > *at) {
            best = Some((use_.time_ms, item, enchants));
        }
    }
    if let Some((_, item, enchants)) = best {
        return Some((item, enchants));
    }
    // Nothing in the log triggered it (a shoot-triggered summon, say): the one
    // equipped item that declares the creature is the answer, and only when it is
    // alone.
    match candidates {
        [only] => Some((
            only.item,
            equipment_enchants
                .get(only.slot)
                .cloned()
                .unwrap_or_default(),
        )),
        _ => None,
    }
}

/// Whether an activation's own declared server-applied effect could still be
/// producing a bulletless `DamagePacket` at `time_ms`. The single definition of
/// "this packet is already accounted for", shared by the ability-credit pass and
/// the pet/summon attribution so an event can never be credited twice.
fn activation_claims_packet(use_: &LocalAbilityUse, time_ms: i64) -> bool {
    use_.server_effect_window_ms
        .is_some_and(|window| time_ms >= use_.time_ms && time_ms <= use_.time_ms + window)
}

/// What produced one bulletless `DamagePacket` the server credited to the local
/// player.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PacketDamageSource {
    /// A creature spawned by an activation of the player's loadout.
    Summon {
        /// Its own object type, which names and draws the row.
        creature: i32,
        /// Its object id when it was seen in the world, so the row keys on the
        /// real instance; `None` when only the timing window places it.
        object_id: Option<i32>,
    },
    /// The damage-capable pet that was out when the packet landed.
    Pet { pet_id: i32 },
}

/// Decide which pet or summon a packet-borne local damage event belongs to, or
/// `None` when nothing explains it (the event then stays an unattributed
/// `packet_damage` figure).
///
/// `spawns` are the summons the loadout can spawn, per
/// [`CombatTracker::activation_spawned_summons_of_local_loadout`]; `uses` are the
/// player's activations in the fight. Evidence wins in this order:
///
/// 1. a summoned creature seen in the world, alive and in reach of the target --
///    the closest thing to a direct trace, since it is observed rather than
///    inferred from timing;
/// 2. a damage-capable pet that was out and in range;
/// 3. an activation-created summon, when the creature itself was never seen (a
///    missed spawn packet) and no pet could have been the shooter. A pet out at
///    the same time takes precedence here, because a spammed ability makes the
///    timing window nearly always true and it can no longer tell the two apart.
fn packet_damage_source(
    uses: &[&LocalAbilityUse],
    spawns: &[(i32, Option<i32>)],
    seen_creatures: &HashSet<i32>,
    event: &LocalDamageEvent,
) -> Option<PacketDamageSource> {
    // An activation whose item declares a server-applied effect owns its packet,
    // so it is never re-attributed to a pet or a summon sitting nearby.
    if uses
        .iter()
        .any(|u| activation_claims_packet(u, event.time_ms))
    {
        return None;
    }
    if let Some((object_id, creature)) = event.summon {
        return Some(PacketDamageSource::Summon {
            creature,
            object_id: Some(object_id),
        });
    }
    if let Some(pet_id) = event.pet_id {
        return Some(PacketDamageSource::Pet { pet_id });
    }
    // Creatures the loadout can spawn, each with the last activation that could
    // have created it (a `Use` summon only by its own item's, anything else by
    // any activation).
    let mut candidates: Vec<(i64, i32)> = spawns
        .iter()
        .filter_map(|&(creature, only_item)| {
            let at = uses
                .iter()
                .filter(|u| only_item.map_or(true, |item| u.item_id == item))
                .map(|u| u.time_ms)
                .filter(|&t| event.time_ms >= t && event.time_ms - t <= SUMMON_SPAWN_ATTRIBUTION_MS)
                .max();
            at.map(|t| (t, creature))
        })
        .collect();
    // A creature seen in the world around this fight is the better guess: an item
    // that spawns on any activation keeps this window open almost always, so the
    // timing alone would hand the packet to whichever creature happened to be
    // activated last, including one that was never actually out.
    if candidates.len() > 1 {
        let observed: Vec<(i64, i32)> = candidates
            .iter()
            .filter(|(_, creature)| seen_creatures.contains(creature))
            .copied()
            .collect();
        if !observed.is_empty() {
            candidates = observed;
        }
    }
    // Most recent activation wins; ties by lowest creature id so the choice cannot
    // depend on iteration order.
    candidates
        .into_iter()
        .max_by_key(|&(at, creature)| (at, std::cmp::Reverse(creature)))
        .map(|(_, creature)| PacketDamageSource::Summon {
            creature,
            object_id: None,
        })
}

/// A creature the local player's own item summons, seen in the world.
#[derive(Debug, Clone, Copy)]
struct LocalSummonEntity {
    /// The creature's own object type, which names and draws its row.
    object_type: i32,
    /// When it first appeared, which bounds how long it can have been hitting
    /// anything: a spirit item's own tooltip gives a 6 second lifetime.
    first_seen_ms: i64,
    last_seen_ms: i64,
    x: f32,
    y: f32,
}

/// One packet-borne damage event the server credited to the local player. Only
/// such damage is on the wire; the rest of the local player's damage is
/// reconstructed from their own shot packets and has no server-side event.
/// An object that left the world, with where it was last seen and when.
///
/// A hit the engine cannot explain any other way is credited to the one that
/// vanished as it landed: several dungeons damage the boss *for* the player through
/// an object -- Parasite Chambers' barrels, the Galleon's cannonballs -- and the
/// server credits the player who last touched it. Nothing else on the wire names
/// the source, but the removal is tick-aligned with the packet (measured on a solo
/// Nightmare Colony killed with barrels alone: all 105 hits coincided with an
/// `Explosive Barrel` removal).
#[derive(Debug, Clone, Copy)]
struct LocalObjectRemoval {
    time_ms: i64,
    object_type: i32,
    x: f32,
    y: f32,
}

#[derive(Debug, Clone, Copy)]
struct LocalDamageEvent {
    time_ms: i64,
    damage: i64,
    /// The fight this packet belongs to. The packet names only its target, so the
    /// target has to be resolved to the same stable destination the fight uses:
    /// a boss object id, a wandering/invulnerable boss type, or an aggregated aux
    /// category. Without it, every overlapping fight would consume the same
    /// events inside its time window.
    destination: FightDestination,
    /// A pet of the local player that was out, in range and able to deal damage
    /// when this landed (`None` otherwise). The packet names the *owner*, never
    /// the shooter, so this observation is what separates a pet's server-credited
    /// damage from any other bulletless damage the server credits to the player.
    pet_id: Option<i32>,
    /// A summoned creature of the local player's that was alive and within reach
    /// of this packet's target when it landed, as `(object id, object type)`.
    /// Seen in the world rather than inferred, so it outranks the timing window.
    summon: Option<(i32, i32)>,
    /// The object type of something that left the world near the target as this
    /// landed: the source of a mechanic that damages the boss for the player (see
    /// [`LocalObjectRemoval`]). Last resort, only when nothing the player fired
    /// explains the packet.
    removal: Option<i32>,
}
/// One landed hit from a volley the server fired for the local player. The
/// `ServerPlayerShoot` packet names the item that fired the bullet, so the hit
/// (and its packet-borne damage) belongs to that item's own activation.
#[derive(Debug, Clone, Copy)]
struct LocalVolleyHit {
    time_ms: i64,
    item_id: i32,
    damage: i64,
    /// The fight the hit landed in, for the same reason as
    /// [`LocalDamageEvent::destination`].
    destination: FightDestination,
}

/// The stable identity of one fight's damage destination. A normal boss is its
/// object id; a wandering / invulnerable-finish / Moonlight Village boss is keyed
/// by type because it re-enters under new ids; an aggregated aux fight is keyed
/// by its category.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
enum FightDestination {
    Boss(i32),
    BossType(i32),
    Aux(&'static str),
}

impl FightDestination {
    fn for_boss(object_type: i32, object_id: i32) -> Self {
        let am = get_asset_manager();
        if am.is_wandering_boss(object_type)
            || crate::assets::is_invuln_finish_boss(object_type)
            || crate::assets::is_mv_boss(object_type)
        {
            Self::BossType(object_type)
        } else {
            Self::Boss(object_id)
        }
    }
}

/// The local player's boost-relevant stats, snapshotted around one stat update
/// so the change can be turned into a boost window.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct BoostSnapshot {
    attack: i32,
    attack_boost: i32,
    defense: i32,
    dexterity: i32,
    wisdom: i32,
    max_hp: i32,
    max_mp: i32,
    vitality: i32,
    speed: i32,
    condition: i32,
}

impl BoostSnapshot {
    fn of(obj: &TrackedObject) -> Self {
        Self {
            attack: obj.attack,
            attack_boost: obj.attack_boost,
            defense: obj.defense,
            dexterity: obj.dexterity,
            wisdom: obj.wisdom,
            max_hp: obj.max_hp,
            max_mp: obj.max_mp,
            vitality: obj.vitality,
            speed: obj.speed,
            condition: obj.condition,
        }
    }

    /// Level-stat changes between two snapshots, as (label, from, to). Vitality
    /// and Speed are included so a VIT-stacking build's stat swings are visible
    /// in the fight's boost timeline, not just its damage; Max HP is included
    /// because the life-buffing treats raise it.
    fn level_changes(&self, after: &Self) -> Vec<(&'static str, i32, i32)> {
        [
            ("Attack", self.attack, after.attack),
            ("ATT boost", self.attack_boost, after.attack_boost),
            ("Defense", self.defense, after.defense),
            ("Dexterity", self.dexterity, after.dexterity),
            ("Wisdom", self.wisdom, after.wisdom),
            ("Max HP", self.max_hp, after.max_hp),
            ("Max MP", self.max_mp, after.max_mp),
            ("Vitality", self.vitality, after.vitality),
            ("Speed", self.speed, after.speed),
        ]
        .into_iter()
        .filter(|(_, before, now)| before != now)
        .collect()
    }

    /// Condition-bit changes between two snapshots, as (label, from, to) with
    /// 0/1 values: a bit turning on opens a window, turning off closes it.
    ///
    /// Only the primary mask (stat 29) is read here. The secondary mask (stat 69)
    /// is a *different* layout -- its bits mean other things (`NEWCON_*` in
    /// `damage`) -- so testing this mask's bits against the combined value made a
    /// new-condition bit read as a classic one, e.g. a phantom `Invisible` on a
    /// character that can never become invisible.
    fn condition_changes(&self, after: &Self) -> Vec<(&'static str, i32, i32)> {
        let before_bits = self.condition;
        let after_bits = after.condition;
        BOOST_CONDITIONS
            .iter()
            .filter(|(bit, _)| (before_bits & bit) != (after_bits & bit))
            .map(|(bit, name)| {
                if after_bits & bit != 0 {
                    (*name, 0, 1)
                } else {
                    (*name, 1, 0)
                }
            })
            .collect()
    }
}

/// The live character's state as the *character cache* reports it -- the same
/// values the ATTRIBUTES panel reads.
///
/// Blood Ritual and the Crucible never reach the engine as numeric stats (the
/// ritual is a string stat, the crucible a marker), and a pet's identity lives
/// only in the account cache. The processor pushes them here, and every fight
/// finalised from then on carries them in its own record, so the fight card
/// shows the state the fight was fought under rather than whatever is true when
/// the card is opened.
#[derive(Debug, Clone, Default)]
struct LocalSnapshot {
    /// The Blood Ritual tier the character has on (its id -- the definition itself
    /// is the season's, not the character's), or `None` when none is active. The
    /// *id* is what a fight has to keep: a character's recorded tier is cleared the
    /// moment the ritual is switched off, so a card that resolved it later showed a
    /// fight fought under a ritual as having none.
    blood_ritual: Option<String>,
    crucible: bool,
    /// The pet's sprite: its skin, or its type where it has no skin -- the same
    /// choice ATTRIBUTES makes. 0 when the character has no pet out.
    pet_skin: i32,
    /// The pet's own type id, which is what the wire reports for a pet
    /// (`StatType::PetType`): the identity a claim has to match. 0 when unknown.
    pet_type: i32,
}

/// State of an in-progress boss fight, keyed by the boss object id.
#[derive(Debug, Clone)]
struct FightState {
    boss_object_id: i32,
    boss_object_type: i32,
    /// The stable damage destination this fight owns (see [`FightDestination`]).
    destination: FightDestination,
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
    /// Moonlight Village spirits released during this fight. Each spirit is one
    /// `MV Total Counter` object spawned at the end of a dance/Umi phase.
    spirits: i32,
    /// Total time the boss was both vulnerable and taking damage, in ms. Drives
    /// the damage-per-minute figure; 0 means the value is unknown (records
    /// written before this existed), so the UI falls back to the whole duration.
    boss_uptime_ms: i64,
    /// Cursor for uptime accrual. Only gaps up to [`MAX_UPTIME_GAP_MS`] count, so
    /// a boss left standing between phases does not accrue idle time.
    uptime_cursor_ms: i64,
}

/// Longest gap between boss events that still counts as fight uptime. Anything
/// longer is treated as idle (boss parked, players away) and contributes only
/// this much, which keeps DPM from being diluted by standing around.
const MAX_UPTIME_GAP_MS: i64 = 5_000;

impl FightState {
    /// Accrue boss uptime up to `now_ms`, counting the elapsed slice only while
    /// the boss is vulnerable. Call on every boss damage event and on every boss
    /// condition change (with the state that applied *before* the change).
    fn account_uptime(&mut self, now_ms: i64, vulnerable: bool) {
        let delta = (now_ms - self.uptime_cursor_ms).clamp(0, MAX_UPTIME_GAP_MS);
        if vulnerable {
            self.boss_uptime_ms += delta;
        }
        self.uptime_cursor_ms = now_ms;
    }
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
    /// Mastery and seasonal challenge damage multipliers applied to the local
    /// player's rolled weapon shots. Character data, not packets: the app pushes
    /// them with [`CombatTracker::set_shot_damage_bonus`], resolved through
    /// [`crate::stats::derived::shot_damage_bonus`] -- the same helper the
    /// theoretical DPS page uses, so Combat History and that page cannot
    /// disagree about what the player's own shot is worth. Neutral (1.0) until
    /// set, which reproduces the pre-existing numbers exactly.
    shot_damage_bonus: crate::stats::derived::ShotDamageBonus,
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
    /// by bullet id. The value is `(damage, armor_piercing, item_id)`, where the
    /// item is the one the server named as the shot's source (a spell's volley
    /// names the spell). These arrive via `ServerPlayerShoot` with owner == local
    /// and are confirmed by an outgoing `EnemyHit`; their damage is only in that
    /// packet. Kept disjoint from `pending_shots` (newest registration owns an
    /// id). Cleared on map change.
    pending_server_shots: HashMap<i16, (i32, bool, i32)>,
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
    /// Death broadcasts observed since the last drain, for the caller to persist.
    /// Never cleared on map change: the caller persists them and the persisted row
    /// is what lets a death announced after its fight was written be applied.
    pending_death_broadcasts: Vec<DeathBroadcast>,
    /// Set while a map change / disconnect force-finalizes active fights, so the
    /// local player -- always still in `objects` from their own view -- is scored
    /// as having left (nexused) any boss fight that was not completed.
    local_departing: bool,
    /// Per-instance pet -> owner proximity votes and pet identity.
    /// Keyed by pet object id; reset on map change / disconnect.
    pet_tracks: HashMap<i32, PetTrack>,
    /// Summoned creatures of the local player's that are currently in the world,
    /// keyed by object id (see [`CombatTracker::track_local_summon`]).
    local_summons: HashMap<i32, LocalSummonEntity>,
    /// Rolling log of the local player's stat/condition transitions in absolute
    /// time. Sliced into per-fight boost windows at finalize, so one log serves
    /// every fight in the session (and multi-phase merges stay correct without
    /// any merge bookkeeping).
    local_stat_changes: VecDeque<LocalStatChange>,
    /// The live character's Blood Ritual, Crucible and pet, pushed by the
    /// processor from the character cache (see [`LocalSnapshot`]).
    local_snapshot: LocalSnapshot,
    /// Objects that left the world recently, so a packet nothing else explains can
    /// be credited to the one that vanished as it landed (see
    /// [`LocalObjectRemoval`]).
    object_removals: VecDeque<LocalObjectRemoval>,
    /// Rolling log of the local player's ability activations (absolute time),
    /// likewise sliced per fight at finalize.
    local_ability_uses: VecDeque<LocalAbilityUse>,
    /// Rolling log of packet-borne damage the server credited to the local
    /// player. This is the only local damage that reaches the wire, so it is what
    /// ability activations can be credited from.
    local_damage_events: VecDeque<LocalDamageEvent>,
    /// Rolling log of the local player's server-fired volley hits: the damage the
    /// `ServerPlayerShoot` packet named, on the bullet's `EnemyHit`, together with
    /// the item that packet said fired it. Unlike `local_damage_events` (which
    /// carries no source), these are source-exact and are credited to that same
    /// item's own activation.
    local_volley_hits: VecDeque<LocalVolleyHit>,
    /// Last local `UseItem` (item id, time), used to propose a source for a stat
    /// change that follows it. `None` until the first use this session.
    last_local_use: Option<(i32, i64)>,
    /// Timestamp (epoch millis) of the local player's most recent weapon shot, used
    /// to tell an on-shoot gear proc from an ally's buff arriving at the same time.
    last_shot_ms: i64,
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
    /// Object ids of Moonlight Village spirits (`MV Total Counter`) already
    /// counted this map instance. The server re-adds an object repeatedly as it
    /// re-enters view, so deduping by id keeps the tally at one per spirit.
    /// Cleared on map change / disconnect.
    mv_spirit_ids: HashSet<i32>,
    /// Spirits released while no Moonlight Village boss fight was live (the
    /// release can race the fight's finalize); carried onto the next such fight
    /// started this run. Reset on map change / disconnect.
    pending_mv_spirits: i32,
    /// Whether a Moonlight Village dancer/Umi fight has been engaged this run.
    /// Until then the run is in the boss-less tutorial lantern phases, whose
    /// releases must not be scored as spirits.
    mv_boss_engaged: bool,
    /// Whether this run was switched to Moonlight Village's Leisurely Mode (a
    /// Tofu Delicacy was consumed). Latched onto the run's fights so the card
    /// can label the run -- the mode shortens the phases and cuts the loot, so
    /// the run is not comparable with a normal clear. Reset on map change.
    mv_leisurely: bool,
    /// Whether the local player took Moonlight Village's Challenge Mode this run
    /// (they whacked the Challenge Gate, which pet-stasises them permanently).
    /// Latched onto the run's fights so the card labels the run "Petless". The
    /// gate itself is a curated non-boss and never becomes a fight, so this latch
    /// is the only trace it leaves. Reset on map change.
    mv_petless: bool,
    /// The Spectral Penitentiary mini-bosses that shouted their hard-mode taunt
    /// this instance (the taunt only plays when every objective of their wing was
    /// cleared). Kept per instance: the dungeon is hard mode only when both of
    /// the run's mini-bosses were, which then applies to Soulwarden Murcian too.
    /// Reset on map change.
    spectral_hm_bosses: HashSet<&'static str>,
    /// The Shatters hard mode, stage: the Stone Idol only becomes damageable
    /// once the Void Phantasm was absorbed next to it, so real HP loss on it
    /// means the run is in hard mode and the Bridge Sentinel will be Valen the
    /// Unbreakable. Reset on map change.
    shatters_hm_bridge: bool,
    /// The Shatters hard mode, late stage: The Source is the secret object in
    /// the Alchemy Lab wing, which is only ever spawned in hard mode; destroying
    /// it makes the Twilight Archmage Nox the Wild Shadow and carries through to
    /// the Forgotten King, who becomes King Azamoth. Reset on map change.
    shatters_hm_late: bool,
    /// Whether the local player has reached The Bridge Sentinel at all this
    /// instance. Hard mode can be "wasted" by pulling the first boss before the
    /// Stone Idol is destroyed, so evidence read once the bridge fight is under
    /// way describes the bosses after it, never the Sentinel itself. Reset on map
    /// change.
    shatters_bridge_engaged: bool,
    /// The Shatters bosses this instance heard speak under their hard-mode name
    /// (see [`Self::on_boss_text`]). Per boss rather than per stage, because that
    /// is exactly what the reveal proves: the boss that spoke was renamed, and
    /// each of the run's bosses is labelled from its own evidence. Reset on map
    /// change.
    shatters_hm_named: HashSet<i32>,
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
            shot_damage_bonus: crate::stats::derived::ShotDamageBonus::none(),
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
            pending_death_broadcasts: Vec::new(),
            local_departing: false,
            pet_tracks: HashMap::new(),
            local_summons: HashMap::new(),
            local_stat_changes: VecDeque::new(),
            local_snapshot: LocalSnapshot::default(),
            object_removals: VecDeque::new(),
            local_ability_uses: VecDeque::new(),
            local_damage_events: VecDeque::new(),
            local_volley_hits: VecDeque::new(),
            last_local_use: None,
            last_shot_ms: 0,
            completed_boss_types: HashSet::new(),
            completed_recorded_types: HashSet::new(),
            shield_dr_until: 0,
            shield_dr_pct: 0,
            nil_active_until: 0,
            nil_ready_at: 0,
            local_low_hp_active: false,
            pending_close_calls: 0,
            run_pending_fight_close_calls: 0,
            mv_spirit_ids: HashSet::new(),
            pending_mv_spirits: 0,
            mv_boss_engaged: false,
            mv_leisurely: false,
            mv_petless: false,
            spectral_hm_bosses: HashSet::new(),
            shatters_hm_bridge: false,
            shatters_hm_late: false,
            shatters_bridge_engaged: false,
            shatters_hm_named: HashSet::new(),
        }
    }

    /// Set the local player's shot damage multipliers (exaltation mastery times
    /// the seasonal challenge multipliers). The app derives them from the
    /// character's own data with [`crate::stats::derived::shot_damage_bonus`],
    /// the same helper the theoretical DPS page uses. Until they are set the
    /// engine runs neutral, i.e. exactly as it did before they existed.
    pub fn set_shot_damage_bonus(&mut self, bonus: crate::stats::derived::ShotDamageBonus) {
        self.shot_damage_bonus = bonus;
    }

    /// The shot damage multipliers currently applied to self-computed shots.
    /// Neutral (1.0/1.0) until the app pushes the character's values.
    pub fn shot_damage_bonus(&self) -> crate::stats::derived::ShotDamageBonus {
        self.shot_damage_bonus
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
        self.object_removals.clear();
        self.pending_summon_shots.clear();
        self.pending_server_shots.clear();
        self.seen_player_ids.clear();
        self.pending_enemy_shots.clear();
        self.recent_graves.clear();
        self.departures.clear();
        self.pending_local_grave = None;
        self.remote_deaths.clear();
        self.pet_tracks.clear();
        self.local_summons.clear();
        self.completed_boss_types.clear();
        self.completed_recorded_types.clear();
        self.reset_local_mitigation_windows();
        // A new instance starts a fresh close-call carry buffer; the low-HP edge
        // state resets so the first dip in the new map always counts.
        self.local_low_hp_active = false;
        self.run_pending_fight_close_calls = 0;
        self.mv_spirit_ids.clear();
        self.pending_mv_spirits = 0;
        self.mv_boss_engaged = false;
        self.mv_leisurely = false;
        self.mv_petless = false;
        self.spectral_hm_bosses.clear();
        self.shatters_hm_bridge = false;
        self.shatters_hm_late = false;
        self.shatters_bridge_engaged = false;
        self.shatters_hm_named.clear();
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
        self.object_removals.clear();
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
        self.local_summons.clear();
        self.completed_boss_types.clear();
        self.completed_recorded_types.clear();
        self.reset_local_mitigation_windows();
        self.local_low_hp_active = false;
        self.run_pending_fight_close_calls = 0;
        self.mv_spirit_ids.clear();
        self.pending_mv_spirits = 0;
        self.mv_boss_engaged = false;
        self.mv_leisurely = false;
        self.mv_petless = false;
        self.spectral_hm_bosses.clear();
        self.shatters_hm_bridge = false;
        self.shatters_hm_late = false;
        self.shatters_bridge_engaged = false;
        self.shatters_hm_named.clear();
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
        // An object id can be reused after a despawn. Drop any summon ownership or
        // per-instance summon identity recorded for the old object so the new one
        // cannot inherit it (a player-class spawn must never read as a summon).
        self.local_summons.remove(&object_id);
        self.summon_owners.remove(&object_id);
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
        self.track_local_summon(object_id, object_type, status.pos.x, status.pos.y, time_ms);
        let spawn_max_hp = self.objects.get(&object_id).map(|o| o.max_hp).unwrap_or(0);
        self.note_aux_instance(object_id, object_type, spawn_max_hp);
        // The Bridge Sentinel coming into view means the local player reached the
        // bridge arena, so any hard-mode evidence read from here on describes the
        // *next* bosses, not this one (see `note_shatters_hm_unlock_object`).
        if object_type == crate::assets::SHATTERS_BRIDGE_SENTINEL_TYPE {
            self.shatters_bridge_engaged = true;
        }
        // Tempest, the phoenix the hard-mode Twilight Archmage summons. It is the
        // only evidence for the late stage that does not depend on us having
        // caught The Source before it was destroyed.
        if object_type == crate::assets::SHATTERS_TEMPEST_TYPE {
            self.confirm_shatters_hm_late("Tempest in view");
        }
        // The Idol's pool may already be damaged when we first see it (a run we
        // arrived late to), and The Source needs no HP reading at all: the object
        // itself proves hard mode.
        self.note_shatters_hm_unlock_object(object_id, 0);
        // A Moonlight Village spirit released at the end of a dance/Umi phase.
        // The server re-adds the same object as it re-enters view, so count each
        // id once per run and credit it to the fight that released it. The
        // dungeon's opening tutorial (three boss-less lantern mini-phases, see
        // `MV Tutorial Lantern*` objects) precedes the dance and scores no
        // spirits, so nothing counts until a dancer/Umi fight has been engaged.
        if crate::assets::is_mv_spirit(object_type) && self.mv_spirit_ids.insert(object_id) {
            if self.mv_boss_engaged {
                self.note_mv_spirit();
            }
        }
    }

    /// Credit one collected Moonlight Village spirit to the fight that released
    /// it: the most recently active dancer/Umi fight. When no such fight is live
    /// (a phase-end release can race the fight's finalize), hold the spirit until
    /// the next one starts this run; if the run ends first, [`Self::finalize_all`]
    /// credits it to the run's last MV fight so the run total stays exact.
    fn note_mv_spirit(&mut self) {
        // Ties in `last_activity` (e.g. the burst landing between two phases) are
        // broken by start time then object id so the pick is deterministic --
        // `fights` is a HashMap, so its iteration order is not.
        let live = self
            .fights
            .iter()
            .filter(|(_, f)| crate::assets::is_mv_boss(f.boss_object_type))
            .max_by_key(|(&id, f)| (f.last_activity, f.started_at, id))
            .map(|(&id, _)| id);
        if let Some(id) = live {
            if let Some(fight) = self.fights.get_mut(&id) {
                fight.spirits += 1;
            }
            return;
        }
        // The dancer left view between phases, so its fight is suspended. Credit
        // the suspension rather than dropping the burst: it is the same run.
        let dormant = self
            .dormant_fights
            .iter()
            .filter(|(_, f)| crate::assets::is_mv_boss(f.boss_object_type))
            .max_by_key(|(&ty, f)| (f.last_activity, f.started_at, ty))
            .map(|(&ty, _)| ty);
        match dormant {
            Some(ty) => {
                if let Some(fight) = self.dormant_fights.get_mut(&ty) {
                    fight.spirits += 1;
                }
            }
            None => self.pending_mv_spirits += 1,
        }
    }

    /// Follow a spawned creature that one of the local player's items can summon.
    ///
    /// The wire never says who owns a summoned creature, so this keeps only the
    /// ones that (a) have a type an equipped item of the player's declares a
    /// summon for and (b) appeared next to the player. Its lifetime is then the
    /// strongest evidence of whose hits are whose: a packet names the owner and
    /// never the shooter, so a creature alive at that moment -- rather than a
    /// 6s window after an activation -- is the closest thing to a direct trace.
    fn track_local_summon(
        &mut self,
        object_id: i32,
        object_type: i32,
        x: f32,
        y: f32,
        time_ms: i64,
    ) {
        if object_id == self.local_object_id {
            return;
        }
        let declared = self
            .activation_spawned_summons_of_local_loadout()
            .into_iter()
            .any(|(creature, _)| creature == object_type);
        if !declared {
            return;
        }
        let Some(local) = self.objects.get(&self.local_object_id) else {
            return;
        };
        let (dx, dy) = (x - local.last_x, y - local.last_y);
        if dx * dx + dy * dy > PET_ASSOC_MAX_DIST * PET_ASSOC_MAX_DIST {
            return;
        }
        self.local_summons.insert(
            object_id,
            LocalSummonEntity {
                object_type,
                first_seen_ms: time_ms,
                last_seen_ms: time_ms,
                x,
                y,
            },
        );
        // Bounded: a creature that never sends a removal is forgotten once it has
        // not been seen for a while, so a long session cannot accumulate ids.
        let cutoff = time_ms - LOCAL_SUMMON_MAX_AGE_MS;
        self.local_summons
            .retain(|&id, e| id == object_id || e.last_seen_ms >= cutoff);
    }

    /// The local player's summoned creature that could have shot `target_id`: the
    /// nearest one still within its own lifetime and within reach of it. `None`
    /// when the player has none out.
    fn local_summon_attacking(&self, target_id: i32, time_ms: i64) -> Option<(i32, i32)> {
        let target = self.objects.get(&target_id)?;
        let (tx, ty) = (target.last_x, target.last_y);
        self.local_summons
            .iter()
            .filter(|(_, e)| time_ms - e.first_seen_ms <= SUMMON_SPAWN_ATTRIBUTION_MS)
            .filter_map(|(&id, e)| {
                let (dx, dy) = (e.x - tx, e.y - ty);
                let dist = (dx * dx + dy * dy).sqrt();
                (dist <= SUMMON_ATTACK_MAX_DIST).then_some((dist, id, e.object_type))
            })
            .min_by(|a, b| {
                a.0.partial_cmp(&b.0)
                    .unwrap_or(std::cmp::Ordering::Equal)
                    .then(a.1.cmp(&b.1))
            })
            .map(|(_, id, object_type)| (id, object_type))
    }

    /// The item that created a summon of `creature_type`, with the enchants it had
    /// at the time.
    ///
    /// A creature's catalog entry lists *every* item that can spawn it, so
    /// `Summon Bear` names both `Sovereign Mace` and its `(SB)` twin and the row
    /// reads "from 2 items" even though one mace was used. The player's own
    /// equipment and activation log settle it:
    ///
    /// * an equipped item that declares the creature is a candidate, and the one
    ///   whose trigger the log can still satisfy at `at_ms` wins -- a `Use` summon
    ///   needs its own activation, an ability-activated one is created by *any*
    ///   activation (the Kimono's spirits come from the Kimono, not from the
    ///   ability that happened to be pressed), and each spawning element declares
    ///   its creature's `lifeTime`, so a summon cast before the fight is still
    ///   traceable;
    /// * with nothing in the log, a candidate that is equipped now is the only
    ///   other answer, and only when it is the single one.
    fn summon_source_item(&self, creature_type: i32, at_ms: i64) -> Option<(i32, Vec<u16>)> {
        let sources = get_asset_manager().summon_sources(creature_type);
        if sources.is_empty() {
            return None;
        }
        let local = self.objects.get(&self.local_object_id)?;
        let mut candidates: Vec<SummonCandidate> = Vec::new();
        for (slot, &item) in local.equipment.iter().enumerate() {
            if item <= 0 {
                continue;
            }
            let Some(source) = sources.iter().find(|s| s.item_type == item) else {
                continue;
            };
            candidates.push(SummonCandidate {
                slot,
                item,
                trigger: source.trigger,
                lifetime_ms: source.lifetime_ms,
            });
        }
        pick_summon_source(
            &candidates,
            &self.local_ability_uses,
            &local.equipment_enchants,
            at_ms,
        )
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
        // Same idea for the boost log: the stats that a consumable, an ally's buff
        // or a status tick would move, snapshotted so the update can be diffed.
        let old_local_boost = if object_id == self.local_object_id && self.local_object_id != 0 {
            self.objects.get(&object_id).map(BoostSnapshot::of)
        } else {
            None
        };
        // Boss uptime: settle the interval that just ended using the condition
        // that applied to it, before the new state takes over. Only an engaged
        // fight tracks this.
        let old_boss_condition = if self.fights.contains_key(&object_id) {
            self.objects
                .get(&object_id)
                .map(|o| o.condition)
                .unwrap_or(0)
        } else {
            0
        };
        let (new_hp, obj_type, obj_max_hp, previous_hp) = {
            let entry = self.objects.entry(object_id).or_default();
            let previous_hp = entry.hp as i64;
            apply_stats(entry, status, time_ms);
            (
                entry.hp as i64,
                entry.object_type,
                entry.max_hp,
                previous_hp,
            )
        };
        // Diff the local player's boost-relevant stats against the snapshot taken
        // before the update, recording a transition per stat/condition that moved.
        if let Some(before) = old_local_boost {
            if let Some(after) = self.objects.get(&object_id).map(BoostSnapshot::of) {
                self.note_local_stat_changes(&before, &after, time_ms);
            }
        }
        if self.fights.contains_key(&object_id) {
            let vulnerable_before = old_boss_condition & COND_INVULNERABLE == 0;
            if let Some(fight) = self.fights.get_mut(&object_id) {
                fight.account_uptime(time_ms, vulnerable_before);
            }
        }
        // The Shatters hard mode is unlocked by two objects the group has to
        // destroy, and both are invulnerable or unreachable until then:
        //   - the Stone Idol cannot be damaged until the Void Phantasm is
        //     absorbed next to it, which turns The Bridge Sentinel into Valen the
        //     Unbreakable;
        //   - The Source only exists in hard mode, and destroying it turns the
        //     Twilight Archmage into Nox the Wild Shadow and the Forgotten King
        //     into King Azamoth.
        self.note_shatters_hm_unlock_object(object_id, previous_hp);
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
        // A tracked summoned creature keeps its last-seen position and time, which
        // is what decides whether it could have hit a target minutes later.
        if let Some(entity) = self.local_summons.get_mut(&object_id) {
            entity.last_seen_ms = time_ms;
            entity.x = status.pos.x;
            entity.y = status.pos.y;
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
            // Moonlight Village dancers/Umi floor invulnerable and leave view at
            // the end of a phase, so a zero HP reading is not a kill: their run is
            // scored only by its completion (the concluding line or the clear
            // loot). Letting the generic rule fire here would mark every dancer
            // killed and complete a run that was never cleared.
            if !hb && new_hp <= 0 && !crate::assets::is_mv_boss(fight.boss_object_type) {
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
        speaker: Option<&str>,
        text: &str,
        time_ms: i64,
    ) -> Vec<CompletedFight> {
        // The Shatters: a hard-mode boss *speaks* under its revealed name, which
        // is the only way the client can learn it (the renamed boss keeps its
        // object type), so hearing it is distinctive evidence for that boss --
        // and it works for a player who joined after the unlock objects were
        // destroyed. Latched per boss for the instance.
        if let Some(name) = speaker {
            if let Some(boss_type) = crate::assets::shatters_hm_named_boss(name) {
                if self.shatters_hm_named.insert(boss_type) {
                    tracing::info!("[SHATTERS_HM] {name} speaks: that boss is hard mode");
                }
            }
        }
        // Spectral Penitentiary: a mini-boss shouts its hard-mode taunt only when
        // every objective of its wing was cleared, so the line itself proves that
        // boss was hard mode. Latched for the instance; Murcian's own mode is
        // inferred from these latches when his fight ends.
        if let Some(boss) = crate::assets::spectral_hm_taunt_boss(text) {
            if self.spectral_hm_bosses.insert(boss) {
                tracing::info!("[SPECTRAL_HM] hard mode: {boss} cleared its objectives");
            }
            return Vec::new();
        }
        // Moonlight Village: the dancers announce the dance is over just before
        // the clear loot lands. This is the earliest completion signal, so the
        // dancer fights are scored here rather than at the (start-of-encounter)
        // dropper spawn, which also keeps the run's loot attributable to them.
        if crate::assets::is_mv_dance_concluded_text(text) {
            self.complete_mv_bosses(crate::assets::MV_DANCER_TYPES, time_ms);
            return std::mem::take(&mut self.deferred_finished);
        }
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

    /// The group activated Moonlight Village's Leisurely Mode by consuming a Tofu
    /// Delicacy (the game raises a server notification for it). The mode
    /// shortens the dance phases and cuts the loot, so the run's fights are
    /// labelled with it on the card. Latched for the current instance.
    pub fn on_mv_leisurely_mode(&mut self) {
        self.mv_leisurely = true;
    }

    /// Latch Moonlight Village's Challenge Mode when the *local player* attacked
    /// `target_id`: whacking the Challenge Gate pet-stasises them for the rest of
    /// the run, which the card labels "Petless".
    ///
    /// The gate is a curated non-boss (it has 500k HP but is never a fight), so
    /// the fight paths would skip it -- this is the only trace it leaves. Only
    /// the local player counts: a teammate taking the challenge doesn't make this
    /// player's run petless.
    fn note_mv_challenge_gate(&mut self, target_id: i32, local: bool) {
        if !local || self.mv_petless {
            return;
        }
        let is_gate = self
            .objects
            .get(&target_id)
            .is_some_and(|o| o.object_type == crate::assets::MV_CHALLENGE_GATE_TYPE);
        if is_gate {
            tracing::info!("[MV] Challenge Gate attacked by the local player: run is petless");
            self.mv_petless = true;
        }
    }

    /// The hard-mode name of a Shatters boss the group has already unlocked, or
    /// `None` when the boss is not a hard-mode rename (or its unlock object was
    /// never observed). Hard mode is per run, so the bridge and late stages are
    /// tracked separately: a group that stops after Valen keeps the regular
    /// names for the bosses it never reached.
    pub fn shatters_hm_revealed_name(&self, boss_object_type: i32) -> Option<&'static str> {
        use crate::assets::{
            SHATTERS_BRIDGE_SENTINEL_TYPE, SHATTERS_KING_TYPE, SHATTERS_TWILIGHT_ARCHMAGE_TYPE,
        };
        let revealed = self.shatters_hm_named.contains(&boss_object_type)
            || match boss_object_type {
                SHATTERS_BRIDGE_SENTINEL_TYPE => self.shatters_hm_bridge,
                SHATTERS_TWILIGHT_ARCHMAGE_TYPE | SHATTERS_KING_TYPE => self.shatters_hm_late,
                _ => false,
            };
        if revealed {
            crate::assets::shatters_hm_boss_name(boss_object_type)
        } else {
            None
        }
    }

    /// Latch The Shatters' hard-mode stages from an unlock object we can see.
    ///
    /// The two unlock objects prove hard mode in different ways:
    ///
    /// - The Stone Idol is invulnerable outside hard mode -- it only becomes
    ///   damageable once the Void Phantasm was absorbed next to it -- so *any*
    ///   pool below its maximum proves hard mode, including one that had already
    ///   lost HP before we arrived (a fight we joined late) and one that is now
    ///   empty (the group destroyed it while we watched, or just before we read
    ///   it). Its pool has to have been reported for a reading to mean anything:
    ///   an object whose HP the server never sends keeps the zero default and is
    ///   never mistaken for a damaged or destroyed one.
    /// - The Source is spawned only in hard mode at all, and it sits in the
    ///   secret wing the group only reaches past the bridge, so merely having it
    ///   in view proves both stages -- no matter how far away it is, whether its
    ///   HP is ever reported, or whether its destruction is ever seen.
    ///
    /// Seeing the Idol *leave* proves nothing on its own: in a regular Shatters
    /// it patrols, chases whoever comes close and then wanders off, so a removal
    /// -- however close to the local player it happens -- can be nothing but a
    /// view cull. Only the pool it left behind can tell a destroyed Idol from one
    /// that walked away. That leaves a deliberate gap: a hard-mode Idol killed
    /// out of view, whose pool we therefore never read, is called regular until a
    /// revealed boss proves the run's mode later.
    ///
    /// `previous_hp` is the pool before this reading (0 when it is the first we
    /// get).
    fn note_shatters_hm_unlock_object(&mut self, object_id: i32, previous_hp: i64) {
        let Some(&TrackedObject {
            object_type,
            hp,
            max_hp,
            hp_seen,
            ..
        }) = self.objects.get(&object_id)
        else {
            return;
        };
        if object_type == crate::assets::SHATTERS_THE_SOURCE_TYPE {
            // The Source sits in the Alchemy Lab, past the archmage, so by the
            // time it is destroyed the bridge fight is long over: its presence
            // proves the late stage, and only proves the bridge stage for a run
            // that has not reached the bridge yet.
            if !self.shatters_bridge_engaged {
                self.confirm_shatters_hm_bridge("The Source in view");
            }
            self.confirm_shatters_hm_late("The Source in view");
            return;
        }
        if object_type != crate::assets::SHATTERS_STONE_IDOL_TYPE
            || self.shatters_hm_bridge
            || !hp_seen
            || max_hp <= 0
        {
            return;
        }
        // A group can pull the first boss *before* destroying the Idol, so hard
        // mode can be "wasted" on a fight already under way: evidence that only
        // turns up while the bridge fight is running describes the bosses after
        // it, and the Sentinel stays regular. This is the accepted gap -- an Idol
        // killed out of view reads the same way.
        if self.shatters_bridge_engaged {
            tracing::info!(
                "[SHATTERS_HM] Stone Idol at {hp}/{max_hp} HP read after the bridge \
                 fight started: not proof of hard mode for the bridge"
            );
            return;
        }
        let (max_hp, hp) = (max_hp as i64, hp as i64);
        // Damage we can see, including a pool the group had already taken down
        // before we looked.
        let damaged = hp > 0 && hp < max_hp;
        // A pool we had seen alive is now empty.
        let destroyed = hp <= 0 && previous_hp > 0;
        if damaged || destroyed {
            self.confirm_shatters_hm_bridge(&format!("Stone Idol at {hp}/{max_hp} HP"));
        }
    }

    /// Latch The Shatters' bridge-stage hard mode, logging the evidence the first
    /// time so a run whose unlock object was never observed at all can be
    /// diagnosed from the log.
    fn confirm_shatters_hm_bridge(&mut self, evidence: &str) {
        if !self.shatters_hm_bridge {
            tracing::info!("[SHATTERS_HM] hard mode at the bridge stage: {evidence}");
            self.shatters_hm_bridge = true;
        }
    }

    /// Latch The Shatters' late-stage hard mode (the archmage and the king), with
    /// the same logging as [`Self::confirm_shatters_hm_bridge`].
    fn confirm_shatters_hm_late(&mut self, evidence: &str) {
        if !self.shatters_hm_late {
            tracing::info!("[SHATTERS_HM] hard mode at the late stage: {evidence}");
            self.shatters_hm_late = true;
        }
    }

    /// A loot bag was recorded in the current instance. Moonlight Village's
    /// mechanics bosses never die, so the run is scored Completed by its loot:
    /// the invisible `MV Dungeon Complete` / `MV Umi Complete` droppers emit the
    /// dungeon's bags when the run is cleared, and a bag recorded here therefore
    /// clears the dancers (or Umi, for her dropper's loot). The droppers' own
    /// `mob_type` is usually unresolved -- the player never hits them -- so any
    /// bag in the instance counts, guarded by the dancers having been engaged:
    /// the tutorial reward bag lands before that and must not score a clear.
    ///
    /// Returns any fights finalized as a side effect. A run whose map changes
    /// with no such loot stays Escaped.
    pub fn on_instance_loot(
        &mut self,
        mob_type: i32,
        map_seed: i32,
        time_ms: i64,
    ) -> Vec<CompletedFight> {
        if map_seed != self.current_seed
            || normalize_dungeon(&self.current_map) != crate::loot::MOONLIGHT_VILLAGE_NAME
            || !self.mv_boss_engaged
        {
            return Vec::new();
        }
        let targets: &[i32] = crate::assets::mv_loot_completion_targets(mob_type);
        self.complete_mv_bosses(targets, time_ms);
        std::mem::take(&mut self.deferred_finished)
    }

    /// Mark Moonlight Village bosses Completed for the current run and finalize
    /// their engaged fights. Mirrors what a real death would do: the types are
    /// remembered so a later re-detection cannot record a second run, and each
    /// type's live/suspended segments merge into one kill.
    fn complete_mv_bosses(&mut self, targets: &[i32], time_ms: i64) {
        for &boss_type in targets {
            self.completed_boss_types.insert(boss_type);
            if let Some(cf) = self.consolidate_completed_boss(boss_type, time_ms) {
                self.deferred_finished.push(cf);
            }
        }
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
                destination: FightDestination::for_boss(obj_type, object_id),
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
                spirits: 0,
                boss_uptime_ms: 0,
                uptime_cursor_ms: time_ms,
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
        self.local_summons.remove(&object_id);
        self.pending_summon_shots
            .retain(|&(owner, _), _| owner != object_id);
        let last = self.objects.remove(&object_id);
        // Remember what left the world, when it is the kind of object that damages the
        // boss *for* the player (see [`is_boss_damaging_mechanic`]). Everything else is
        // excluded -- a boss's own death, a pet, a player, a trap or a wall piece all
        // leave the world too, and their removals mean something else.
        if let Some(obj) = &last {
            if !obj.is_pet
                && !self.fights.contains_key(&object_id)
                && crate::assets::aux_target_for_type(obj.object_type).is_none()
                && !class_ids::is_player_class(obj.object_type)
                && is_boss_damaging_mechanic(obj.object_type)
            {
                self.object_removals.push_back(LocalObjectRemoval {
                    time_ms,
                    object_type: mechanic_row_type(obj.object_type),
                    x: obj.last_x,
                    y: obj.last_y,
                });
                while self
                    .object_removals
                    .front()
                    .is_some_and(|r| time_ms - r.time_ms > 2_000)
                {
                    self.object_removals.pop_front();
                }
            }
        }
        // The Shatters' Stone Idol needs no removal handling: it patrols, chases
        // and wanders out of view in a regular run just as it does in hard mode,
        // so a removal -- however close to the local player it happens -- proves
        // nothing on its own. The pool it left behind is the evidence, and that
        // is read where its stats arrive (see
        // [`Self::note_shatters_hm_unlock_object`]).
        // The Source needs no removal handling either: it only ever exists in
        // hard mode, so having it in view latched the route already.
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
        // Moonlight Village dancers/Umi are exempt: they floor invulnerable and
        // leave view at the end of each phase, so a low or zero HP reading is not
        // a kill. `suspend_or_finalize` holds them for the whole run and only the
        // completion path (`complete_mv_bosses`) may score them; marking them here
        // would complete a run that was never cleared.
        if !crate::assets::is_mv_boss(fight.boss_object_type)
            && (reached_zero || (!wandering && dipped_lethal))
        {
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
    /// run's clear scores the kill, so type-keyed suspension folds every
    /// re-detection into one fight per boss type instead of one record per id.
    fn suspend_or_finalize(&mut self, fight: FightState, time_ms: i64) -> Option<CompletedFight> {
        let otype = fight.boss_object_type;
        // Moonlight Village mechanics bosses are always suspended, never finalized
        // here: they leave view repeatedly across their phases, and their run is
        // only scored by its completion (the dancers' concluding line or the
        // run's loot). Holding them keeps every phase's damage and spirits on one
        // record, and lets a completion land on it however late it arrives; an
        // unfinished run is finalized as Escaped at map change.
        if crate::assets::is_mv_boss(otype) {
            match self.dormant_fights.get_mut(&otype) {
                Some(existing) => merge_fight_into(existing, fight),
                None => {
                    self.dormant_fights.insert(otype, fight);
                }
            }
            return None;
        }
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
        // A local DamagePacket against the Challenge Gate (recorded for some of
        // the local player's own hits) also proves Challenge Mode.
        self.note_mv_challenge_gate(target_id, attacker_id == self.local_object_id);
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
        // Boss uptime also accrues from other players' damage, so a fight the
        // local player is only watching still records a meaningful denominator.
        let vulnerable = if is_boss {
            self.objects
                .get(&target_id)
                .map(|o| o.condition & COND_INVULNERABLE == 0)
                .unwrap_or(true)
        } else {
            true
        };
        let destination = match aux {
            Some(aux) => FightDestination::Aux(aux.category),
            None => self
                .fights
                .get(&target_id)
                .map(|f| f.destination)
                .expect("fight ensured"),
        };
        let fight = match aux {
            Some(aux) => self.ensure_aux_fight(aux, time_ms),
            None => self.fights.get_mut(&target_id).expect("fight ensured"),
        };
        fight.account_uptime(time_ms, vulnerable);
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
        if is_local {
            // A local damage packet is the server reporting damage it attributes to
            // the local player -- the only local damage that is packet-borne, since
            // the player's own hits are reconstructed from their shot packets. It
            // gets its own total (an ability activation can be credited it) and stays
            // out of the loadout buckets, which hold self-computed weapon damage only.
            accum.packet_damage += amount;
            accum.packet_hits += 1;
            self.local_damage_events.push_back(LocalDamageEvent {
                time_ms,
                damage: amount,
                destination,
                pet_id: self.damage_capable_pet_near_local(),
                summon: self.local_summon_attacking(target_id, time_ms),
                removal: self.object_removal_near(target_id, time_ms),
            });
        } else {
            let bucket = accum
                .damage_by_equipment
                .entry((equipment, enchants))
                .or_default();
            bucket.0 += amount;
            bucket.1 += 1;
        }
        self.trim_local_event_logs();
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
        // Two facts are all the performance sections need from a use: what the
        // local player used, and when. The item is remembered as the proposed
        // source of the next stat change, and an ability-slot use opens an
        // activation window for the damage breakdown.
        self.last_local_use = Some((item_id, time_ms));
        let ability_enchants = self
            .objects
            .get(&self.local_object_id)
            .filter(|o| o.equipment[ABILITY_SLOT] == item_id)
            .map(|o| o.equipment_enchants[ABILITY_SLOT].clone());
        if let Some(enchants) = ability_enchants {
            self.local_ability_uses.push_back(LocalAbilityUse {
                time_ms,
                item_id,
                enchants,
                server_effect_window_ms: get_asset_manager()
                    .server_applied_damage_window_ms(item_id),
            });
            self.trim_local_event_logs();
        }
    }

    /// Record one local stat update as boost-window transitions. Called with the
    /// snapshot taken before the update and the one after it.
    fn note_local_stat_changes(
        &mut self,
        before: &BoostSnapshot,
        after: &BoostSnapshot,
        time_ms: i64,
    ) {
        let mut changes = before.level_changes(after);
        changes.extend(before.condition_changes(after));
        for (label, from, to) in changes {
            // A falling stat is the previous boost expiring; only a rising one can
            // be sourced to an item the player just used.
            let (source, source_item) = self.boost_source_hint(time_ms, to > from, label);
            self.local_stat_changes.push_back(LocalStatChange {
                time_ms,
                label,
                from,
                to,
                source,
                source_item,
            });
        }
        self.trim_local_event_logs();
    }

    /// Record the live character's Blood Ritual, Crucible and pet as they stand
    /// now. The processor calls this wherever the character cache changes -- a
    /// ritual switched on or off, the crucible marker, a different pet equipped --
    /// and every fight finalised afterwards is stamped with them (see
    /// [`Self::local_state_windows`]).
    ///
    /// `pet_skin` is the sprite to draw and `pet_type` the pet's own type id, which
    /// is what the wire reports for a pet: the identity that decides which pet in
    /// the world is this character's.
    pub fn set_local_snapshot(
        &mut self,
        blood_ritual: Option<String>,
        crucible: bool,
        pet_skin: i32,
        pet_type: i32,
    ) {
        self.local_snapshot = LocalSnapshot {
            blood_ritual,
            crucible,
            pet_skin: pet_skin.max(0),
            pet_type: pet_type.max(0),
        };
    }

    /// The windows a fight carries for a state the character cache reports rather
    /// than a stat the engine watched: a ritual or crucible that was on while the
    /// fight was fought is written as on for its whole length.
    ///
    /// Writing it into the fight's own record is the point. Both live on the
    /// character *now*, so a card that read them back from there -- or from the
    /// transition log, which a later switch rewrites -- made a stored fight follow
    /// every later toggle.
    fn local_state_windows(&self, started_at: i64, ended_at: i64) -> Vec<BoostWindow> {
        let mut out = Vec::new();
        // The ritual's window names the tier it was fought under, so the card can
        // render the right definition without asking the character (which may have
        // switched the ritual off since).
        let ritual = self
            .local_snapshot
            .blood_ritual
            .as_deref()
            .map(|id| format!("{CHARACTER_STATE_SOURCE}:{id}"))
            .unwrap_or_else(|| CHARACTER_STATE_SOURCE.to_string());
        for (stat, source, active) in [
            (
                "Blood Ritual",
                ritual,
                self.local_snapshot.blood_ritual.is_some(),
            ),
            (
                "Crucible",
                CHARACTER_STATE_SOURCE.to_string(),
                self.local_snapshot.crucible,
            ),
        ] {
            if !active {
                continue;
            }
            out.push(BoostWindow {
                start_offset_ms: 0,
                end_offset_ms: (ended_at - started_at).max(0),
                stat: stat.to_string(),
                from: 0,
                to: 1,
                source,
                source_item: None,
            });
        }
        out
    }

    /// The object that vanished as a hit landed, if one did -- the source of a
    /// mechanic that damages the boss *for* the player (a barrel pushed into it, a
    /// cannonball fired at it).
    ///
    /// Both sides of this were measured on a solo Nightmare Colony killed with
    /// barrels alone: all 105 hits the server credited to the player coincided with
    /// an `Explosive Barrel` removal, in the same tick, with no shot, activation or
    /// creature of theirs behind the packet. Only objects that are such a mechanic are
    /// recorded at all (see [`is_boss_damaging_mechanic`]), so a trap or a wall piece
    /// despawning at the same tick cannot claim the hit. Background: issue #74.
    fn object_removal_near(&self, target_id: i32, time_ms: i64) -> Option<i32> {
        let target = self.objects.get(&target_id)?;
        let (tx, ty) = (target.last_x, target.last_y);
        let mut best: Option<(i64, i32)> = None;
        for removal in &self.object_removals {
            let dt = (removal.time_ms - time_ms).abs();
            if dt > MECHANIC_REMOVAL_WINDOW_MS {
                continue;
            }
            let dx = removal.x - tx;
            let dy = removal.y - ty;
            if (dx * dx + dy * dy).sqrt() > MECHANIC_REMOVAL_MAX_DIST {
                continue;
            }
            if best.map_or(true, |(bd, _)| dt < bd) {
                best = Some((dt, removal.object_type));
            }
        }
        best.map(|(_, object_type)| object_type)
    }

    /// Whether the engine knows `shooter_id` as a creature of the local player's: a
    /// summon whose ownership it learned from the summon's own shoot packet, a
    /// creature it has watched as the local player's summon in the world, or the pet
    /// the character cache names.
    fn knows_local_creature(&self, shooter_id: i32) -> bool {
        if self.summon_owners.get(&shooter_id) == Some(&self.local_object_id) {
            return true;
        }
        if self.local_summons.contains_key(&shooter_id) {
            return true;
        }
        let Some(o) = self.objects.get(&shooter_id) else {
            return false;
        };
        if !o.is_pet || o.pet_type == 0 {
            return false;
        }
        let mine = self.local_snapshot.pet_type;
        if mine != 0 {
            return o.pet_type == mine;
        }
        self.pet_follows_local(shooter_id)
    }

    /// The identity of a pet credited to the local player: the skin the character
    /// cache reports, which is what ATTRIBUTES draws and names. The wire carries
    /// the pet's *type* id, which is not a sprite, so it is only the fallback for
    /// when no cache pet is known.
    fn local_pet_identity(&self, wire_type: i32) -> i32 {
        if self.local_snapshot.pet_skin > 0 {
            self.local_snapshot.pet_skin
        } else {
            wire_type
        }
    }

    /// The wire never says what caused a change, so the tracker infers it and the
    /// UI labels it as an inference. An item use is only accepted as the cause when
    /// that item's own XML declares the effect: casting a spell is also a `UseItem`
    /// (so a spell used to be blamed for any nearby Attack boost) and a treat that
    /// only raises Speed must never be blamed for an Attack change. Failing that,
    /// an on-shoot enchantment proc on the equipped gear is checked, and only then
    /// is the change left unexplained.
    fn boost_source_hint(&self, time_ms: i64, rising: bool, label: &str) -> (String, Option<i32>) {
        if !rising {
            return ("expired".to_string(), None);
        }
        let am = get_asset_manager();
        if let Some((item, at)) = self.last_local_use {
            if item >= 0 && time_ms - at <= BOOST_SOURCE_HINT_MS && self.item_can_cause(item, label)
            {
                let name = am
                    .object_name(item)
                    .filter(|n| !n.is_empty())
                    .unwrap_or_else(|| format!("Item 0x{item:X}"));
                return (name, Some(item));
            }
        }
        if let Some(proc) = self.gear_proc_source(time_ms, label) {
            return proc;
        }
        // Nothing local explains it. These arrive from the server with no source,
        // and the only distinction available is the effect itself: Weak is a
        // monster-applied debuff, everything else is credited to an ally.
        let fallback = if label == "Weak" {
            "Enemy debuff"
        } else {
            "Ally buff"
        };
        (fallback.to_string(), None)
    }

    /// Whether `item` itself declares the effect a boost label names: a stat buff
    /// for a stat label, a condition for a condition label. A label we do not map
    /// to a stat is matched as a condition, so an item is never accepted merely
    /// because the label is unfamiliar.
    fn item_can_cause(&self, item: i32, label: &str) -> bool {
        let am = get_asset_manager();
        match stat_kind_for_boost_label(label) {
            Some(stat) => am.item_declares_stat_boost(item, stat),
            None => am.item_declares_condition(item, label),
        }
    }

    /// The equipped item and enchantment whose on-shoot proc raises `label`, when
    /// a local shot landed close enough for that proc to have fired. Returns the
    /// label to show (`"Attack Boost IV (Staff of The Vital Unity)"`) and the item
    /// the UI should draw.
    fn gear_proc_source(&self, time_ms: i64, label: &str) -> Option<(String, Option<i32>)> {
        let stat = stat_kind_for_boost_label(label)?;
        if self.last_shot_ms == 0 || time_ms - self.last_shot_ms > BOOST_PROC_HINT_MS {
            return None;
        }
        let local = self.objects.get(&self.local_object_id)?;
        let am = get_asset_manager();
        for slot in 0..local.equipment.len() {
            let item = local.equipment[slot];
            if item < 0 {
                continue;
            }
            let Some(enchant) =
                am.enchant_declaring_stat_boost(&local.equipment_enchants[slot], stat)
            else {
                continue;
            };
            let enchant_name = am.enchant_name(enchant).unwrap_or_default();
            let item_name = am.object_name(item).unwrap_or_default();
            let text = match (enchant_name.is_empty(), item_name.is_empty()) {
                (false, false) => format!("{enchant_name} ({item_name} proc)"),
                (false, true) => format!("{enchant_name} (proc)"),
                (true, false) => format!("{item_name} (proc)"),
                (true, true) => format!("Item 0x{item:X} (proc)"),
            };
            return Some((text, Some(item)));
        }
        None
    }

    /// Trim the rolling local-event logs to their cap.
    fn trim_local_event_logs(&mut self) {
        while self.local_stat_changes.len() > MAX_LOCAL_EVENT_LOG {
            self.local_stat_changes.pop_front();
        }
        while self.local_ability_uses.len() > MAX_LOCAL_EVENT_LOG {
            self.local_ability_uses.pop_front();
        }
        while self.local_damage_events.len() > MAX_LOCAL_EVENT_LOG {
            self.local_damage_events.pop_front();
        }
        while self.local_volley_hits.len() > MAX_LOCAL_EVENT_LOG {
            self.local_volley_hits.pop_front();
        }
    }

    /// Rebuild the local player's boost windows for one fight from the rolling
    /// transition log. Each transition opens a window that closes at the next
    /// transition of the same label, or at the fight's end while still active.
    fn boost_windows_for(&self, started_at: i64, ended_at: i64) -> Vec<BoostWindow> {
        let mut out: Vec<BoostWindow> = Vec::new();
        let window = |change: &LocalStatChange, start: i64, end: i64| BoostWindow {
            start_offset_ms: (start - started_at).max(0),
            end_offset_ms: (end - started_at).max(0),
            stat: change.label.to_string(),
            from: change.from,
            to: change.to,
            source: change.source.clone(),
            source_item: change.source_item,
        };
        // A buff already held when the fight starts has no transition inside it, so
        // the fight's own record carries the state it was fought under instead --
        // stamped at finalize (see `local_state_windows`), which survives a later
        // switch-off where this log does not.
        for (i, change) in self.local_stat_changes.iter().enumerate() {
            if change.time_ms < started_at || change.time_ms > ended_at {
                continue;
            }
            let end = self
                .local_stat_changes
                .iter()
                .skip(i + 1)
                .find(|c| c.label == change.label && c.time_ms <= ended_at)
                .map(|c| c.time_ms)
                .unwrap_or(ended_at);
            out.push(window(change, change.time_ms, end));
        }
        out.sort_by_key(|b| (b.start_offset_ms, b.end_offset_ms));
        out
    }

    /// The local player's ability activations inside one fight, each credited the
    /// packet-borne damage that belongs to it. Damage is matched by **source**,
    /// never by "whatever was used last":
    ///
    /// * a bulletless `DamagePacket` carries no ability id, so it is credited only
    ///   to an activation of an item whose own XML declares a server-applied
    ///   effect (`DamageNova` waves, a poison's ticks, a hex detonation) and only
    ///   within that declared duration -- the mushroom a spray leaves behind
    ///   cannot be blamed on the spell cast next;
    /// * a volley the server fired for the player names its item in the
    ///   `ServerPlayerShoot` packet, so its landed hits are credited to that
    ///   item's own activation and to nothing else.
    ///
    /// An activation with no credited damage is an effect that never connected
    /// (see the UI note); it is listed anyway so nothing is silently dropped.
    fn ability_activations_for(
        &self,
        started_at: i64,
        ended_at: i64,
        destination: FightDestination,
    ) -> Vec<AbilityActivation> {
        let uses: Vec<&LocalAbilityUse> = self
            .local_ability_uses
            .iter()
            .filter(|u| u.time_ms >= started_at && u.time_ms <= ended_at)
            .collect();
        // Credit every packet to the activation that can own it, so no packet is
        // counted twice and none lands on an ability that could not have produced
        // it. Packets are scoped to their destination first: overlapping fights
        // share the same clock, not the same damage.
        let mut credited: Vec<(i64, u64)> = vec![(0, 0); uses.len()];
        for event in self
            .local_damage_events
            .iter()
            .filter(|e| e.time_ms >= started_at && e.time_ms <= ended_at)
            .filter(|e| e.destination == destination)
        {
            let owner = uses
                .iter()
                .enumerate()
                .rev()
                .find(|(_, use_)| activation_claims_packet(use_, event.time_ms));
            if let Some((i, _)) = owner {
                credited[i].0 += event.damage;
                credited[i].1 += 1;
            }
        }
        for hit in self
            .local_volley_hits
            .iter()
            .filter(|h| h.time_ms >= started_at && h.time_ms <= ended_at)
            .filter(|h| h.destination == destination)
        {
            let owner = uses.iter().enumerate().rev().find(|(_, use_)| {
                use_.item_id == hit.item_id
                    && hit.time_ms >= use_.time_ms
                    && hit.time_ms - use_.time_ms <= ABILITY_ATTRIBUTION_WINDOW_MS
            });
            if let Some((i, _)) = owner {
                credited[i].0 += hit.damage;
                credited[i].1 += 1;
            }
        }
        let mut out = Vec::with_capacity(uses.len());
        for (i, use_) in uses.iter().enumerate() {
            // The activation window still bounds the *record*: it ends at the next
            // activation or after the fixed window, whichever is sooner.
            let next = uses
                .iter()
                .map(|u| u.time_ms)
                .filter(|&t| t > use_.time_ms)
                .min()
                .unwrap_or(i64::MAX);
            let until = (use_.time_ms + ABILITY_ATTRIBUTION_WINDOW_MS)
                .min(next)
                .min(ended_at);
            out.push(AbilityActivation {
                item_id: use_.item_id,
                enchants: use_.enchants.clone(),
                start_offset_ms: (use_.time_ms - started_at).max(0),
                window_ms: (until - use_.time_ms).max(0),
                damage: credited[i].0,
                hits: credited[i].1,
            });
        }
        out
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
        // On-shoot procs (an "OnShoot Attack Boost IV" enchantment, say) apply with
        // the shot that fired them, so the moment of the last shot is what tells a
        // proc from an ally's buff landing at the same time.
        self.last_shot_ms = time_ms;
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
            tracing::warn!(
                "[COMBAT] self-compute disabled for this map: RNG not seeded (no map \
                 seed yet) at weapon={weapon_id} projectile_id={projectile_id}"
            );
            self.self_damage_available = false;
            return;
        }
        // The local player's stats must be known to compute the attack multiplier.
        let Some(local) = self.objects.get(&self.local_object_id) else {
            tracing::warn!(
                "[COMBAT] self-compute disabled for this map: local object {} not in \
                 the object table at weapon={weapon_id} projectile_id={projectile_id}",
                self.local_object_id
            );
            self.self_damage_available = false;
            return;
        };
        if !local.attack_seen {
            tracing::warn!(
                "[COMBAT] self-compute disabled for this map: local ATT stat not \
                 observed yet at weapon={weapon_id} projectile_id={projectile_id}"
            );
            self.self_damage_available = false;
            return;
        }
        // Mastery and the seasonal Crucible/Blood Ritual multipliers are
        // character data rather than object stats, so they are folded in from the
        // tracker (see `set_shot_damage_bonus`) -- the game scales the weapon's
        // whole damage range by them, and the server-side spell values the
        // ability rows use already include them.
        let atk = AttackerStats {
            mastery_mult: self.shot_damage_bonus.mastery,
            challenge_mult: self.shot_damage_bonus.challenge,
            ..local.attacker_stats()
        };
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
            tracing::warn!(
                "[COMBAT] self-compute disabled for this map: no projectile data for \
                 weapon={weapon_id} projectile_id={projectile_id} (index {proj_index}, \
                 weapon defines {} projectile type(s))",
                get_asset_manager().projectile_type_count(weapon_id)
            );
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
        // The gear this shot is credited to, snapshotted now: the hit may arrive
        // after a swap (see `PendingShot::shot_gear`).
        let mut shot_gear = (local.equipment, local.equipment_enchants.clone());
        if shot_gear.0[WEAPON_SLOT] != weapon_id {
            // The equipment snapshot lags the swap (same condition the damage-roll
            // guard above covers): trust the packet's weapon and drop the stale
            // weapon enchants rather than pair one weapon with another's.
            shot_gear.0[WEAPON_SLOT] = weapon_id;
            shot_gear.1[WEAPON_SLOT].clear();
        }
        self.pending_shots.insert(
            bullet_id,
            PendingShot {
                base_damage,
                armor_piercing: ap,
                shot_gear,
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
                        self.pending_server_shots
                            .insert(bid, (damage as i32, ap, container_type));
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
        // Whacking the Challenge Gate is the only trace Moonlight Village's
        // Challenge Mode leaves; it is a curated non-boss, so the fight paths
        // below would skip it.
        self.note_mv_challenge_gate(target_id, main_id == self.local_object_id);
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

        // The gear a weapon shot is credited to, taken from the shot itself (see
        // `PendingShot::shot_gear`) so a swap between firing and the hit landing
        // cannot move the damage to the newly equipped weapon.
        let shot_gear = self
            .pending_shots
            .get(&bullet_id)
            .map(|shot| shot.shot_gear.clone());
        // Set when this hit came from a volley the server fired for a local
        // ability activation; its damage is that ability's, not the weapon's.
        let mut volley_item: Option<i32> = None;
        let resolved = if shooter_id == self.local_object_id {
            // Weapon shot: damage was rolled from the map RNG at shoot time.
            let weapon_resolved = if self.self_damage_available {
                self.pending_shots.get(&bullet_id).cloned().map(|shot| {
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
            match weapon_resolved {
                Some(dmg) => Some(dmg),
                None => {
                    let volley = self.pending_server_shots.get(&bullet_id).copied();
                    // The packet named the item that fired this bullet. When that
                    // item is one the local player activated, the hit is that
                    // ability's own damage (a spell volley), not the weapon's, so
                    // it is credited to the ability row instead of the loadout
                    // bucket (which stays self-computed weapon damage only).
                    if let Some((_, _, item_id)) = volley {
                        if self.local_ability_uses.iter().any(|u| {
                            u.item_id == item_id
                                && u.time_ms <= time_ms
                                && time_ms - u.time_ms <= ABILITY_ATTRIBUTION_WINDOW_MS
                        }) {
                            volley_item = Some(item_id);
                        }
                    }
                    volley.map(|(base, ap, _)| {
                        damage::damage_with_defense(base, ap, defense, cond0, cond1)
                    })
                }
            }
        } else {
            // Summon shot: damage is the server-provided projectile value; apply
            // the target's defense. Summon projectiles are treated as non-armor-
            // piercing (an approximation for the rare AP-summon case).
            self.pending_summon_shots
                .get(&(shooter_id, bullet_id))
                .map(|&base| damage::damage_with_defense(base, false, defense, cond0, cond1))
        };

        let local_id = self.local_object_id;
        // A hit fired by anything other than the local player is a summon/pet hit.
        // Its own base damage came from that entity's shoot packet, so it is
        // tracked separately: the owner's row keeps it in the totals (the game
        // attributes it to the player) but not in the owner's per-hit average or
        // per-loadout buckets.
        //
        // That holds only for a creature the engine actually knows is theirs. A
        // secondary projectile names a shooter id that can belong to an object that
        // is no creature of the player's at all -- ids are reused as objects despawn
        // -- and reading that object's identity as the source invented pet rows out
        // of other players' pets (a Battle God, a Poison Drake) hit by nobody's pet
        // (see `Self::knows_local_creature`). Anything else is the player's own hit.
        let is_summon = shooter_id != local_id && self.knows_local_creature(shooter_id);
        let (summon_pet_type, summon_is_pet, summon_object_type) = if is_summon {
            self.objects
                .get(&shooter_id)
                .map(|o| {
                    // A creature that merely carries pet-ish stats is not a pet: an
                    // ability summon (a Summoner's bear, a Pocket Rat) can arrive with
                    // a `PetName` and no pet type, and crediting its hits to a "pet"
                    // with no identity hides which creature, and which item, made
                    // them. An identity is what makes a pet a pet; everything else is
                    // named and drawn by its own object type.
                    let is_pet = o.is_pet && o.pet_type != 0;
                    (
                        if is_pet {
                            self.local_pet_identity(o.pet_type)
                        } else {
                            0
                        },
                        is_pet,
                        o.object_type,
                    )
                })
                .unwrap_or((0, false, 0))
        } else {
            (0, false, 0)
        };
        // Resolved before the fight is borrowed mutably below.
        let summon_source = if is_summon && !summon_is_pet {
            self.summon_source_item(summon_object_type, time_ms)
        } else {
            None
        };
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
        // Boss uptime advances on every hit: the boss was reachable and being shot
        // at. Aux summaries have no invulnerability concept, so they count as
        // always vulnerable.
        let vulnerable = if is_boss {
            self.objects
                .get(&target_id)
                .map(|o| o.condition & COND_INVULNERABLE == 0)
                .unwrap_or(true)
        } else {
            true
        };
        let destination = match aux {
            Some(aux) => FightDestination::Aux(aux.category),
            None => self
                .fights
                .get(&target_id)
                .map(|f| f.destination)
                .expect("fight ensured"),
        };
        let fight = match aux {
            Some(aux) => self.ensure_aux_fight(aux, time_ms),
            None => self.fights.get_mut(&target_id).expect("fight ensured"),
        };
        fight.account_uptime(time_ms, vulnerable);
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
                if is_summon {
                    accum.unresolved_summon_hits += 1;
                } else {
                    accum.unresolved_local_hits += 1;
                }
                0
            }
        };
        if is_summon {
            accum.summon_hits += 1;
            accum.summon_damage += dmg;
            let entry = accum.summons.entry(shooter_id).or_default();
            entry.hits += 1;
            entry.damage += dmg;
            entry.is_pet = entry.is_pet || summon_is_pet;
            if summon_pet_type != 0 {
                entry.pet_type = summon_pet_type;
            }
            if summon_object_type != 0 {
                entry.object_type = summon_object_type;
            }
            if entry.source_item == 0 {
                if let Some((item, enchants)) = summon_source {
                    entry.source_item = item;
                    entry.source_enchants = enchants;
                }
            }
        } else if let Some(item_id) = volley_item {
            // An ability volley's hit: the packet named the ability that fired it,
            // so the damage belongs to that ability's own activation. It stays out
            // of the loadout bucket, which is the weapon breakdown and holds
            // self-computed weapon damage only -- the same reason a local
            // DamagePacket's figure is kept out of it.
            self.local_volley_hits.push_back(LocalVolleyHit {
                time_ms,
                item_id,
                damage: dmg,
                destination,
            });
            self.trim_local_event_logs();
        } else {
            // Only the player's own hits are bucketed per loadout: the bucket is
            // what the gear breakdown reports, and pet damage is not a property of
            // the equipped weapon. A weapon shot's bucket comes from the shot's own
            // gear snapshot (the gear that fired it); a server-fired shot (an
            // ability proc or volley) has no shot record, so the gear equipped when
            // it lands is the best available.
            let key = shot_gear.unwrap_or((equipment, enchants));
            let bucket = accum.damage_by_equipment.entry(key).or_default();
            bucket.0 += dmg;
            bucket.1 += 1;
        }
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

        // Engaging a dancer/Umi ends the boss-less tutorial: from here on,
        // released spirits score.
        if crate::assets::is_mv_boss(object_type) {
            self.mv_boss_engaged = true;
        }

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
                destination: FightDestination::for_boss(object_type, target_id),
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
                // Spirits released between fights (their phase end raced the
                // previous fight's finalize) belong to this one.
                spirits: if crate::assets::is_mv_boss(object_type) {
                    std::mem::take(&mut self.pending_mv_spirits)
                } else {
                    0
                },
                boss_uptime_ms: 0,
                uptime_cursor_ms: time_ms,
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
                destination: FightDestination::Aux(aux.category),
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
                spirits: 0,
                boss_uptime_ms: 0,
                uptime_cursor_ms: time_ms,
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
        // Moonlight Village spirits released after the run's last dancer/Umi
        // fight finalized (a phase-end burst racing the finalize) still belong to
        // the run: credit them to its most recent MV fight, so the run total stays
        // exact even though the individual burst was late.
        if self.pending_mv_spirits > 0 {
            if let Some(f) = fights
                .iter_mut()
                .filter(|f| crate::assets::is_mv_boss(f.boss_object_type))
                .max_by_key(|f| (f.last_activity, f.started_at, f.boss_object_id))
            {
                f.spirits += std::mem::take(&mut self.pending_mv_spirits);
            }
        }
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
        // The Shatters hard mode is a property of the run (see
        // [`Self::shatters_hm_revealed_name`]), not of the boss packets: the game
        // only reveals the renamed boss in dialogue, so the card carries it.
        // Nothing about the Idol's *absence* can prove it either -- a regular
        // run's Idol sits across the map and is easy to never see -- so the latch
        // comes solely from the unlock objects we actually witnessed (its
        // destruction, a damaged pool, or The Source in view). A run that never
        // showed either stays regular, which is the honest reading at the cost of
        // a hard-mode run whose Idol died elsewhere being called regular until
        // its revealed boss is fought.
        let shatters_hm = self
            .shatters_hm_revealed_name(fight.boss_object_type)
            .is_some();
        // Aggregated aux fights carry an explicit name ("Marble Core"); real
        // bosses resolve theirs from assets and may carry a segment suffix
        // ("Marble Colossus (Post-survival)").
        let boss_name = match fight.display_name_override {
            Some(name) => name.to_string(),
            None => {
                let base = match self.shatters_hm_revealed_name(fight.boss_object_type) {
                    Some(name) => name.to_string(),
                    None => assets
                        .object_name(fight.boss_object_type)
                        .unwrap_or_else(|| format!("Boss 0x{:04X}", fight.boss_object_type as u16)),
                };
                match fight.segment_label {
                    Some(label) => format!("{base} ({label})"),
                    None => base,
                }
            }
        };

        let dungeon = normalize_dungeon(&self.current_map);
        // Spectral Penitentiary's hard mode is a property of the run, inferred
        // from its mini-bosses' objective-cleared taunts (see
        // [`Self::on_boss_text`]): a mini-boss carries the mode when it taunted,
        // and Soulwarden Murcian -- who has no taunt of his own -- carries it only
        // when both mini-bosses did, which is what makes the dungeon hard mode.
        // Marking his row is what the card's "(HM)" tag reads.
        let spectral_hm = if crate::assets::is_spectral_murcian(fight.boss_object_type) {
            self.spectral_hm_bosses.len() >= SPECTRAL_HM_BOSSES_REQUIRED
        } else {
            // The type is what ties a latched taunt to this boss's own card; the
            // name is a fallback for a form carrying an unexpected type.
            let taunted = crate::assets::spectral_hm_taunt_boss_of_type(fight.boss_object_type);
            let name = boss_name.trim();
            self.spectral_hm_bosses
                .iter()
                .any(|boss| Some(*boss) == taunted || boss.eq_ignore_ascii_case(name))
        };
        // Some bosses fought from the realm belong to a rated dungeon the raw
        // map name doesn't reflect (Oryx the Mad God 2 -> Wine Cellar). Remap so
        // the fight classifies by that dungeon's difficulty and shows its card.
        let dungeon = crate::assets::canonical_dungeon_for_boss_named(
            fight.boss_object_type,
            boss_name.trim(),
        )
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
        let fight_started_at = fight.started_at;
        let fight_destination = fight.destination;
        // Server-credited, bulletless damage first: a pet's or an ability-summon's
        // hits have to reach their own rows before the participant is built.
        self.attribute_owner_packet_damage(
            &mut fight,
            fight_started_at,
            time_ms,
            fight_destination,
        );
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
        //
        // Moonlight Village mechanics bosses are exempt: they never lose HP (the
        // run is scored by its clear, not by a death), so their nominal max HP is
        // not the damage they absorbed. Capping there would truncate the party's
        // real output, so raw damage is kept as-is and shares are later taken
        // against the tracked total.
        if fight.killed && !crate::assets::is_mv_boss(fight.boss_object_type) {
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

        // The stat/condition boosts the player was observed under, plus the state
        // the character cache reports (see `local_state_windows`): both belong to
        // the fight's own record, so nothing can rewrite them afterwards.
        let mut boosts = self.boost_windows_for(fight.started_at, time_ms);
        boosts.extend(self.local_state_windows(fight.started_at, time_ms));
        boosts.sort_by_key(|b| (b.start_offset_ms, b.end_offset_ms));

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
            boss_uptime_ms: fight.boss_uptime_ms,
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
            spirits: fight.spirits,
            leisurely: self.mv_leisurely,
            petless: self.mv_petless,
            spectral_hm,
            shatters_hm,
            boosts,
            abilities: self.ability_activations_for(fight.started_at, time_ms, fight_destination),
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
            let prov = if accum.unresolved_local_hits > 0
                || accum.unresolved_summon_hits > 0
                || accum.hits == 0
            {
                // Estimate the unvalued hits from the average of the resolved ones
                // instead of leaving them at zero. An unresolved EnemyHit still
                // landed on the target, so zero systematically under-counts; the
                // average is a far better approximation. The two populations are
                // averaged separately, so a missed pet hit is never valued at the
                // player's per-hit rate (and vice versa).
                let own_damage = accum.damage - accum.summon_damage;
                let own_resolved_hits = resolved_hits.saturating_sub(
                    accum
                        .summon_hits
                        .saturating_sub(accum.unresolved_summon_hits),
                );
                let own_avg = if own_resolved_hits > 0 {
                    own_damage / own_resolved_hits as i64
                } else {
                    0
                };
                let resolved_summon_hits = accum
                    .summon_hits
                    .saturating_sub(accum.unresolved_summon_hits);
                let summon_avg = if resolved_summon_hits > 0 {
                    accum.summon_damage / resolved_summon_hits as i64
                } else {
                    0
                };
                let extra = (accum.unresolved_local_hits as i128 * own_avg as i128)
                    .saturating_add(accum.unresolved_summon_hits as i128 * summon_avg as i128)
                    .min(i64::MAX as i128) as i64;
                if accum.damage > 0 && extra > 0 {
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
            // Recorded for every participant, a zero included: the flawless
            // marker has to tell "never hit" apart from "this fight predates
            // hits-taken tracking" (a NULL row), and only a recorded zero can.
            damage_taken_hits: Some(accum.damage_taken_hits),
            damage_blocked,
            guarded_damage: Some(accum.guarded_damage),
            guarded_hits: Some(accum.guarded_hits),
            summon_damage: accum.summon_damage,
            summon_hits: accum.summon_hits,
            summons: {
                let mut v: Vec<SummonContribution> = accum
                    .summons
                    .iter()
                    .map(|(&object_id, s)| SummonContribution {
                        object_id,
                        pet_type: s.pet_type,
                        is_pet: s.is_pet,
                        object_type: s.object_type,
                        source_item: s.source_item,
                        source_enchants: s.source_enchants.clone(),
                        damage: s.damage,
                        hits: s.hits,
                    })
                    .collect();
                v.sort_by(|a, b| b.damage.cmp(&a.damage).then(b.hits.cmp(&a.hits)));
                v
            },
            // Per-weapon rows are only meaningful for the local player, whose
            // damage is reconstructed from their own shot packets. Remote rows are
            // whole damage packets with no weapon attribution.
            weapons: if is_local {
                aggregate_weapons(&accum.damage_by_equipment)
            } else {
                Vec::new()
            },
            packet_damage: accum.packet_damage,
            packet_hits: accum.packet_hits,
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
            // The local player's pet carries its identity from the character cache
            // instead of the type id the wire sent (see `local_pet_identity`).
            let pet_type = if p.is_local {
                self.local_pet_identity(track.pet_type)
            } else {
                track.pet_type
            };
            p.pet = Some(PetInfo {
                pet_type,
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

    /// The damage-capable pet the local player has out, if any.
    ///
    /// A pet's hits are credited to its owner by the server and arrive as
    /// bulletless `DamagePacket`s naming the player, so the pet's own presence is
    /// the only handle on them. "Damage-capable" uses the ability ids `pets.xml`
    /// declares (Attack Close/Mid/Far, Electric); heal, magic heal, Savage and
    /// Decoy cannot damage anything. Only a pet close enough to its owner to be
    /// the shooter counts, and the nearest wins (ties by lowest id).
    ///
    /// *Which* pet that is comes from the character cache -- the pet ATTRIBUTES
    /// shows -- and not from distance. The wire reports every pet in the world with
    /// its own type id, and the character's own pet has a known one, so only a pet
    /// answering to that type can claim their damage: in a group a teammate's pet
    /// stands right beside them, and distance alone credited 943 damage the server
    /// reported to the local player to a teammate's Great Jelly, while their own pet
    /// was a Blue Ant that cannot damage anything. When the character's own pet is
    /// not in the world at all, nothing claims -- an unattributed packet is better
    /// than a teammate's pet wearing their damage.
    ///
    /// A character whose pet the cache does not know falls back to the pet that
    /// follows the local player (see [`Self::pet_follows_local`]).
    fn damage_capable_pet_near_local(&self) -> Option<i32> {
        let mine = self.local_snapshot.pet_type;
        if mine != 0 {
            return self.nearest_damage_capable_pet(|_, o| o.pet_type == mine);
        }
        self.nearest_damage_capable_pet(|_, _| true)
    }

    /// The nearest pet within [`PET_ASSOC_MAX_DIST`] of the local player that can
    /// deal damage, follows the local player, and passes `keep`; ties are broken by
    /// the lowest object id.
    fn nearest_damage_capable_pet(
        &self,
        keep: impl Fn(i32, &TrackedObject) -> bool,
    ) -> Option<i32> {
        let local = self.objects.get(&self.local_object_id)?;
        let (lx, ly) = (local.last_x, local.last_y);
        let mut best: Option<(f32, i32)> = None;
        for (&oid, o) in &self.objects {
            if !o.is_pet || o.pet_type == 0 {
                continue;
            }
            if !o.pet_abilities.iter().any(|&a| pet_ability_deals_damage(a)) {
                continue;
            }
            if !keep(oid, o) || !self.pet_follows_local(oid) {
                continue;
            }
            let dx = o.last_x - lx;
            let dy = o.last_y - ly;
            let dist = (dx * dx + dy * dy).sqrt();
            if dist > PET_ASSOC_MAX_DIST {
                continue;
            }
            if best.map_or(true, |(bd, bid)| dist < bd || (dist == bd && oid < bid)) {
                best = Some((dist, oid));
            }
        }
        best.map(|(_, oid)| oid)
    }

    /// Whether a pet object follows the local player, by the same proximity vote
    /// that resolves a participant's own pet row ([`Self::assign_pets`]).
    ///
    /// A pet with no vote recorded at all is left to proximity: a fight whose pet
    /// was never observed following anyone keeps the behaviour it had, while a pet
    /// whose owner *is* known -- every pet in a group fight -- has to be the local
    /// player's before it can claim their damage.
    fn pet_follows_local(&self, pet_id: i32) -> bool {
        let Some(track) = self.pet_tracks.get(&pet_id) else {
            return true;
        };
        match track
            .votes
            .iter()
            .max_by(|a, b| a.1.cmp(b.1).then(b.0.cmp(a.0)))
        {
            Some((&owner, &votes)) if votes > 0 => owner == self.local_object_id,
            _ => true,
        }
    }

    /// The summons an activation of the local player's loadout can spawn, as
    /// `(creature object type, the item whose own use spawns it or None when any
    /// activation does)`.
    ///
    /// `Flowering Kimono` declares `OnPlayerAbilityActivate Pet` for its spirits:
    /// any activation of any of the player's abilities spawns them. An item whose
    /// summon is declared on its own `Use` is only spawned by its own activations,
    /// so that distinction is kept.
    fn activation_spawned_summons_of_local_loadout(&self) -> Vec<(i32, Option<i32>)> {
        let Some(local) = self.objects.get(&self.local_object_id) else {
            return Vec::new();
        };
        let manager = get_asset_manager();
        let mut out: Vec<(i32, Option<i32>)> = Vec::new();
        for &item in local.equipment.iter() {
            if item <= 0 {
                continue;
            }
            for (creature, source) in manager.summoned_by_item(item) {
                // A summon the item's own text never claims damage for (the Marble
                // Seal's Eye of Marble buffers allies) cannot be the source of a
                // damage packet, however close it stands to the target.
                if !source.deals_damage {
                    continue;
                }
                let only_item = match source.trigger {
                    SummonTrigger::OnAbilityUse => None,
                    SummonTrigger::Use => Some(item),
                    _ => continue,
                };
                if !out.contains(&(creature, only_item)) {
                    out.push((creature, only_item));
                }
            }
        }
        out.sort_unstable();
        out
    }

    /// Split the local player's bulletless `DamagePacket`s into the pet/summon
    /// rows they came from, and only that much more -- what nothing explains stays
    /// in `packet_damage` as an unattributed figure.
    ///
    /// The packet names the *owner*, never the shooter, so a source has to be
    /// decided by elimination against what else could have produced the damage:
    ///
    /// * an activation whose item declares a server-applied damage effect already
    ///   claims its own window (see [`Self::ability_activations_for`]), so those
    ///   events are not candidates here and cannot be counted twice;
    /// * of the rest, one landing within [`SUMMON_SPAWN_ATTRIBUTION_MS`] of an
    ///   activation that can spawn the loadout's ability-summon belongs to that
    ///   creature;
    /// * otherwise one that landed while a damage-capable pet of the player was
    ///   out and in range belongs to that pet.
    ///
    /// Both kinds are credited where a source-exact summon hit goes: inside the
    /// owner's total, outside its weapon and ability buckets.
    fn attribute_owner_packet_damage(
        &self,
        fight: &mut FightState,
        started_at: i64,
        ended_at: i64,
        destination: FightDestination,
    ) {
        let local_id = self.local_object_id;
        let events: Vec<LocalDamageEvent> = self
            .local_damage_events
            .iter()
            .filter(|e| e.time_ms >= started_at && e.time_ms <= ended_at)
            .filter(|e| e.destination == destination)
            .copied()
            .collect();
        if events.is_empty() {
            return;
        }
        let uses: Vec<&LocalAbilityUse> = self
            .local_ability_uses
            .iter()
            .filter(|u| u.time_ms >= started_at && u.time_ms <= ended_at)
            .collect();
        let summons = self.activation_spawned_summons_of_local_loadout();
        if summons.is_empty()
            && events
                .iter()
                .all(|e| e.pet_id.is_none() && e.removal.is_none())
        {
            return;
        }
        // Creatures seen in the world during this fight, which outrank timing when
        // more than one of the loadout's summons could have fired.
        let mut seen_creatures: HashSet<i32> = self
            .local_summons
            .values()
            .filter(|e| e.last_seen_ms >= started_at && e.last_seen_ms <= ended_at)
            .map(|e| e.object_type)
            .collect();
        seen_creatures.extend(events.iter().filter_map(|e| e.summon).map(|(_, t)| t));
        let Some(accum) = fight.attackers.get_mut(&local_id) else {
            return;
        };
        for event in events {
            let source = match packet_damage_source(&uses, &summons, &seen_creatures, &event) {
                Some(source) => source,
                // Nothing the player fired explains this packet. An object that left
                // the world as it landed does: a mechanic that damages the boss for
                // the player (see `object_removal_near`). Its row is drawn from the
                // object's own type, so it reads and sprites as what it is. A packet
                // carrying no damage names nothing -- it would only add empty rows for
                // whatever else happened to despawn at that tick.
                None => match event.removal.filter(|_| event.damage > 0) {
                    Some(creature) => PacketDamageSource::Summon {
                        creature,
                        object_id: None,
                    },
                    None => continue,
                },
            };
            // A creature the packet never names is keyed by the negative of its
            // object type, which no live object id can collide with; the type
            // itself is what names and draws the row.
            let (key, is_pet, pet_type, object_type) = match source {
                PacketDamageSource::Summon {
                    creature,
                    object_id,
                } => {
                    // A creature the packet never named, and never seen in the
                    // world either, is keyed by the negative of its object type --
                    // which no live object id can collide with. The type itself is
                    // what names and draws the row either way.
                    (object_id.unwrap_or(-creature), false, 0, creature)
                }
                PacketDamageSource::Pet { pet_id } => {
                    // Identified by the cache, like every other pet row credited to
                    // the local player: the wire's type id is not a sprite.
                    let (pet_type, object_type) = self
                        .objects
                        .get(&pet_id)
                        .map(|o| (self.local_pet_identity(o.pet_type), o.object_type))
                        .unwrap_or((0, 0));
                    (pet_id, true, pet_type, object_type)
                }
            };
            accum.summon_damage += event.damage;
            accum.summon_hits += 1;
            let entry = accum.summons.entry(key).or_default();
            entry.damage += event.damage;
            entry.hits += 1;
            entry.is_pet = entry.is_pet || is_pet;
            if pet_type != 0 {
                entry.pet_type = pet_type;
            }
            if object_type != 0 {
                entry.object_type = object_type;
            }
            // The item behind the creature, resolved from the player's own
            // loadout and activation log rather than from the catalog, which
            // cannot tell an item from its `(SB)` twin.
            if entry.source_item == 0 && !is_pet {
                if let Some((item, enchants)) = self.summon_source_item(object_type, event.time_ms)
                {
                    entry.source_item = item;
                    entry.source_enchants = enchants;
                }
            }
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

    /// Assign buffered gravestones to departed participants closest pair first,
    /// consuming the claimed graves. Each participant claims at most one grave and
    /// each grave is claimed at most once; resolving the closest pairs before the
    /// farther ones is what stops a clustered death from stealing the grave that
    /// sits on a neighbour, which would otherwise leave that neighbour with no
    /// grave at all (and mark them Nexused despite having died).
    fn match_graves_nearest_first(
        &mut self,
        departures: &[(usize, Departure, bool)],
    ) -> Vec<Option<i32>> {
        let r2 = GRAVE_MATCH_RADIUS * GRAVE_MATCH_RADIUS;
        let mut pairs: Vec<(f32, usize, usize)> = Vec::new();
        for (di, (_, d, _)) in departures.iter().enumerate() {
            for (gi, g) in self.recent_graves.iter().enumerate() {
                if (g.time - d.time).abs() > GRAVE_MATCH_WINDOW_MS {
                    continue;
                }
                let dx = g.x - d.x;
                let dy = g.y - d.y;
                let dist2 = dx * dx + dy * dy;
                if dist2 <= r2 {
                    pairs.push((dist2, di, gi));
                }
            }
        }
        pairs.sort_by(|a, b| a.0.total_cmp(&b.0));

        let mut assigned = vec![None; departures.len()];
        let mut claimed = vec![false; self.recent_graves.len()];
        for (_, di, gi) in pairs {
            if assigned[di].is_none() && !claimed[gi] {
                assigned[di] = Some(self.recent_graves[gi].grave_type);
                claimed[gi] = true;
            }
        }
        if claimed.iter().any(|c| *c) {
            self.recent_graves = self
                .recent_graves
                .iter()
                .zip(&claimed)
                .filter(|(_, c)| !**c)
                .map(|(g, _)| *g)
                .collect();
        }
        assigned
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
        // Participants that left the fight and may still own a gravestone, with
        // their departure and whether the death broadcast confirmed them. Graves
        // are assigned after the loop so the matching can run nearest-first across
        // the whole roster instead of in roster order (see `match_graves_nearest_first`).
        let mut departed: Vec<(usize, Departure, bool)> = Vec::new();

        for (idx, p) in participants.iter_mut().enumerate() {
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
            match self
                .departures
                .get(&p.object_id)
                .copied()
                .filter(|d| d.time >= started - DEPARTURE_GRACE_MS)
            {
                Some(dep) => departed.push((idx, dep, name_died)),
                None if name_died => p.end_status = ParticipantEndStatus::Died { grave_type: 0 },
                None => p.end_status = ParticipantEndStatus::Present,
            }
        }

        let graves = self.match_graves_nearest_first(&departed);
        for (i, (idx, _, name_died)) in departed.iter().enumerate() {
            participants[*idx].end_status = match (graves[i], name_died) {
                (Some(grave_type), _) => ParticipantEndStatus::Died { grave_type },
                (None, true) => ParticipantEndStatus::Died { grave_type: 0 },
                // No death evidence: a departure inside the fight means they left
                // (nexused); no departure means they were simply present.
                (None, false) => ParticipantEndStatus::Nexused,
            };
        }
    }

    /// Record the local player's death from a `CharacterDied` game event so the
    /// finalizing fight can mark the local participant as dead with the right
    /// gravestone sprite. Buffered until the current fight(s) finalize.
    pub fn on_local_death(&mut self, char_id: i32, gravestone_type: i32, time_ms: i64) {
        self.pending_local_grave = Some((char_id, gravestone_type, time_ms));
    }

    /// Record a remote player's death from the server death broadcast (`s.death`,
    /// rendering as "<name> died at level N, killed by <killer>"). Buffered by name
    /// until the current fight(s) finalize; this authoritative signal is what marks
    /// a clustered-death player as Died even when their gravestone couldn't be
    /// position-correlated (see `resolve_end_statuses`). The raw broadcast is also
    /// queued for the caller to persist (see `take_pending_death_broadcasts`), so a
    /// death announced after its fight was written can still be applied to it.
    pub fn on_remote_death(&mut self, player_name: &str, killer: &str, time_ms: i64) {
        let key = sanitize_player_name(player_name).to_ascii_lowercase();
        if key.is_empty() {
            return;
        }
        self.remote_deaths.insert(key, time_ms);
        if self.pending_death_broadcasts.len() >= CORRELATION_MAX_ENTRIES {
            self.pending_death_broadcasts.remove(0);
        }
        self.pending_death_broadcasts.push(DeathBroadcast {
            player: player_name.trim().to_string(),
            killer: killer.trim().to_string(),
            time: time_ms,
        });
    }

    /// Take the death broadcasts observed since the last drain, for persistence.
    pub fn take_pending_death_broadcasts(&mut self) -> Vec<DeathBroadcast> {
        std::mem::take(&mut self.pending_death_broadcasts)
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
            x if x == StatType::HP as u8 => {
                obj.hp = stat.stat_value;
                obj.hp_seen = true;
            }
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
            x if x == StatType::AttackBoost as u8 => obj.attack_boost = stat.stat_value,
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
            x if x == StatType::Vitality as u8 => {
                obj.vitality = stat.stat_value;
                obj.vitality_seen = true;
            }
            x if x == StatType::Speed as u8 => {
                obj.speed = stat.stat_value;
                obj.speed_seen = true;
            }
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
    dst.spirits += src.spirits;
    dst.boss_uptime_ms += src.boss_uptime_ms;
    dst.uptime_cursor_ms = dst.uptime_cursor_ms.max(src.uptime_cursor_ms);
    for (aid, accum) in src.attackers {
        let e = dst.attackers.entry(aid).or_default();
        e.damage += accum.damage;
        e.hits += accum.hits;
        e.is_local = e.is_local || accum.is_local;
        e.guarded_damage += accum.guarded_damage;
        e.guarded_hits += accum.guarded_hits;
        e.unresolved_local_hits += accum.unresolved_local_hits;
        e.summon_damage += accum.summon_damage;
        e.summon_hits += accum.summon_hits;
        e.packet_damage += accum.packet_damage;
        e.packet_hits += accum.packet_hits;
        e.unresolved_summon_hits += accum.unresolved_summon_hits;
        for (sid, s) in accum.summons {
            let entry = e.summons.entry(sid).or_default();
            entry.damage += s.damage;
            entry.hits += s.hits;
            entry.is_pet = entry.is_pet || s.is_pet;
            if s.pet_type != 0 {
                entry.pet_type = s.pet_type;
            }
        }
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

/// Collapse a participant's per-loadout damage buckets into one row per weapon
/// (item id plus the enchants it was fired with), summing across loadouts so a
/// mid-fight armor or ring swap does not split the same weapon's contribution.
/// Highest damage first.
fn aggregate_weapons(
    buckets: &HashMap<([i32; 4], [Vec<u16>; 4]), (i64, u64)>,
) -> Vec<WeaponContribution> {
    let mut out: Vec<WeaponContribution> = Vec::new();
    for ((gear, enchants), &(damage, hits)) in buckets {
        let weapon_id = gear[WEAPON_SLOT];
        if weapon_id < 0 {
            continue;
        }
        let weapon_enchants = enchants[WEAPON_SLOT].clone();
        match out
            .iter_mut()
            .find(|w| w.weapon_id == weapon_id && w.enchants == weapon_enchants)
        {
            Some(w) => {
                w.damage += damage;
                w.hits += hits;
            }
            None => out.push(WeaponContribution {
                weapon_id,
                enchants: weapon_enchants,
                damage,
                hits,
            }),
        }
    }
    out.sort_by(|a, b| b.damage.cmp(&a.damage).then(b.hits.cmp(&a.hits)));
    out
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

    /// The stat an ability summon can carry with no pet type behind it, which is
    /// what used to turn a Pocket Rat's hits into a nameless "Pet" row.
    fn pet_name_stat(name: &str) -> StatData {
        StatData {
            stat_type_id: StatType::PetName as u8,
            stat_type: StatType::PetName,
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
            damage_taken_hits: None,
            damage_blocked: None,
            guarded_damage: None,
            guarded_hits: None,
            summon_damage: 0,
            summon_hits: 0,
            summons: Vec::new(),
            weapons: Vec::new(),
            packet_damage: 0,
            packet_hits: 0,
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
            destination: FightDestination::Boss(500),
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
            spirits: 0,
            boss_uptime_ms: 0,
            uptime_cursor_ms: started_at,
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
    fn end_status_clustered_deaths_each_claim_their_own_grave() {
        // Two players die within the match radius of each other and of both
        // graves. Roster order must not decide ownership: the nearer departure
        // claims its own grave and the other is left with its grave, not Nexused.
        let mut t = CombatTracker::new();
        // Listed farthest-first, which is what used to steal the near grave.
        t.departures.insert(
            1,
            Departure {
                x: 12.0,
                y: 12.0,
                time: 1_000,
            },
        );
        t.departures.insert(
            2,
            Departure {
                x: 10.0,
                y: 10.0,
                time: 1_000,
            },
        );
        // Grave A sits on participant 2, grave B on participant 1.
        t.recent_graves.push_back(GraveSpawn {
            grave_type: 1829,
            x: 10.1,
            y: 10.1,
            time: 1_100,
        });
        t.recent_graves.push_back(GraveSpawn {
            grave_type: 1837,
            x: 12.1,
            y: 12.1,
            time: 1_100,
        });

        let mut ps = vec![participant(1, false), participant(2, false)];
        t.resolve_end_statuses(&mut ps, 0, 5_000, 0, false);

        assert_eq!(
            ps[0].end_status,
            ParticipantEndStatus::Died { grave_type: 1837 }
        );
        assert_eq!(
            ps[1].end_status,
            ParticipantEndStatus::Died { grave_type: 1829 }
        );
        // Both graves were consumed.
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
        t.on_remote_death("Zannmp", "Oryx the Mad God", 1_050);
        t.on_remote_death("Yumiho", "Oryx the Mad God", 1_050);

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
    fn remote_death_broadcasts_are_queued_for_persistence() {
        let mut t = CombatTracker::new();
        t.on_remote_death("Pillar", "Oryx the Mad God", 1_234);
        // A broadcast carrying no player name persists nothing.
        t.on_remote_death("   ", "a Spider", 2_000);
        let queued = t.take_pending_death_broadcasts();
        assert_eq!(queued.len(), 1);
        assert_eq!(queued[0].player, "Pillar");
        assert_eq!(queued[0].killer, "Oryx the Mad God");
        assert_eq!(queued[0].time, 1_234);
        assert!(t.take_pending_death_broadcasts().is_empty());
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
        t.on_remote_death("Later", "Oryx the Mad God", 500_000);
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
        // are completed by the run's clear (the dancers' concluding line, or their
        // loot). The completion scores the dancer as killed, so the local player is
        // recorded Present / Completed rather than Nexused.
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
        // The dance concludes: the dancer is scored Completed and its record is
        // emitted. The local player is still present at that moment.
        let done = t.on_boss_text(500, None, "This concludes the Moonlight Dance.", 350);
        assert_eq!(done.len(), 1);
        assert!(done[0].killed, "completion scores the dancer as killed");
        let local = done[0]
            .participants
            .iter()
            .find(|p| p.is_local)
            .expect("local");
        assert_eq!(local.end_status, ParticipantEndStatus::Present);
        // A later nexus adds nothing: the completion was already recorded.
        assert!(
            t.on_map_change("Nexus", 0, 500).is_empty(),
            "no duplicate record after the nexus"
        );
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
    fn shot_damage_bonus_defaults_neutral_and_is_settable() {
        // The engine must start at the raw roll (what every pre-fix fight was
        // credited with) and take whatever the character data resolves to. The
        // roll arithmetic itself is covered in damage.rs; this pins the plumbing
        // the app pushes through.
        let mut t = CombatTracker::new();
        assert_eq!(
            t.shot_damage_bonus,
            crate::stats::ShotDamageBonus::none(),
            "a new tracker applies no shot multiplier"
        );
        let bonus = crate::stats::ShotDamageBonus {
            mastery: 1.1,
            challenge: 1.24,
        };
        t.set_shot_damage_bonus(bonus);
        assert_eq!(t.shot_damage_bonus, bonus);
        assert!((t.shot_damage_bonus.total() - 1.364).abs() < 1e-6);
        // Back to neutral: the pre-fix numbers exactly.
        t.set_shot_damage_bonus(crate::stats::ShotDamageBonus::none());
        assert_eq!(t.shot_damage_bonus.total(), 1.0);
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
        let split = t.on_boss_text(500, None, "...!", 25);
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
        assert_eq!(t.on_boss_text(500, None, "...!", 25).len(), 1);

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
        assert!(t.on_boss_text(500, None, "Fear the halls!", 22).is_empty());
        // The transition taunt from an unrelated object id must not split.
        assert!(t.on_boss_text(999, None, "...!", 23).is_empty());
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
    fn curated_event_adds_never_become_their_own_fight() {
        // The Goblin Patriarch Adept Encounter entries -- Outpost (34554), Shaman
        // (34555, shown as "Goblin Priest"), Villager (34556) and Fire (34557) --
        // plus the Legion Soldier (53007) are label-less event entries that the
        // realm event scales past the boss fallback, so the curated deny-list is
        // the only thing keeping them out of combat history, even when the local
        // player lands hits on them.
        for (object_type, name) in [
            (34554, "Goblin Outpost"),
            (34555, "Goblin Priest"),
            (34556, "Goblin Villager"),
            (34557, "Goblin Fire"),
            (53007, "Legion Soldier"),
        ] {
            let mut t = CombatTracker::new();
            t.on_map_change("Realm", 1, 0);
            t.on_player_loaded(1000, 1);
            t.on_object_spawn(
                500,
                object_type,
                &status(
                    500,
                    vec![stat(StatType::MaxHP, 20_000), stat(StatType::HP, 20_000)],
                ),
                0,
            );
            t.on_damage(500, 1000, 5_000, 10);
            t.on_local_hit(500, 7, 1000, 1000, 12);
            t.on_object_status(500, &status(500, vec![stat(StatType::HP, 0)]), 20);
            let done = t.on_map_change("Nexus", 0, 100);
            assert!(done.is_empty(), "{name} must not be tracked as a fight");
        }
    }

    #[test]
    fn curated_demonic_effigy_becomes_its_own_fight() {
        // The Hero of Oryx set-piece "Demonic Effigy" (0x86F7) is a label-less
        // realm boss whose 9k HP sits *below* the label-less boss fallback, so it
        // used to be dropped: its loot was attributed to it while no fight was
        // ever recorded. The curated allow-list is what promotes it.
        let mut t = CombatTracker::new();
        t.on_map_change("Realm", 1, 0);
        t.on_player_loaded(1000, 1);
        t.on_object_spawn(
            500,
            0x86F7,
            &status(
                500,
                vec![stat(StatType::MaxHP, 9_000), stat(StatType::HP, 9_000)],
            ),
            0,
        );
        t.on_damage(500, 1000, 4_000, 10);
        t.on_local_hit(500, 7, 1000, 1000, 12);
        t.on_object_status(500, &status(500, vec![stat(StatType::HP, 0)]), 20);
        let done = t
            .on_object_removed(500, 24)
            .expect("the effigy is tracked as a fight");
        assert_eq!(done.boss_object_type, 0x86F7);

        // Its Worshipper adds (0x86DA, 5.5k HP, also label-less) stay out.
        let mut t = CombatTracker::new();
        t.on_map_change("Realm", 1, 0);
        t.on_player_loaded(1000, 1);
        t.on_object_spawn(
            600,
            0x86DA,
            &status(
                600,
                vec![stat(StatType::MaxHP, 5_500), stat(StatType::HP, 5_500)],
            ),
            0,
        );
        t.on_damage(600, 1000, 2_000, 10);
        t.on_local_hit(600, 7, 1000, 1000, 12);
        t.on_object_status(600, &status(600, vec![stat(StatType::HP, 0)]), 20);
        assert!(
            t.on_map_change("Nexus", 0, 100).is_empty(),
            "an effigy worshipper must not be tracked as a fight"
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
    fn weapon_buckets_aggregate_by_weapon_across_loadouts() {
        // The same weapon fired with two different armors must read as one row;
        // the enchant that was on the weapon rides with it.
        let mut buckets: HashMap<([i32; 4], [Vec<u16>; 4]), (i64, u64)> = HashMap::new();
        buckets.insert(
            ([100, 1, 2, 3], [vec![7], vec![], vec![], vec![]]),
            (500, 5),
        );
        buckets.insert(
            ([100, 1, 9, 3], [vec![7], vec![], vec![], vec![]]),
            (300, 3),
        );
        buckets.insert(([200, 1, 2, 3], [vec![], vec![], vec![], vec![]]), (100, 2));
        let weapons = aggregate_weapons(&buckets);
        assert_eq!(weapons.len(), 2, "one row per weapon, not per loadout");
        assert_eq!(weapons[0].weapon_id, 100);
        assert_eq!(weapons[0].damage, 800);
        assert_eq!(weapons[0].hits, 8);
        assert_eq!(weapons[0].enchants, vec![7]);
        assert_eq!(weapons[1].weapon_id, 200);
        assert_eq!(weapons[1].damage, 100);
    }

    #[test]
    fn a_shot_is_credited_to_the_weapon_that_fired_it_across_a_swap() {
        // Reported: a T7 loot-bonus staff swapped in for the last moments of a
        // fight took credit for shots fired with the T13 main staff that were still
        // in flight, making it read as the main damage weapon. Damage is bucketed
        // by the shot's own gear, so a swap only counts the shots fired with it.
        const MAIN: i32 = 2513; // Staff of the Vital Unity (T13)
        const LOOT: i32 = 2718; // Staff of Destruction (T7)
        let mut t = CombatTracker::new();
        t.on_map_change("Mad Lab", 42, 0);
        t.on_player_loaded(1000, 42);
        t.on_object_status(
            1000,
            &status(
                1000,
                vec![
                    name_stat("Lovens"),
                    stat(StatType::Attack, 65),
                    stat(StatType::Inventory0, MAIN),
                ],
            ),
            10,
        );
        t.on_object_spawn(
            500,
            0x10E4,
            &status(
                500,
                vec![stat(StatType::MaxHP, 300_000), stat(StatType::HP, 300_000)],
            ),
            10,
        );

        // A shot fired with the main staff is still in flight ...
        t.pending_shots.insert(
            7,
            PendingShot {
                shot_gear: ([MAIN, -1, -1, -1], Default::default()),
                ..Default::default()
            },
        );
        // ... while the loot swapout is equipped, which is the gear seen when the
        // hit below is processed.
        t.on_object_status(
            1000,
            &status(1000, vec![stat(StatType::Inventory0, LOOT)]),
            11,
        );
        t.on_local_hit(500, 7, 1000, 1000, 12);

        t.on_object_status(500, &status(500, vec![stat(StatType::HP, 0)]), 20);
        let done = t.on_tick(20);
        let local = done[0]
            .participants
            .iter()
            .find(|p| p.is_local)
            .expect("local participant retained");
        assert_eq!(local.weapons.len(), 1, "one weapon row");
        assert_eq!(
            local.weapons[0].weapon_id, MAIN,
            "the in-flight shot must stay credited to the weapon that fired it"
        );
        assert_eq!(local.weapons[0].hits, 1);
    }

    #[test]
    fn a_local_attack_change_is_logged_and_sourced_to_a_local_use() {
        let mut t = CombatTracker::new();
        t.on_map_change("Sprite World", 1, 0);
        t.on_player_loaded(1000, 1);
        t.on_object_status(1000, &status(1000, vec![stat(StatType::Attack, 65)]), 10);
        // A consumable use immediately before the jump, so the change is sourced
        // to it (the item name may be unresolvable in tests, so only the inferred
        // marker is asserted).
        t.on_use_item(0x0B50, 0.0, 0.0, 15);
        t.on_object_status(1000, &status(1000, vec![stat(StatType::Attack, 80)]), 20);

        let changes: Vec<(&str, i32, i32)> = t
            .local_stat_changes
            .iter()
            .map(|c| (c.label, c.from, c.to))
            .collect();
        assert!(changes.contains(&("Attack", 65, 80)), "got {changes:?}");
        let boost = t
            .local_stat_changes
            .iter()
            .find(|c| c.label == "Attack" && c.to == 80)
            .unwrap();
        // The use is only *proposed* as the source: it stands when the item's own
        // XML declares the stat, and otherwise the change falls back to the ally
        // label. Either way the log must never blame an item that cannot cause it.
        assert!(
            boost.source == "Ally buff" || boost.source.contains("Potion"),
            "unexpected source for a local consumable use: {}",
            boost.source
        );

        // The window closes at the fight's end, since the value never reverted.
        let windows = t.boost_windows_for(0, 100);
        let attack = windows
            .iter()
            .find(|w| w.stat == "Attack" && w.to == 80)
            .expect("the boost is a window");
        assert_eq!(attack.start_offset_ms, 20);
        assert_eq!(attack.end_offset_ms, 100);
    }

    #[test]
    fn the_secondary_condition_mask_does_not_read_as_a_classic_condition() {
        // Condition stat 69 is a *different* bit layout (`NEWCON_*` in `damage`), so
        // one of its bits must never be tested against the classic bits: a new-mask
        // bit 0x1000 used to surface as a phantom "Invisible" on a Wizard who has no
        // way to become invisible.
        let mut t = CombatTracker::new();
        t.on_map_change("Sprite World", 1, 0);
        t.on_player_loaded(1000, 1);
        // The object must exist first: a change is only diffed against the previous
        // snapshot, so the very update that creates it logs nothing.
        t.on_object_status(1000, &status(1000, vec![stat(StatType::Attack, 65)]), 5);
        t.on_object_status(
            1000,
            &status(1000, vec![stat(StatType::ConditionNew, 0x1000)]),
            10,
        );
        assert!(
            !t.local_stat_changes.iter().any(|c| c.label == "Invisible"),
            "a secondary-mask bit must not read as a classic condition: {:?}",
            t.local_stat_changes
                .iter()
                .map(|c| c.label)
                .collect::<Vec<_>>()
        );
        // The primary mask still reports it, which is what a rogue's cloak sets.
        t.on_object_status(
            1000,
            &status(1000, vec![stat(StatType::Condition, 0x1000)]),
            20,
        );
        assert!(t.local_stat_changes.iter().any(|c| c.label == "Invisible"));
    }

    #[test]
    fn weak_is_reported_as_a_monster_debuff_and_other_rises_as_an_ally_buff() {
        // Nothing local explains either: Weak is applied by monsters, a Damaging
        // burst by an ally's aura, so the two must not share one label.
        let mut t = CombatTracker::new();
        t.on_map_change("Sprite World", 1, 0);
        t.on_player_loaded(1000, 1);
        // Establish the object first: the update that creates it is not diffed.
        t.on_object_status(1000, &status(1000, vec![stat(StatType::Attack, 65)]), 5);
        t.on_object_status(
            1000,
            &status(1000, vec![stat(StatType::Condition, 0x4 | 0x0004_0000)]),
            10,
        );
        let source = |label: &str| {
            t.local_stat_changes
                .iter()
                .find(|c| c.label == label && c.to == 1)
                .map(|c| c.source.clone())
                .unwrap_or_default()
        };
        assert_eq!(source("Weak"), "Enemy debuff");
        assert_eq!(source("Damaging"), "Ally buff");
    }

    #[test]
    fn ability_activation_is_credited_only_packet_borne_damage() {
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
        // The local player with that ability in their ability slot.
        t.on_object_status(
            1000,
            &status(
                1000,
                vec![name_stat("You"), stat(StatType::Inventory1, 999)],
            ),
            10,
        );

        t.on_use_item(999, 0.0, 0.0, 100);
        // Damage the server itself attributed to the local player afterwards.
        t.on_damage(500, 1000, 700, 120);
        // A second activation with no packet-borne damage in its window.
        t.on_use_item(999, 0.0, 0.0, 400);

        let acts = t.ability_activations_for(0, 1_000, FightDestination::Boss(500));
        assert_eq!(acts.len(), 2);
        assert_eq!(acts[0].item_id, 999);
        assert_eq!(acts[0].start_offset_ms, 100);
        assert_eq!(
            (acts[0].damage, acts[0].hits),
            (0, 0),
            "an item that declares no server-applied effect is never credited a \
             bulletless packet, however close the packet lands"
        );
        assert_eq!(
            (acts[1].damage, acts[1].hits),
            (0, 0),
            "an activation with no packet damage stays at zero, never borrowed"
        );
        // The packet is still the fight's own packet-borne figure.
        let p = &t.fights.get(&500).unwrap().attackers[&1000];
        assert_eq!((p.packet_damage, p.packet_hits), (700, 1));
    }

    #[test]
    fn ability_activations_ignore_packets_that_belong_to_another_fight() {
        // Two bosses were damaged inside one activation window. The packet log is
        // session-wide, so without a destination filter each fight's activation
        // would be credited both packets (100 + 200).
        let mut t = CombatTracker::new();
        t.local_ability_uses
            .push_back(ability_use(100, 999, Some(5_000)));
        for (destination, damage) in [
            (FightDestination::Boss(500), 100),
            (FightDestination::Boss(501), 200),
        ] {
            t.local_damage_events.push_back(LocalDamageEvent {
                time_ms: 200,
                damage,
                destination,
                pet_id: None,
                summon: None,
                removal: None,
            });
            t.local_volley_hits.push_back(LocalVolleyHit {
                time_ms: 200,
                item_id: 999,
                damage,
                destination,
            });
        }

        let a = t.ability_activations_for(0, 1_000, FightDestination::Boss(500));
        let b = t.ability_activations_for(0, 1_000, FightDestination::Boss(501));
        assert_eq!((a[0].damage, a[0].hits), (200, 2));
        assert_eq!((b[0].damage, b[0].hits), (400, 2));
    }

    #[test]
    fn volley_hit_is_credited_to_the_ability_that_fired_it() {
        // A spell's volley is fired by the server, and the ServerPlayerShoot
        // packet names the item that fired it -- so its landed damage belongs to
        // that ability's row, never to the weapon the player happened to hold.
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
        t.on_object_spawn(600, 0x0321, &status(600, vec![name_stat("Bob")]), 0);
        t.on_object_status(
            1000,
            &status(
                1000,
                vec![name_stat("You"), stat(StatType::Inventory1, 777)],
            ),
            10,
        );
        t.on_use_item(777, 0.0, 0.0, 100);
        // The volley's per-bullet pre-defense damage, as on_ally_shot registers it
        // for the local player's own server-fired bullets.
        t.pending_server_shots.insert(5, (300, false, 777));
        t.on_local_hit(500, 5, 1000, 1000, 150);

        let acts = t.ability_activations_for(0, 1_000, FightDestination::Boss(500));
        assert_eq!(acts.len(), 1);
        assert_eq!(
            (acts[0].damage, acts[0].hits),
            (280, 1),
            "300 pre-defense, the target's 20 defense applied"
        );
        let p = &t.fights.get(&500).unwrap().attackers[&1000];
        assert_eq!(p.damage, 280, "the hit still counts in the fight total");
        assert!(
            p.damage_by_equipment.is_empty(),
            "a spell volley is not the weapon's self-computed damage"
        );
    }

    #[test]
    fn a_spray_nova_is_credited_by_its_declared_effect_window() {
        // The mushroom a spray leaves behind pulses its nova for the effect's own
        // declared life (4 waves 1.2s apart), so its packets stay with the spray
        // even after the player has cast something else -- and a spell that
        // declares no such effect never claims them.
        use crate::assets::get_asset_manager;
        if let Some(dir) = crate::assets::find_assets_dir() {
            get_asset_manager().set_assets_dir(&dir);
        }
        let am = get_asset_manager();
        if !am.try_load() || am.server_applied_damage_window_ms(0x04b3).is_none() {
            eprintln!("skipping: game assets not available");
            return;
        }
        assert!(
            am.server_applied_damage_window_ms(0x9443).is_none(),
            "a BulletNova spell declares no server-applied effect of its own"
        );

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
                    stat(StatType::Defense, 0),
                ],
            ),
            0,
        );
        t.on_object_spawn(600, 0x0321, &status(600, vec![name_stat("Bob")]), 0);
        // The ability slot holds the spray, so its uses register as activations.
        t.on_object_status(
            1000,
            &status(
                1000,
                vec![name_stat("You"), stat(StatType::Inventory1, 0x04b3)],
            ),
            10,
        );
        t.on_use_item(0x04b3, 0.0, 0.0, 1_000);
        // Nova pulses 0.28s and 2.5s later (inside the mushroom's life)...
        t.on_damage(500, 1000, 1_088, 1_280);
        t.on_damage(500, 1000, 1_088, 3_500);
        // ...and a third long after it expired, which no activation may claim.
        t.on_damage(500, 1000, 1_088, 30_000);

        let acts = t.ability_activations_for(0, 31_000, FightDestination::Boss(500));
        assert_eq!(acts.len(), 1);
        assert_eq!(
            (acts[0].damage, acts[0].hits),
            (2_176, 2),
            "the mushroom's waves stay with the spray that spawned it"
        );
    }

    #[test]
    fn beer_god_is_curated_as_its_own_fight() {
        // "New Beer God" (0x5670) is defined only in encounters.xml, which the
        // object catalog does not index: it has no resolvable name and no
        // <Labels>, so nothing but the 10k-HP fallback ever classified it -- and a
        // real capture showed the fight being dropped entirely. It is curated
        // explicitly instead, name included.
        let mut t = CombatTracker::new();
        t.on_map_change("Realm", 12345, 1_000);
        t.on_player_loaded(1000, 2537);
        // MaxHP deliberately omitted: the classification must not depend on it.
        t.on_object_spawn(
            500,
            22128,
            &status(500, vec![stat(StatType::HP, 80_000)]),
            1_100,
        );
        t.on_damage(500, 1000, 1_000, 2_000);
        t.on_object_status(500, &status(500, vec![stat(StatType::HP, 0)]), 20_000);

        let done = t.on_tick(20_000);
        assert_eq!(done.len(), 1, "the Beer God fight must be recorded");
        assert_eq!(done[0].boss_object_type, 22128);
        assert_eq!(done[0].boss_name, "Beer God");
        assert!(done[0].killed);
        assert_eq!(done[0].total_damage(), 1_000);
    }

    #[test]
    fn summon_hits_are_split_from_the_owners_own_hits() {
        // A pet-only fight. The owner's row keeps the damage (the game attributes
        // pet damage to the player) but the pet gets its own line, and nothing is
        // bucketed as the owner's weapon damage.
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

        // The pet (object 200) fires with the local player as its summoner.
        t.on_ally_shot(200, 1000, 5, 1, 300, 0, 0);
        t.on_local_hit(500, 5, 200, 1000, 10);

        let accum = t.fights.get(&500).unwrap().attackers.get(&1000).unwrap();
        assert_eq!(accum.summon_damage, 300);
        assert_eq!(accum.summon_hits, 1);
        assert!(
            accum.damage_by_equipment.is_empty(),
            "pet damage must not be bucketed as the owner's weapon damage"
        );

        t.on_object_status(500, &status(500, vec![stat(StatType::HP, 0)]), 20);
        let done = t.on_tick(20);
        let p = &done[0].participants[0];
        assert_eq!(p.damage, 300);
        assert_eq!(p.hits, 1);
        assert_eq!(p.summon_damage, 300);
        assert_eq!(p.summon_hits, 1);
        assert_eq!(p.summons.len(), 1);
        assert_eq!(p.summons[0].object_id, 200);
        assert_eq!(p.summons[0].damage, 300);
        assert_eq!(p.summons[0].hits, 1);
        // The owner's own hits are all accounted for, so the row stays exact.
        assert_eq!(p.provenance, DamageProvenance::SelfComputed);
    }

    #[test]
    fn summon_rows_carry_the_entitys_own_object_type() {
        // A summoned creature's object type is its identity: it names the row and
        // gives it a sprite ("Summon Pocket Rat" is 0x4683), where a pet is named
        // by its skin under `pet_type`. The type comes from the spawned entity, so
        // the row is right even before its first hit resolves.
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
        // Three summoned Pocket Rats, as the Pocket Rat consumable spawns them.
        for id in [157, 158, 159] {
            t.on_object_spawn(id, 0x4683, &status(id, vec![]), 0);
        }

        t.on_ally_shot(157, 1000, 5, 1, 50, 0x4683, 0);
        t.on_local_hit(500, 5, 157, 1000, 10);
        t.on_ally_shot(158, 1000, 6, 1, 50, 0x4683, 0);
        t.on_local_hit(500, 6, 158, 1000, 11);
        // A summon that was never seen spawning keeps a zero type, so the UI can
        // still fall back to the generic label.
        t.on_ally_shot(999, 1000, 7, 1, 50, 0x4683, 0);
        t.on_local_hit(500, 7, 999, 1000, 12);

        t.on_object_status(500, &status(500, vec![stat(StatType::HP, 0)]), 20);
        let done = t.on_tick(20);
        let p = &done[0].participants[0];
        assert_eq!(p.summons.len(), 3);
        for s in &p.summons {
            if s.object_id == 999 {
                assert_eq!(s.object_type, 0, "an unseen entity has no type to show");
                assert!(!s.is_pet);
                continue;
            }
            assert_eq!(
                s.object_type, 0x4683,
                "object {} has the rat's type",
                s.object_id
            );
            assert!(!s.is_pet);
            assert_eq!(s.pet_type, 0, "a summon is not named by a pet skin");
            assert_eq!(s.damage, 50);
            assert_eq!(s.hits, 1);
        }
    }

    #[test]
    fn summon_pet_type_still_wins_for_pets() {
        // A real pet is named by its skin: the type it carries is a pet skin id,
        // which is what the UI draws, not the object type it was spawned as.
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
        t.on_object_spawn(600, 0x0321, &status(600, vec![name_stat("Bob")]), 0);
        t.on_object_spawn(
            200,
            0x0321,
            &status(200, vec![stat(StatType::PetType, 12_345), name_stat("Rat")]),
            0,
        );
        t.on_ally_shot(200, 1000, 5, 1, 300, 0, 0);
        t.on_local_hit(500, 5, 200, 1000, 10);
        t.on_object_status(500, &status(500, vec![stat(StatType::HP, 0)]), 20);
        let done = t.on_tick(20);
        let s = &done[0].participants[0].summons[0];
        assert_eq!(s.pet_type, 12_345);
        assert!(s.is_pet);
        assert_eq!(
            s.object_type, 0x0321,
            "the spawned type is kept alongside the skin"
        );
    }

    #[test]
    fn missed_summon_shot_is_valued_at_the_summon_rate() {
        // A pet hit whose shoot packet was missed must not be valued at the
        // owner's (much larger) average: it falls back to the summon's own
        // average, so a pet-heavy fight cannot inflate the owner's figure.
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

        // Owner's own shot: 1000 pre-defense. Resolving a real server shot needs the
        // asset set, so register the pre-defense value directly, as the other local
        // shot tests do.
        t.on_ally_shot(1000, 0, 1, 1, 1000, 0, 0);
        t.pending_server_shots.insert(1, (1000, false, 0));
        t.on_local_hit(500, 1, 1000, 1000, 10);
        // Pet: one shot with its packet (100) and one hit whose shot was missed.
        t.on_ally_shot(200, 1000, 5, 1, 100, 0, 0);
        t.on_local_hit(500, 5, 200, 1000, 11);
        t.on_local_hit(500, 9, 200, 1000, 12);

        t.on_object_status(500, &status(500, vec![stat(StatType::HP, 0)]), 20);
        let done = t.on_tick(20);
        let p = &done[0].participants[0];
        assert_eq!(p.summon_hits, 2);
        assert_eq!(p.summon_damage, 100);
        // 1000 (own) + 100 (resolved pet) + 100 (missed pet hit at the pet's rate),
        // never + 1000.
        assert_eq!(p.damage, 1200);
        assert_eq!(p.provenance, DamageProvenance::SelfEstimated);
    }

    #[test]
    fn boss_uptime_counts_only_vulnerable_time() {
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
                    stat(StatType::Defense, 0),
                    stat(StatType::Condition, 0x0100_0000),
                ],
            ),
            0,
        );
        // Damage while invulnerable, then the boss opens up and takes a hit.
        t.on_damage(500, 600, 100, 1_000);
        t.on_object_status(500, &status(500, vec![stat(StatType::Condition, 0)]), 2_000);
        t.on_damage(500, 600, 100, 3_000);

        let fight = t.fights.get(&500).expect("fight tracked");
        // 1000 -> 2000 was invulnerable (nothing counted); 2000 -> 3000 counts.
        assert_eq!(fight.boss_uptime_ms, 1_000);
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
        t.pending_server_shots.insert(5, (300, false, 0));
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
        t.pending_server_shots.insert(5, (300, true, 0)); // armor-piercing
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
        t.pending_server_shots.insert(5, (300, false, 0));
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
        t.pending_server_shots.insert(5, (300, false, 0));
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
        t.pending_server_shots.insert(5, (300, false, 0));
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
        t.pending_server_shots.insert(5, (300, false, 0));
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
                    stat(StatType::Vitality, 55),
                    stat(StatType::VitalityBoost, 20),
                    stat(StatType::Speed, 45),
                    stat(StatType::SpeedBoost, 10),
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
        // VIT- and SPD-scaled gear (Cloak of the Darkened Sun, Sharktooth Sigil)
        // must resolve too; returning `None` for them silently dropped the whole
        // stat-scaled proc.
        assert_eq!(
            local.effective_scaling_stat(ScalingStat::Vitality),
            Some(55)
        );
        assert_eq!(local.effective_scaling_stat(ScalingStat::Speed), Some(45));
    }

    #[test]
    fn vitality_and_speed_changes_are_recorded_as_boosts() {
        // A VIT-stacking build's stat swings must reach the fight's boost
        // timeline, not just the damage math: only ATT/DEF/DEX/WIS/MP used to be
        // snapshotted, so a VIT potion or VIT-buffing ability was invisible.
        let mut t = CombatTracker::new();
        t.on_map_change("Mad Lab", 42, 0);
        t.on_player_loaded(1000, 42);
        t.on_object_spawn(1000, 0x0321, &status(1000, vec![name_stat("You")]), 0);
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
        // Pre-fight stats (their arrival precedes the fight, so it is not a boost).
        t.on_object_status(
            1000,
            &status(
                1000,
                vec![
                    stat(StatType::Vitality, 75),
                    stat(StatType::Speed, 50),
                    stat(StatType::MaxHP, 800),
                ],
            ),
            5,
        );
        // Open the fight.
        t.pending_server_shots.insert(1, (100, false, 0));
        t.on_local_hit(500, 1, 1000, 1000, 10);
        // A VIT potion / VIT-buffing ability lands mid-fight; SPD does not move,
        // while a life treat raises Max HP.
        t.on_object_status(
            1000,
            &status(
                1000,
                vec![
                    stat(StatType::Vitality, 100),
                    stat(StatType::Speed, 50),
                    stat(StatType::MaxHP, 820),
                ],
            ),
            20,
        );
        let done = t.on_map_change("Nexus", 0, 40);
        let vit: Vec<_> = done[0]
            .boosts
            .iter()
            .filter(|b| b.stat == "Vitality")
            .collect();
        assert_eq!(vit.len(), 1, "exactly one VIT boost window");
        assert_eq!((vit[0].from, vit[0].to), (75, 100));
        assert!(
            done[0].boosts.iter().all(|b| b.stat != "Speed"),
            "an unchanged SPD must not open a window"
        );
        // Max HP is a buffed stat too (life treats raise it), so it is tracked.
        let hp: Vec<_> = done[0]
            .boosts
            .iter()
            .filter(|b| b.stat == "Max HP")
            .collect();
        assert_eq!(hp.len(), 1, "exactly one Max HP boost window");
        assert_eq!((hp[0].from, hp[0].to), (800, 820));
    }

    #[test]
    fn unobserved_vitality_does_not_apply_a_vitality_scaled_bonus() {
        // Before the VIT stat arrives, a VIT-scaled proc must not be computed
        // from the zero default (which would under-credit it silently).
        use crate::assets::ScalingStat;
        let mut t = CombatTracker::new();
        t.on_player_loaded(1000, 1);
        t.on_object_spawn(1000, 0x0321, &status(1000, vec![name_stat("You")]), 0);
        let local = t.objects.get(&1000).expect("local");
        assert_eq!(local.effective_scaling_stat(ScalingStat::Vitality), None);
        assert_eq!(local.effective_scaling_stat(ScalingStat::Speed), None);
    }

    #[test]
    fn vitality_scaled_lethal_strike_bonus_is_credited() {
        // Regression for the Cloak of the Darkened Sun: its Lethal Strike scales
        // off Vitality, so the proc used to resolve to no bonus at all (the stat
        // was never tracked). Params mirror the parsed asset values.
        use crate::assets::ScalingStat;
        let params = LethalStrikeParams {
            ignore_flat: 60.0,
            ignore_perc: 0.45,
            stat_mod_flat: 3.0,
            stat_mod_perc: 0.01,
            stat_mod_scaling_min: 24.0,
            scaling_stat: ScalingStat::Vitality,
            duration: 2.4,
            extra_shots: 2,
        };
        let mut t = CombatTracker::new();
        t.on_map_change("Mad Lab", 42, 0);
        t.on_player_loaded(1000, 42);
        t.on_object_spawn(1000, 0x0321, &status(1000, vec![name_stat("You")]), 0);
        t.on_object_status(
            1000,
            &status(
                1000,
                vec![
                    stat(StatType::Vitality, 130),
                    // The server folds the boost into the base stat; adding it
                    // again would inflate the bonus.
                    stat(StatType::VitalityBoost, 20),
                ],
            ),
            10,
        );
        t.on_object_spawn(
            500,
            0x10E4,
            &status(
                500,
                vec![
                    stat(StatType::MaxHP, 500_000),
                    stat(StatType::HP, 500_000),
                    stat(StatType::Defense, 65),
                ],
            ),
            0,
        );

        let vit = t
            .objects
            .get(&1000)
            .expect("local")
            .effective_scaling_stat(ScalingStat::Vitality)
            .expect("VIT observed");
        assert_eq!(vit, 130);

        // w = 130 - 24 = 106 -> flat = 60 + 3*106 = 378, perc = 0.45 + 1.06 = 1.51
        // bonus vs 65 DEF = 378 + 1.51*65 = 476.15 -> 476 (two side procs included)
        let bonus = damage::lethal_strike_bonus(&params, vit, 65);
        assert_eq!(bonus, 476);

        t.lethal_strike_windows.push_back((0, 10_000));
        t.pending_shots.insert(
            10,
            PendingShot {
                base_damage: 1000,
                shot_time: 100,
                lethal_strike: Some((params, vit)),
                extra_projectile: false,
                ..Default::default()
            },
        );
        t.on_local_hit(500, 10, 1000, 1000, 100);
        let done = t.on_map_change("Nexus", 0, 200);
        let local = done[0]
            .participants
            .iter()
            .find(|p| p.is_local)
            .expect("local");
        // weapon 1000 - 65 DEF = 935, plus the main bonus and both side procs.
        assert_eq!(local.damage, 935 + 3 * 476);
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
        t.pending_server_shots.insert(5, (300, false, 0));
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
        t.pending_server_shots.insert(5, (300, false, 0));
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
        t.pending_server_shots.insert(5, (300, false, 0));
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
        t.pending_server_shots.insert(5, (300, false, 0));
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
    fn mv_boss_damage_is_not_capped_to_nominal_hp() {
        // Moonlight Village mechanics bosses floor invulnerable -- their nominal
        // max HP is not the damage they absorbed -- so the party's raw damage must
        // survive finalize untouched instead of being trimmed to the boss HP.
        let mut t = CombatTracker::new();
        t.on_map_change("Moonlight Village", 42, 0);
        t.on_player_loaded(1000, 42);
        t.on_object_spawn(600, 0x0400, &status(600, vec![name_stat("Bob")]), 0);
        t.on_object_spawn(
            500,
            20450,
            &status(
                500,
                vec![stat(StatType::MaxHP, 180_000), stat(StatType::HP, 180_000)],
            ),
            100,
        );
        // One other player plus the local player, together far above the nominal
        // 180000 HP pool.
        t.on_damage(500, 600, 250_000, 110);
        t.pending_shots.insert(
            10,
            PendingShot {
                base_damage: 200_000,
                armor_piercing: false,
                ..Default::default()
            },
        );
        t.on_local_hit(500, 10, 1000, 1000, 120);
        // The dance concludes: the dancer is scored completed.
        let done = t.on_boss_text(500, None, "This concludes the Moonlight Dance.", 400);
        assert_eq!(done.len(), 1);
        assert!(done[0].killed);
        assert_eq!(
            done[0].total_damage(),
            450_000,
            "raw damage kept, not capped to the nominal 180000 HP"
        );
    }

    #[test]
    fn mv_spirits_count_once_per_object_and_credit_the_active_fight() {
        // Each spirit is one `MV Total Counter` object released at a phase end.
        // The server re-adds the same object as it re-enters view, so the tally
        // must count each id once and credit it to the live dancer fight.
        let mut t = CombatTracker::new();
        t.on_map_change("Moonlight Village", 42, 0);
        t.on_player_loaded(1000, 42);
        t.on_object_spawn(600, 0x0400, &status(600, vec![name_stat("Bob")]), 0);
        t.on_object_spawn(
            500,
            20450,
            &status(
                500,
                vec![stat(StatType::MaxHP, 180_000), stat(StatType::HP, 180_000)],
            ),
            100,
        );
        t.on_damage(500, 600, 10_000, 110);
        t.pending_shots.insert(
            10,
            PendingShot {
                base_damage: 1000,
                armor_piercing: false,
                ..Default::default()
            },
        );
        t.on_local_hit(500, 10, 1000, 1000, 120);
        // A phase-end burst of four spirits (pairs, up to 8 per phase).
        let spirit = |id: i32| status(id, vec![stat(StatType::Size, 105)]);
        for id in 700..704 {
            t.on_object_spawn(id, 0x5026, &spirit(id), 200);
        }
        // Re-adds of the same objects (re-entering view) must not double-count.
        for id in 700..704 {
            t.on_object_spawn(id, 0x5026, &spirit(id), 240);
        }
        // A later burst adds two more.
        for id in 710..712 {
            t.on_object_spawn(id, 0x5026, &spirit(id), 300);
        }
        let done = t.on_map_change("Nexus", 0, 500);
        let mv: Vec<_> = done
            .iter()
            .filter(|f| f.boss_object_type == 20450)
            .collect();
        assert_eq!(mv.len(), 1);
        assert_eq!(mv[0].spirits, 6, "four plus two, re-adds deduped");
    }

    #[test]
    fn mv_tutorial_spirits_do_not_count_before_a_boss_is_engaged() {
        // The dungeon opens with three boss-less lantern mini-phases. Any spirit
        // releases there (or any pre-fight noise) must not score; counting starts
        // only once a dancer/Umi fight has been engaged.
        let mut t = CombatTracker::new();
        t.on_map_change("Moonlight Village", 42, 0);
        t.on_player_loaded(1000, 42);
        let spirit = |id: i32| status(id, vec![stat(StatType::Size, 105)]);
        for id in 700..704 {
            t.on_object_spawn(id, 0x5026, &spirit(id), 50);
        }
        // Engage Sage Genji, then a phase-end burst counts.
        t.on_object_spawn(600, 0x0400, &status(600, vec![name_stat("Bob")]), 0);
        t.on_object_spawn(
            500,
            20450,
            &status(
                500,
                vec![stat(StatType::MaxHP, 180_000), stat(StatType::HP, 180_000)],
            ),
            100,
        );
        t.on_damage(500, 600, 10_000, 110);
        t.pending_shots.insert(
            10,
            PendingShot {
                base_damage: 1000,
                armor_piercing: false,
                ..Default::default()
            },
        );
        t.on_local_hit(500, 10, 1000, 1000, 120);
        for id in 710..714 {
            t.on_object_spawn(id, 0x5026, &spirit(id), 200);
        }
        let done = t.on_map_change("Nexus", 0, 500);
        let mv: Vec<_> = done
            .iter()
            .filter(|f| f.boss_object_type == 20450)
            .collect();
        assert_eq!(mv.len(), 1);
        assert_eq!(mv[0].spirits, 4, "only the post-engagement burst counts");
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
                damage_taken_hits: None,
                damage_blocked: None,
                guarded_damage: None,
                guarded_hits: None,
                summon_damage: 0,
                summon_hits: 0,
                summons: Vec::new(),
                weapons: Vec::new(),
                packet_damage: 0,
                packet_hits: 0,
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
                damage_taken_hits: None,
                damage_blocked: None,
                guarded_damage: None,
                guarded_hits: None,
                summon_damage: 0,
                summon_hits: 0,
                summons: Vec::new(),
                weapons: Vec::new(),
                packet_damage: 0,
                packet_hits: 0,
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
                damage_taken_hits: None,
                damage_blocked: None,
                guarded_damage: Some(9000),
                guarded_hits: Some(10),
                summon_damage: 0,
                summon_hits: 0,
                summons: Vec::new(),
                weapons: Vec::new(),
                packet_damage: 0,
                packet_hits: 0,
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
                damage_taken_hits: None,
                damage_blocked: None,
                guarded_damage: Some(0),
                guarded_hits: Some(0),
                summon_damage: 0,
                summon_hits: 0,
                summons: Vec::new(),
                weapons: Vec::new(),
                packet_damage: 0,
                packet_hits: 0,
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
                damage_taken_hits: None,
                damage_blocked: None,
                guarded_damage: None,
                guarded_hits: None,
                summon_damage: 0,
                summon_hits: 0,
                summons: Vec::new(),
                weapons: Vec::new(),
                packet_damage: 0,
                packet_hits: 0,
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
                damage_taken_hits: None,
                damage_blocked: None,
                guarded_damage: None,
                guarded_hits: None,
                summon_damage: 0,
                summon_hits: 0,
                summons: Vec::new(),
                weapons: Vec::new(),
                packet_damage: 0,
                packet_hits: 0,
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
                damage_taken_hits: None,
                damage_blocked: None,
                guarded_damage: None,
                guarded_hits: None,
                summon_damage: 0,
                summon_hits: 0,
                summons: Vec::new(),
                weapons: Vec::new(),
                packet_damage: 0,
                packet_hits: 0,
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
                damage_taken_hits: None,
                damage_blocked: None,
                guarded_damage: None,
                guarded_hits: None,
                summon_damage: 0,
                summon_hits: 0,
                summons: Vec::new(),
                weapons: Vec::new(),
                packet_damage: 0,
                packet_hits: 0,
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
                damage_taken_hits: None,
                damage_blocked: None,
                guarded_damage: None,
                guarded_hits: None,
                summon_damage: 0,
                summon_hits: 0,
                summons: Vec::new(),
                weapons: Vec::new(),
                packet_damage: 0,
                packet_hits: 0,
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
                damage_taken_hits: None,
                damage_blocked: None,
                guarded_damage: None,
                guarded_hits: None,
                summon_damage: 0,
                summon_hits: 0,
                summons: Vec::new(),
                weapons: Vec::new(),
                packet_damage: 0,
                packet_hits: 0,
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
                damage_taken_hits: None,
                damage_blocked: None,
                guarded_damage: None,
                guarded_hits: None,
                summon_damage: 0,
                summon_hits: 0,
                summons: Vec::new(),
                weapons: Vec::new(),
                packet_damage: 0,
                packet_hits: 0,
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
                damage_taken_hits: None,
                damage_blocked: None,
                guarded_damage: None,
                guarded_hits: None,
                summon_damage: 0,
                summon_hits: 0,
                summons: Vec::new(),
                weapons: Vec::new(),
                packet_damage: 0,
                packet_hits: 0,
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
                damage_taken_hits: None,
                damage_blocked: None,
                guarded_damage: None,
                guarded_hits: None,
                summon_damage: 0,
                summon_hits: 0,
                summons: Vec::new(),
                weapons: Vec::new(),
                packet_damage: 0,
                packet_hits: 0,
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
                damage_taken_hits: None,
                damage_blocked: None,
                guarded_damage: None,
                guarded_hits: None,
                summon_damage: 0,
                summon_hits: 0,
                summons: Vec::new(),
                weapons: Vec::new(),
                packet_damage: 0,
                packet_hits: 0,
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
    fn hits_taken_is_recorded_for_every_participant_including_zero() {
        // Every participant's row carries a hit count, a zero included, so the UI
        // can tell "never hit" apart from a fight recorded before the field
        // existed (a NULL row).
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
        t.on_object_spawn(601, 0x0321, &status(601, vec![name_stat("Bob")]), 0);

        // Local engages the boss; Alice and Bob both deal damage, only Alice is hit.
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
        t.on_damage(500, 601, 4000, 21);
        t.on_damage(600, 500, 300, 30);
        t.on_damage(600, 500, 200, 35);

        t.on_object_status(500, &status(500, vec![stat(StatType::HP, 0)]), 40);
        let done = t.on_tick(40);
        let hits_taken = |name: &str| {
            done[0]
                .participants
                .iter()
                .find(|p| p.name == name)
                .unwrap_or_else(|| panic!("{name} recorded"))
                .damage_taken_hits
        };
        assert_eq!(hits_taken("Alice"), Some(2), "both incoming hits counted");
        assert_eq!(
            hits_taken("Bob"),
            Some(0),
            "a player who was never hit records a zero, not None"
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
    fn mv_dancer_without_completion_stays_escaped() {
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
        // The completion dropper is present but emits no loot: the run escaped.
        t.on_object_spawn(
            15598,
            20658,
            &status(
                15598,
                vec![stat(StatType::MaxHP, 100), stat(StatType::HP, 100)],
            ),
            350,
        );
        let done = t.on_map_change("Nexus", 0, 500);
        assert_eq!(done.len(), 1);
        assert!(
            !done[0].killed,
            "a dancer cleared with no loot recorded is Escaped"
        );
    }

    // A real phase end: the dancer's HP reaches 0 and its object leaves view, but
    // the run was never cleared (no concluding line, no loot). The generic HP
    // heuristic must not score the dancer, or every run whose dancer phase ends
    // would be recorded as Completed.
    #[test]
    fn mv_dancer_phase_end_without_completion_stays_escaped() {
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
        // The phase ends: HP hits 0 and the dancer leaves view unscored.
        t.on_object_status(500, &status(500, vec![stat(StatType::HP, 0)]), 300);
        assert!(
            t.on_object_removed(500, 400).is_none(),
            "a dancer is suspended, never finalized on removal"
        );
        // The map changes with no completion signal: the run is Escaped.
        let done = t.on_map_change("Nexus", 0, 500);
        assert_eq!(done.len(), 1);
        assert!(
            !done[0].killed,
            "an uncleared dancer phase must not complete the run"
        );
    }

    // The dancers' concluding line scores all three as Completed even though they
    // go invulnerable rather than dying.
    #[test]
    fn mv_dance_concluded_text_completes_dancer() {
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
        let done = t.on_boss_text(500, None, "This concludes the Moonlight Dance.", 400);
        assert_eq!(done.len(), 1);
        assert!(
            done[0].killed,
            "dancer completed via the dance-concluded line"
        );
    }

    // A recorded loot bag is the other completion signal: the droppers emit the
    // run's loot when it is cleared. Kitsune Umi's loot clears only Umi.
    #[test]
    fn mv_loot_completes_umi_only() {
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
        // Her loot lands (resolved to Umi, or her dropper's own type).
        let done = t.on_instance_loot(20493, 42, 400);
        assert_eq!(done.len(), 1);
        assert!(done[0].killed, "Umi completed by her loot");
        assert_eq!(done[0].boss_object_type, 20493);
    }

    // The dance concluded line completes the dancers only: Umi's optional fight
    // is scored by her own loot, never by the dancers' clear.
    #[test]
    fn mv_dance_text_does_not_complete_umi() {
        let mut t = CombatTracker::new();
        t.on_map_change("Moonlight Village", 42, 0);
        t.on_player_loaded(1000, 42);
        let hp = || vec![stat(StatType::MaxHP, 360_000), stat(StatType::HP, 360_000)];
        t.on_object_spawn(500, 20450, &status(500, hp()), 100);
        t.on_object_spawn(254, 20493, &status(254, hp()), 110);
        t.pending_shots.insert(
            10,
            PendingShot {
                base_damage: 1000,
                armor_piercing: false,
                ..Default::default()
            },
        );
        t.on_local_hit(500, 10, 1000, 1000, 200);
        t.on_local_hit(254, 10, 1000, 1000, 210);
        t.on_object_status(500, &status(500, vec![stat(StatType::HP, 1)]), 300);
        assert_eq!(
            t.on_boss_text(500, None, "This concludes the Moonlight Dance.", 400)
                .len(),
            1
        );
        let done = t.on_map_change("Nexus", 0, 500);
        let umi: Vec<_> = done
            .iter()
            .filter(|f| f.boss_object_type == 20493)
            .collect();
        assert_eq!(umi.len(), 1);
        assert!(!umi[0].killed, "Umi only clears on her own loot");
    }

    // Leisurely Mode (the group consumed a Tofu Delicacy before the dance) is
    // announced with a server notification and labels the run's fights.
    #[test]
    fn mv_leisurely_mode_labels_the_runs_fights() {
        let mut t = CombatTracker::new();
        t.on_map_change("Moonlight Village", 42, 0);
        t.on_player_loaded(1000, 42);
        t.on_mv_leisurely_mode();
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
        let done = t.on_boss_text(500, None, "This concludes the Moonlight Dance.", 300);
        assert_eq!(done.len(), 1);
        assert!(done[0].leisurely, "the clear is labelled Leisurely Mode");

        // The mode is per instance: the next run's fights are unlabelled.
        t.on_map_change("Moonlight Village", 43, 1000);
        t.on_object_spawn(
            600,
            20450,
            &status(
                600,
                vec![stat(StatType::MaxHP, 360_000), stat(StatType::HP, 360_000)],
            ),
            1100,
        );
        t.pending_shots.insert(
            11,
            PendingShot {
                base_damage: 1000,
                armor_piercing: false,
                ..Default::default()
            },
        );
        t.on_local_hit(600, 11, 1000, 1000, 1200);
        let next = t.on_boss_text(600, None, "This concludes the Moonlight Dance.", 1300);
        assert_eq!(next.len(), 1);
        assert!(
            !next[0].leisurely,
            "a later normal run is not labelled Leisurely Mode"
        );
    }

    // Challenge Mode (the local player whacked the Challenge Gate, which
    // pet-stasises them) labels the run's fights "Petless" on the card. The gate
    // itself is a curated non-boss, so it never becomes a fight of its own.
    #[test]
    fn mv_challenge_gate_hit_labels_the_runs_fights_petless() {
        let mut t = CombatTracker::new();
        t.on_map_change("Moonlight Village", 42, 0);
        t.on_player_loaded(1000, 42);
        t.on_object_spawn(
            700,
            crate::assets::MV_CHALLENGE_GATE_TYPE,
            &status(
                700,
                vec![stat(StatType::MaxHP, 500_000), stat(StatType::HP, 500_000)],
            ),
            50,
        );
        t.on_local_hit(700, 7, 1000, 1000, 60);
        assert!(t.fights.is_empty(), "the gate is never a fight of its own");
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
        let done = t.on_boss_text(500, None, "This concludes the Moonlight Dance.", 300);
        assert_eq!(done.len(), 1);
        assert!(done[0].petless, "the clear is labelled Petless");
        assert!(
            !done[0].leisurely,
            "Challenge Mode is independent of Leisure"
        );

        // The mode is per instance: the next run's fights are unlabelled.
        t.on_map_change("Moonlight Village", 43, 1000);
        t.on_object_spawn(
            600,
            20450,
            &status(
                600,
                vec![stat(StatType::MaxHP, 360_000), stat(StatType::HP, 360_000)],
            ),
            1100,
        );
        t.pending_shots.insert(
            11,
            PendingShot {
                base_damage: 1000,
                armor_piercing: false,
                ..Default::default()
            },
        );
        t.on_local_hit(600, 11, 1000, 1000, 1200);
        let next = t.on_boss_text(600, None, "This concludes the Moonlight Dance.", 1300);
        assert_eq!(next.len(), 1);
        assert!(!next[0].petless, "a later normal run is not Petless");
    }

    // Spectral Penitentiary: a mini-boss's objective-cleared taunt marks that
    // boss hard mode, and Soulwarden Murcian is hard mode only when both
    // mini-bosses were -- which is what makes the dungeon hard mode.
    #[test]
    fn spectral_mini_taunts_decide_murcian_hard_mode() {
        let zole = crate::assets::SPECTRAL_HM_TAUNTS[0];
        let lobotomik = crate::assets::SPECTRAL_HM_TAUNTS[1];

        // Only one mini-boss cleared its objectives: Murcian stays regular, so
        // the run is not hard mode even though a boss was.
        let mut t = CombatTracker::new();
        t.on_map_change("Spectral Penitentiary", 7, 0);
        t.on_player_loaded(1000, 7);
        t.on_boss_text(500, None, zole.1, 100);
        let zole_fight = finish_shatters_fight(&mut t, 500, 23659, 300_000, 150);
        assert!(zole_fight.spectral_hm, "Zole taunted, so Zole is HM");
        let murcian = finish_shatters_fight(&mut t, 600, 23681, 400_000, 200);
        assert!(
            !murcian.spectral_hm,
            "one hard-mode mini-boss is not enough for Murcian"
        );

        // Both mini-bosses taunted: Murcian is hard mode, so is the run. A
        // repeated taunt from the same boss must not count twice.
        let mut t = CombatTracker::new();
        t.on_map_change("Spectral Penitentiary", 7, 0);
        t.on_player_loaded(1000, 7);
        t.on_boss_text(500, None, zole.1, 100);
        t.on_boss_text(600, None, lobotomik.1, 110);
        t.on_boss_text(700, None, zole.1, 120);
        let zole_fight = finish_shatters_fight(&mut t, 500, 23659, 300_000, 150);
        assert!(zole_fight.spectral_hm);
        let lobo_fight = finish_shatters_fight(&mut t, 600, 23920, 300_000, 160);
        assert!(lobo_fight.spectral_hm);
        let murcian = finish_shatters_fight(&mut t, 700, 23681, 400_000, 200);
        assert!(murcian.spectral_hm, "both mini-bosses were hard mode");

        // The mode is per instance: the next run starts regular.
        t.on_map_change("Spectral Penitentiary", 8, 500);
        let next = finish_shatters_fight(&mut t, 800, 23681, 400_000, 600);
        assert!(!next.spectral_hm, "a later run is not hard mode");
    }

    // A regular (non-hard-mode) line is never mistaken for a taunt.
    #[test]
    fn spectral_hm_taunts_are_exact() {
        let zole = crate::assets::SPECTRAL_HM_TAUNTS[0];
        assert_eq!(crate::assets::spectral_hm_taunt_boss(zole.1), Some(zole.0));
        // Surrounding whitespace and case are tolerated.
        assert_eq!(
            crate::assets::spectral_hm_taunt_boss(&format!("  {}  ", zole.1.to_uppercase())),
            Some(zole.0)
        );
        // The line Zole opens the fight with is not the taunt.
        assert_eq!(
            crate::assets::spectral_hm_taunt_boss(
                "Pick cells that aren't already taken you imbeciles!"
            ),
            None
        );
    }

    // Only the local player's own hit counts: a teammate ringing the gate makes
    // *their* run petless, not ours.
    #[test]
    fn mv_challenge_gate_hit_by_someone_else_does_not_label_our_run() {
        let mut t = CombatTracker::new();
        t.on_map_change("Moonlight Village", 42, 0);
        t.on_player_loaded(1000, 42);
        t.on_object_spawn(
            700,
            crate::assets::MV_CHALLENGE_GATE_TYPE,
            &status(
                700,
                vec![stat(StatType::MaxHP, 500_000), stat(StatType::HP, 500_000)],
            ),
            50,
        );
        // A DamagePacket from another player, and an EnemyHit owned by them.
        t.on_damage(700, 2000, 5_000, 60);
        t.on_local_hit(700, 7, 2000, 2000, 61);
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
        let done = t.on_boss_text(500, None, "This concludes the Moonlight Dance.", 300);
        assert_eq!(done.len(), 1);
        assert!(!done[0].petless, "someone else's challenge is not our run");
    }

    // A completion in one dungeon instance does not carry to the next: a dancer
    // in a later Moonlight Village run starts fresh.
    #[test]
    fn mv_completion_flag_clears_on_map_change() {
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
        assert_eq!(
            t.on_boss_text(500, None, "This concludes the Moonlight Dance.", 300)
                .len(),
            1
        );
        // New run: a dancer here must not inherit the previous run's completion.
        t.on_map_change("Moonlight Village", 43, 1000);
        t.on_object_spawn(
            600,
            20450,
            &status(
                600,
                vec![stat(StatType::MaxHP, 360_000), stat(StatType::HP, 360_000)],
            ),
            1100,
        );
        t.pending_shots.insert(
            11,
            PendingShot {
                base_damage: 1000,
                armor_piercing: false,
                ..Default::default()
            },
        );
        t.on_local_hit(600, 11, 1000, 1000, 1200);
        t.on_object_status(600, &status(600, vec![stat(StatType::HP, 1)]), 1300);
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
        // The dance concludes while the fight is live.
        let first = t.on_boss_text(500, None, "This concludes the Moonlight Dance.", 400);
        assert_eq!(first.len(), 1, "one completed dancer");
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

    // A summoner run re-detects a dancer under many object ids before the dance
    // concludes. Every re-detection folds into one fight so the DB records a
    // single row, not one per id.
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
        // The dance concludes while the last detection is live.
        let done = t.on_boss_text(500, None, "This concludes the Moonlight Dance.", 3000);
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
        let done = t.on_boss_text(500, None, "This concludes the Moonlight Dance.", 40);
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

    /// Spawn, damage, engage and finish `object_type` under `object_id`.
    fn finish_shatters_fight(
        t: &mut CombatTracker,
        object_id: i32,
        object_type: i32,
        max_hp: i32,
        base: i64,
    ) -> CompletedFight {
        t.on_object_spawn(
            object_id,
            object_type,
            &status(
                object_id,
                vec![stat(StatType::MaxHP, max_hp), stat(StatType::HP, max_hp)],
            ),
            base,
        );
        t.on_damage(object_id, 600, (max_hp / 2) as i64, base + 10);
        t.on_local_hit(object_id, 7, 1000, 1000, base + 12);
        t.on_object_status(
            object_id,
            &status(object_id, vec![stat(StatType::HP, 0)]),
            base + 16,
        );
        t.on_object_removed(object_id, base + 20)
            .expect("fight finalized")
    }

    #[test]
    fn shatters_stone_idol_marks_the_bridge_sentinel_hard_mode() {
        let mut t = CombatTracker::new();
        t.on_map_change("The Shatters", 777, 0);
        t.on_player_loaded(1000, 777);
        t.on_object_spawn(600, 0x0400, &status(600, vec![name_stat("Bob")]), 0);

        // The Idol is invulnerable until it absorbs the Void Phantasm, so any HP
        // loss on it means the group is on the hard-mode route.
        hard_mode_idol(&mut t);

        let bridge = finish_shatters_fight(
            &mut t,
            700,
            crate::assets::SHATTERS_BRIDGE_SENTINEL_TYPE,
            250_000,
            100,
        );
        assert_eq!(bridge.boss_name, "Valen the Unbreakable");
        assert!(bridge.shatters_hm);
    }

    #[test]
    fn shatters_bridge_sentinel_stays_regular_without_evidence() {
        // The Idol was never in view at all -- the common case, since a regular
        // run's Idol sits across the map in the Derelict Village. That is not
        // evidence of anything: the Sentinel keeps its regular name and the run
        // is not hard mode.
        let mut t = CombatTracker::new();
        t.on_map_change("The Shatters", 777, 0);
        t.on_player_loaded(1000, 777);
        t.on_object_spawn(600, 0x0400, &status(600, vec![name_stat("Bob")]), 0);

        let bridge = finish_shatters_fight(
            &mut t,
            700,
            crate::assets::SHATTERS_BRIDGE_SENTINEL_TYPE,
            250_000,
            100,
        );
        assert!(
            !bridge.boss_name.contains("Valen"),
            "regular name kept, got {}",
            bridge.boss_name
        );
        assert!(!bridge.shatters_hm);
    }

    #[test]
    fn shatters_the_source_alone_marks_the_whole_run_hard_mode() {
        let mut t = CombatTracker::new();
        t.on_map_change("The Shatters", 777, 0);
        t.on_player_loaded(1000, 777);
        t.on_object_spawn(600, 0x0400, &status(600, vec![name_stat("Bob")]), 0);

        // The Idol was killed before we ever saw it, so the only hard-mode proof
        // is The Source -- an object that is never spawned outside hard mode. It
        // is far away and its HP is never reported, which must not matter.
        t.on_object_spawn(
            901,
            crate::assets::SHATTERS_THE_SOURCE_TYPE,
            &status_at(901, 200.0, 200.0, vec![]),
            20,
        );
        assert_eq!(
            t.shatters_hm_revealed_name(crate::assets::SHATTERS_BRIDGE_SENTINEL_TYPE),
            Some("Valen the Unbreakable"),
            "seeing the Source proves the bridge stage too"
        );

        let archmage = finish_shatters_fight(
            &mut t,
            701,
            crate::assets::SHATTERS_TWILIGHT_ARCHMAGE_TYPE,
            300_000,
            100,
        );
        assert_eq!(archmage.boss_name, "Nox the Wild Shadow");
        assert!(archmage.shatters_hm);
        let king =
            finish_shatters_fight(&mut t, 702, crate::assets::SHATTERS_KING_TYPE, 400_000, 300);
        assert_eq!(king.boss_name, "King Azamoth");
        assert!(king.shatters_hm);
    }

    #[test]
    fn shatters_the_source_destroyed_in_view_marks_hard_mode() {
        let mut t = CombatTracker::new();
        t.on_map_change("The Shatters", 777, 0);
        t.on_player_loaded(1000, 777);
        t.on_object_spawn(600, 0x0400, &status(600, vec![name_stat("Bob")]), 0);
        t.on_object_spawn(
            901,
            crate::assets::SHATTERS_THE_SOURCE_TYPE,
            &status(
                901,
                vec![stat(StatType::MaxHP, 30_000), stat(StatType::HP, 1)],
            ),
            20,
        );
        t.on_object_removed(901, 22);

        let archmage = finish_shatters_fight(
            &mut t,
            701,
            crate::assets::SHATTERS_TWILIGHT_ARCHMAGE_TYPE,
            300_000,
            100,
        );
        assert_eq!(archmage.boss_name, "Nox the Wild Shadow");
        assert!(archmage.shatters_hm);
    }

    #[test]
    fn shatters_bridge_only_hard_mode_keeps_the_archmage_regular() {
        let mut t = CombatTracker::new();
        t.on_map_change("The Shatters", 777, 0);
        t.on_player_loaded(1000, 777);
        t.on_object_spawn(600, 0x0400, &status(600, vec![name_stat("Bob")]), 0);
        hard_mode_idol(&mut t);

        // The group stopped after Valen (no Alchemy Lab, or the Source survived),
        // so the archmage is fought in its regular form.
        let bridge = finish_shatters_fight(
            &mut t,
            700,
            crate::assets::SHATTERS_BRIDGE_SENTINEL_TYPE,
            250_000,
            100,
        );
        assert_eq!(bridge.boss_name, "Valen the Unbreakable");
        assert!(bridge.shatters_hm);
        let archmage = finish_shatters_fight(
            &mut t,
            701,
            crate::assets::SHATTERS_TWILIGHT_ARCHMAGE_TYPE,
            300_000,
            300,
        );
        assert!(
            !archmage.shatters_hm,
            "a regular archmage keeps its own phase regular"
        );
        assert!(
            !archmage.boss_name.contains("Nox"),
            "got {}",
            archmage.boss_name
        );
    }

    #[test]
    fn shatters_idol_evidence_after_the_bridge_started_stays_regular() {
        // Hard mode can be wasted by pulling the first boss before the Idol is
        // destroyed: the group reaches the bridge, fights the Sentinel in regular
        // mode, and only then takes the Idol down. The evidence belongs to the
        // bosses after the bridge, so the Sentinel stays regular -- while the late
        // stage (proved by The Source) still latches.
        let mut t = CombatTracker::new();
        t.on_map_change("The Shatters", 777, 0);
        t.on_player_loaded(1000, 777);
        // The Sentinel is in view: the bridge fight is under way.
        t.on_object_spawn(
            700,
            crate::assets::SHATTERS_BRIDGE_SENTINEL_TYPE,
            &status(
                700,
                vec![stat(StatType::MaxHP, 250_000), stat(StatType::HP, 250_000)],
            ),
            5,
        );
        // Only now does the Idol become damageable.
        t.on_object_spawn(
            900,
            crate::assets::SHATTERS_STONE_IDOL_TYPE,
            &status(
                900,
                vec![stat(StatType::MaxHP, 25_000), stat(StatType::HP, 25_000)],
            ),
            8,
        );
        t.on_object_status(900, &status(900, vec![stat(StatType::HP, 20_000)]), 10);
        // The Source (past the archmage) still proves the late stage.
        t.on_object_spawn(
            901,
            crate::assets::SHATTERS_THE_SOURCE_TYPE,
            &status_at(901, 200.0, 200.0, vec![]),
            12,
        );

        t.on_damage(700, 1000, 100_000, 20);
        t.on_local_hit(700, 7, 1000, 1000, 22);
        t.on_object_status(700, &status(700, vec![stat(StatType::HP, 0)]), 26);
        let bridge = t.on_object_removed(700, 30).expect("sentinel fight");
        assert!(
            !bridge.shatters_hm,
            "evidence that arrived mid-fight does not make the Sentinel hard mode"
        );
        assert!(
            !bridge.boss_name.contains("Valen"),
            "got {}",
            bridge.boss_name
        );
        assert!(
            t.shatters_hm_revealed_name(crate::assets::SHATTERS_TWILIGHT_ARCHMAGE_TYPE)
                .is_some(),
            "the late stage is still hard mode"
        );
    }

    #[test]
    fn shatters_tempest_marks_the_late_stage_only() {
        // Tempest, the phoenix the hard-mode archmage summons, is the late
        // stage's own evidence: the bridge stays regular when nothing proved hard
        // mode before it, but the archmage and the King are renamed.
        let mut t = CombatTracker::new();
        t.on_map_change("The Shatters", 777, 0);
        t.on_player_loaded(1000, 777);
        t.on_object_spawn(600, 0x0400, &status(600, vec![name_stat("Bob")]), 0);

        let bridge = finish_shatters_fight(
            &mut t,
            700,
            crate::assets::SHATTERS_BRIDGE_SENTINEL_TYPE,
            250_000,
            10,
        );
        assert!(!bridge.shatters_hm, "no evidence before the bridge");

        t.on_object_spawn(
            902,
            crate::assets::SHATTERS_TEMPEST_TYPE,
            &status_at(902, 30.0, 30.0, vec![]),
            20,
        );
        let archmage = finish_shatters_fight(
            &mut t,
            701,
            crate::assets::SHATTERS_TWILIGHT_ARCHMAGE_TYPE,
            300_000,
            30,
        );
        assert_eq!(archmage.boss_name, "Nox the Wild Shadow");
        assert!(archmage.shatters_hm);
        let king =
            finish_shatters_fight(&mut t, 702, crate::assets::SHATTERS_KING_TYPE, 500_000, 50);
        assert_eq!(king.boss_name, "King Azamoth");
        assert!(king.shatters_hm);
    }

    #[test]
    fn shatters_hm_speech_marks_the_boss_that_spoke() {
        // A hard-mode boss speaks under its revealed name; the line is the only
        // place the client can learn it from, so it is per-boss evidence.
        let mut t = CombatTracker::new();
        t.on_map_change("The Shatters", 777, 0);
        t.on_player_loaded(1000, 777);
        t.on_object_spawn(600, 0x0400, &status(600, vec![name_stat("Bob")]), 0);
        // The Sentinel speaks as Valen, so the Sentinel is hard mode...
        let spoken = t.on_boss_text(
            700,
            Some("#Valen the Unbreakable"),
            "I see now... my strength could not have held against this growing power.",
            50,
        );
        assert!(spoken.is_empty(), "speech alone finalizes nothing");
        let bridge = finish_shatters_fight(
            &mut t,
            700,
            crate::assets::SHATTERS_BRIDGE_SENTINEL_TYPE,
            250_000,
            100,
        );
        assert_eq!(
            t.shatters_hm_revealed_name(crate::assets::SHATTERS_BRIDGE_SENTINEL_TYPE),
            Some("Valen the Unbreakable")
        );
        assert!(bridge.shatters_hm);
        // ...and the archmage, whose own line never arrived, stays regular: each
        // boss is labelled from its own evidence, not from the run's.
        let archmage = finish_shatters_fight(
            &mut t,
            701,
            crate::assets::SHATTERS_TWILIGHT_ARCHMAGE_TYPE,
            300_000,
            300,
        );
        assert!(!archmage.shatters_hm);
        assert!(
            !archmage.boss_name.contains("Nox"),
            "got {}",
            archmage.boss_name
        );
    }

    #[test]
    fn shatters_regular_speech_is_not_hard_mode() {
        // The regular Sentinel speaks under its regular name, which is no
        // evidence of hard mode however the line is framed.
        let mut t = CombatTracker::new();
        t.on_map_change("The Shatters", 777, 0);
        t.on_player_loaded(1000, 777);
        t.on_boss_text(
            700,
            Some("#The Bridge Sentinel"),
            "I tried to protect you... I have failed.",
            50,
        );
        let bridge = finish_shatters_fight(
            &mut t,
            700,
            crate::assets::SHATTERS_BRIDGE_SENTINEL_TYPE,
            250_000,
            100,
        );
        assert!(!bridge.shatters_hm);
        assert!(
            !bridge.boss_name.contains("Valen"),
            "got {}",
            bridge.boss_name
        );
    }

    #[test]
    fn shatters_late_join_reads_hard_mode_from_the_speech_it_hears() {
        // A player who joined after the Idol and The Source were destroyed has no
        // unlock evidence, but the archmage and the King speak under their
        // hard-mode names, which is what labels them.
        let mut t = CombatTracker::new();
        t.on_map_change("The Shatters", 777, 0);
        t.on_player_loaded(1000, 777);
        t.on_boss_text(
            701,
            Some("[Nox the Wild Shadow]"),
            "Unworthy as you are to know what hides beyond, I've had... an epiphany.",
            20,
        );
        t.on_boss_text(
            702,
            Some("King Azamoth"),
            "This fate is mine to bear... not hers.",
            30,
        );
        let archmage = finish_shatters_fight(
            &mut t,
            701,
            crate::assets::SHATTERS_TWILIGHT_ARCHMAGE_TYPE,
            300_000,
            40,
        );
        assert_eq!(archmage.boss_name, "Nox the Wild Shadow");
        assert!(archmage.shatters_hm);
        let king =
            finish_shatters_fight(&mut t, 702, crate::assets::SHATTERS_KING_TYPE, 500_000, 60);
        assert_eq!(king.boss_name, "King Azamoth");
        assert!(king.shatters_hm);
    }

    #[test]
    fn shatters_stone_idol_is_its_own_phase_of_the_run() {
        let mut t = CombatTracker::new();
        t.on_map_change("The Shatters", 777, 0);
        t.on_player_loaded(1000, 777);
        t.on_object_spawn(600, 0x0400, &status(600, vec![name_stat("Bob")]), 0);

        let idol = finish_shatters_fight(
            &mut t,
            900,
            crate::assets::SHATTERS_STONE_IDOL_TYPE,
            25_000,
            5,
        );
        assert_eq!(
            idol.boss_object_type,
            crate::assets::SHATTERS_STONE_IDOL_TYPE
        );
        assert!(idol.killed);
        // The Idol is tracked as its own row of the dungeon's card, so the loot it
        // drops can be shown under it.
        assert!(
            idol.encounter_run_id.is_some(),
            "the idol joins the dungeon run"
        );
        // It is an object hard mode unlocks, not a boss hard mode renames, so the
        // phase itself is not a hard-mode variant.
        assert!(!idol.shatters_hm);
    }

    /// Damage the Stone Idol, which is invincible outside hard mode.
    fn hard_mode_idol(t: &mut CombatTracker) {
        t.on_object_spawn(
            900,
            crate::assets::SHATTERS_STONE_IDOL_TYPE,
            &status(
                900,
                vec![stat(StatType::MaxHP, 25_000), stat(StatType::HP, 25_000)],
            ),
            5,
        );
        t.on_object_status(900, &status(900, vec![stat(StatType::HP, 24_000)]), 8);
    }

    #[test]
    fn shatters_stone_idol_found_damaged_marks_hard_mode() {
        let mut t = CombatTracker::new();
        t.on_map_change("The Shatters", 777, 0);
        t.on_player_loaded(1000, 777);
        t.on_object_spawn(600, 0x0400, &status(600, vec![name_stat("Bob")]), 0);
        // We arrive after the Idol lost part of its pool (the group already
        // absorbed the Phantasm): hard mode, even though we never saw the drop.
        t.on_object_spawn(
            900,
            crate::assets::SHATTERS_STONE_IDOL_TYPE,
            &status(
                900,
                vec![stat(StatType::MaxHP, 25_000), stat(StatType::HP, 12_000)],
            ),
            5,
        );
        let bridge = finish_shatters_fight(
            &mut t,
            700,
            crate::assets::SHATTERS_BRIDGE_SENTINEL_TYPE,
            250_000,
            100,
        );
        assert_eq!(bridge.boss_name, "Valen the Unbreakable");
        assert!(bridge.shatters_hm);
    }

    #[test]
    fn shatters_stone_idol_killed_in_view_marks_hard_mode() {
        let mut t = CombatTracker::new();
        t.on_map_change("The Shatters", 777, 0);
        t.on_player_loaded(1000, 777);
        t.on_object_spawn(600, 0x0400, &status(600, vec![name_stat("Bob")]), 0);
        // The group destroys the Idol while we watch, without us landing a hit:
        // its HP only becomes readable because it is in our view.
        t.on_object_spawn(
            900,
            crate::assets::SHATTERS_STONE_IDOL_TYPE,
            &status(
                900,
                vec![stat(StatType::MaxHP, 25_000), stat(StatType::HP, 25_000)],
            ),
            5,
        );
        t.on_object_status(900, &status(900, vec![stat(StatType::HP, 0)]), 8);

        let bridge = finish_shatters_fight(
            &mut t,
            700,
            crate::assets::SHATTERS_BRIDGE_SENTINEL_TYPE,
            250_000,
            100,
        );
        assert!(bridge.shatters_hm);
    }

    #[test]
    fn shatters_stone_idol_removed_without_hp_evidence_stays_regular() {
        // The Idol is removed right beside the local player, but the server never
        // reported its pool, so nothing shows it was ever damageable. A regular
        // Idol that simply wandered out of view looks exactly like this, so the
        // removal must not be read as a destruction.
        let mut t = CombatTracker::new();
        t.on_map_change("The Shatters", 777, 0);
        t.on_player_loaded(1000, 777);
        t.on_object_spawn(
            1000,
            0x0321,
            &status_at(1000, 10.0, 10.0, vec![name_stat("Bob")]),
            1,
        );
        t.on_object_spawn(
            900,
            crate::assets::SHATTERS_STONE_IDOL_TYPE,
            &status_at(900, 11.0, 11.0, vec![]),
            2,
        );
        t.on_object_removed(900, 6);

        let bridge = finish_shatters_fight(
            &mut t,
            700,
            crate::assets::SHATTERS_BRIDGE_SENTINEL_TYPE,
            250_000,
            100,
        );
        assert!(
            !bridge.shatters_hm,
            "a removal with no pool to read proves nothing"
        );
        assert!(
            !bridge.boss_name.contains("Valen"),
            "got {}",
            bridge.boss_name
        );
    }

    #[test]
    fn shatters_wandering_stone_idol_leaving_view_stays_regular() {
        // The regular Idol patrols, chases whoever comes close and then wanders
        // off -- it shoots the local player and is last seen two tiles away at
        // full HP before leaving view. Its own movement caused the removal, so
        // the run stays regular.
        let mut t = CombatTracker::new();
        t.on_map_change("The Shatters", 777, 0);
        t.on_player_loaded(1000, 777);
        t.on_object_spawn(
            1000,
            0x0321,
            &status_at(1000, 10.0, 10.0, vec![name_stat("Bob")]),
            1,
        );
        t.on_object_spawn(
            900,
            crate::assets::SHATTERS_STONE_IDOL_TYPE,
            &status_at(
                900,
                10.0,
                12.0,
                vec![stat(StatType::MaxHP, 25_000), stat(StatType::HP, 25_000)],
            ),
            2,
        );
        t.on_object_removed(900, 6);

        let bridge = finish_shatters_fight(
            &mut t,
            700,
            crate::assets::SHATTERS_BRIDGE_SENTINEL_TYPE,
            250_000,
            100,
        );
        assert!(
            !bridge.shatters_hm,
            "an untouched Idol that patrolled away is not a kill"
        );
        assert!(
            !bridge.boss_name.contains("Valen"),
            "got {}",
            bridge.boss_name
        );
    }

    #[test]
    fn shatters_stone_idol_leaving_view_marks_nothing() {
        let mut t = CombatTracker::new();
        t.on_map_change("The Shatters", 777, 0);
        t.on_player_loaded(1000, 777);
        // The local player walks away from the Idol: the removal is a view cull,
        // so the run stays regular. This is the common case in a non-hard-mode
        // Shatters, where the Idol cannot be killed at all -- and the cull fires
        // far outside the near radius, so it is never mistaken for a kill.
        t.on_object_spawn(
            1000,
            0x0321,
            &status_at(1000, 40.0, 40.0, vec![name_stat("Bob")]),
            1,
        );
        t.on_object_spawn(
            900,
            crate::assets::SHATTERS_STONE_IDOL_TYPE,
            &status_at(
                900,
                0.0,
                0.0,
                vec![stat(StatType::MaxHP, 25_000), stat(StatType::HP, 25_000)],
            ),
            2,
        );
        t.on_object_removed(900, 6);

        let bridge = finish_shatters_fight(
            &mut t,
            700,
            crate::assets::SHATTERS_BRIDGE_SENTINEL_TYPE,
            250_000,
            100,
        );
        assert!(!bridge.shatters_hm);
        assert!(
            !bridge.boss_name.contains("Valen"),
            "got {}",
            bridge.boss_name
        );
    }

    #[test]
    fn shatters_stone_idol_at_lethal_hp_marks_hard_mode() {
        let mut t = CombatTracker::new();
        t.on_map_change("The Shatters", 777, 0);
        t.on_player_loaded(1000, 777);
        t.on_object_spawn(600, 0x0400, &status(600, vec![name_stat("Bob")]), 0);
        // The group takes the Idol down to lethal HP in our view (the kill itself
        // happens out of view, so it is never removed for us): the pool we read
        // proves it was damageable, which is hard mode.
        t.on_object_spawn(
            900,
            crate::assets::SHATTERS_STONE_IDOL_TYPE,
            &status(
                900,
                vec![stat(StatType::MaxHP, 25_000), stat(StatType::HP, 25_000)],
            ),
            2,
        );
        t.on_object_status(900, &status(900, vec![stat(StatType::HP, 1_000)]), 4);

        let bridge = finish_shatters_fight(
            &mut t,
            700,
            crate::assets::SHATTERS_BRIDGE_SENTINEL_TYPE,
            250_000,
            100,
        );
        assert!(bridge.shatters_hm);
    }

    #[test]
    fn shatters_hard_mode_resets_with_the_instance() {
        let mut t = CombatTracker::new();
        t.on_map_change("The Shatters", 777, 0);
        t.on_player_loaded(1000, 777);
        t.on_object_spawn(600, 0x0400, &status(600, vec![name_stat("Bob")]), 0);
        hard_mode_idol(&mut t);
        let hard = finish_shatters_fight(
            &mut t,
            700,
            crate::assets::SHATTERS_BRIDGE_SENTINEL_TYPE,
            250_000,
            100,
        );
        assert!(hard.shatters_hm);

        // The next Shatters instance starts regular again: nothing proved hard
        // mode there, and the Sentinel's own evidence is what carries the mode.
        t.on_map_change("Nexus", 0, 500);
        t.on_map_change("The Shatters", 778, 600);
        t.on_player_loaded(1000, 778);
        t.on_object_spawn(600, 0x0400, &status(600, vec![name_stat("Bob")]), 600);
        let regular = finish_shatters_fight(
            &mut t,
            710,
            crate::assets::SHATTERS_BRIDGE_SENTINEL_TYPE,
            250_000,
            700,
        );
        assert!(!regular.shatters_hm);
        assert!(
            !regular.boss_name.contains("Valen"),
            "got {}",
            regular.boss_name
        );
    }

    fn local_damage_event(time_ms: i64, damage: i64, pet_id: Option<i32>) -> LocalDamageEvent {
        LocalDamageEvent {
            time_ms,
            damage,
            destination: FightDestination::Boss(0),
            pet_id,
            summon: None,
            removal: None,
        }
    }

    fn local_damage_event_from_summon(
        time_ms: i64,
        damage: i64,
        pet_id: Option<i32>,
        summon: (i32, i32),
    ) -> LocalDamageEvent {
        LocalDamageEvent {
            time_ms,
            damage,
            destination: FightDestination::Boss(0),
            pet_id,
            summon: Some(summon),
            removal: None,
        }
    }

    fn ability_use(time_ms: i64, item_id: i32, window_ms: Option<i64>) -> LocalAbilityUse {
        LocalAbilityUse {
            time_ms,
            item_id,
            enchants: Vec::new(),
            server_effect_window_ms: window_ms,
        }
    }

    fn no_creatures_seen() -> HashSet<i32> {
        HashSet::new()
    }

    /// A tracker holding one boss, the local player, and -- when `pet_type` is
    /// non-zero -- a pet out next to the player with the given abilities.
    fn pet_packet_tracker(pet_abilities: &[i32], pet_type: i32) -> CombatTracker {
        let mut t = CombatTracker::new();
        t.on_map_change("Sprite World", 1, 0);
        t.on_player_loaded(1000, 1);
        t.on_object_status(1000, &status_at(1000, 0.0, 0.0, vec![name_stat("You")]), 0);
        t.on_object_spawn(
            500,
            0x10E4,
            &status(
                500,
                vec![
                    stat(StatType::MaxHP, 16_000),
                    stat(StatType::HP, 16_000),
                    stat(StatType::Defense, 0),
                ],
            ),
            0,
        );
        if pet_type != 0 {
            let mut stats = vec![stat(StatType::PetType, pet_type)];
            for (i, ability) in pet_abilities.iter().enumerate() {
                let kind = [
                    StatType::PetFirstAbilityType,
                    StatType::PetSecondAbilityType,
                    StatType::PetThirdAbilityType,
                ][i];
                stats.push(stat(kind, *ability));
            }
            t.on_object_spawn(900, 0x9999, &status_at(900, 1.0, 1.0, stats), 0);
        }
        t
    }

    fn local_row(done: &CompletedFight) -> &FightParticipant {
        done.participants
            .iter()
            .find(|p| p.is_local)
            .expect("the local player's row")
    }

    #[test]
    fn a_spawn_window_only_claims_what_a_pet_cannot_explain() {
        let use_ = ability_use(1_000, 999, None);
        let uses = vec![&use_];
        let spawns = [(0x500a, None)];
        // A pet out at the same moment owns this one: a spammed ability keeps the
        // window open almost always, so it can no longer tell the two apart, and
        // the pet is the source that was actually observed.
        assert_eq!(
            packet_damage_source(
                &uses,
                &spawns,
                &no_creatures_seen(),
                &local_damage_event(3_000, 130, Some(42))
            ),
            Some(PacketDamageSource::Pet { pet_id: 42 })
        );
        // With no pet out the window is the only explanation left, and the row
        // falls back to the creature's type since it was never seen.
        assert_eq!(
            packet_damage_source(
                &uses,
                &spawns,
                &no_creatures_seen(),
                &local_damage_event(3_000, 130, None)
            ),
            Some(PacketDamageSource::Summon {
                creature: 0x500a,
                object_id: None,
            })
        );
        // Past the window, with no pet and nothing seen: unattributed.
        assert_eq!(
            packet_damage_source(
                &uses,
                &spawns,
                &no_creatures_seen(),
                &local_damage_event(9_000, 130, None)
            ),
            None
        );
    }

    #[test]
    fn a_creature_seen_in_the_world_outranks_the_timing_window_and_the_pet() {
        let use_ = ability_use(1_000, 999, None);
        let uses = vec![&use_];
        let spawns = [(0x500a, None)];
        let seen = no_creatures_seen();
        // Seen alive and in reach of the target: it is the shooter, and the row
        // keys on the instance that was actually observed.
        assert_eq!(
            packet_damage_source(
                &uses,
                &spawns,
                &seen,
                &local_damage_event_from_summon(3_000, 130, Some(42), (777, 0x500a)),
            ),
            Some(PacketDamageSource::Summon {
                creature: 0x500a,
                object_id: Some(777),
            })
        );
        // A spammed ability puts every packet inside the window, so a creature
        // seen in the world is what keeps a pet's own hits off the summon's row --
        // and out of the window's reach once it is gone.
        assert_eq!(
            packet_damage_source(
                &uses,
                &spawns,
                &seen,
                &local_damage_event_from_summon(4_000, 280, Some(42), (777, 0x500a)),
            ),
            Some(PacketDamageSource::Summon {
                creature: 0x500a,
                object_id: Some(777),
            })
        );
    }

    #[test]
    fn a_creature_seen_in_the_world_beats_one_only_its_window_reached() {
        // Two items in the loadout each spawn their own creature on their own use.
        // The one activated last is not necessarily the one that is out, so a
        // creature actually seen around the fight takes the packet.
        let first = ability_use(1_000, 111, None);
        let second = ability_use(4_000, 222, None);
        let uses = vec![&first, &second];
        let spawns = [(0x500a, Some(111)), (0xb053, Some(222))];
        let event = local_damage_event(4_500, 130, None);
        // Nothing seen: the window's own order decides, and the later use wins.
        assert_eq!(
            packet_damage_source(&uses, &spawns, &no_creatures_seen(), &event),
            Some(PacketDamageSource::Summon {
                creature: 0xb053,
                object_id: None,
            })
        );
        // Seen in the world: it takes the packet despite the later activation.
        let mut seen = no_creatures_seen();
        seen.insert(0x500a);
        assert_eq!(
            packet_damage_source(&uses, &spawns, &seen, &event),
            Some(PacketDamageSource::Summon {
                creature: 0x500a,
                object_id: None,
            })
        );
    }

    #[test]
    fn a_summon_declared_on_the_items_own_use_ignores_other_activations() {
        let other = ability_use(1_000, 111, None);
        let own = ability_use(2_000, 999, None);
        let uses = vec![&other, &own];
        let spawns = [(0x4683, Some(999))];
        assert_eq!(
            packet_damage_source(
                &uses,
                &spawns,
                &no_creatures_seen(),
                &local_damage_event(1_500, 40, None)
            ),
            None,
            "an unrelated activation cannot spawn this item's creature"
        );
        assert_eq!(
            packet_damage_source(
                &uses,
                &spawns,
                &no_creatures_seen(),
                &local_damage_event(2_500, 40, None)
            ),
            Some(PacketDamageSource::Summon {
                creature: 0x4683,
                object_id: None,
            })
        );
    }

    #[test]
    fn a_packet_an_activation_already_owns_is_never_handed_to_a_pet() {
        // The item declares a server-applied effect, so its own window owns the
        // packet (`ability_activations_for` credits it there). A pet out at the
        // same moment must not be credited the same damage a second time.
        let use_ = ability_use(1_000, 999, Some(2_000));
        let uses = vec![&use_];
        assert_eq!(
            packet_damage_source(
                &uses,
                &[],
                &no_creatures_seen(),
                &local_damage_event(1_500, 500, Some(42))
            ),
            None
        );
    }

    #[test]
    fn map_change_and_object_id_reuse_clear_local_summon_ownership() {
        let summon = LocalSummonEntity {
            object_type: 0x500a,
            first_seen_ms: 10,
            last_seen_ms: 10,
            x: 1.0,
            y: 1.0,
        };
        let mut t = CombatTracker::new();
        t.local_summons.insert(900, summon);
        t.summon_owners.insert(900, 1000);
        t.on_map_change("Realm", 1, 0);
        assert!(t.local_summons.is_empty(), "map change clears summons");
        assert!(t.summon_owners.is_empty(), "map change clears ownership");

        // A map-less session can still recycle an id: a player-class spawn must
        // not keep the old summon's identity.
        t.local_summons.insert(900, summon);
        t.summon_owners.insert(900, 1000);
        t.on_object_spawn(900, 0x0321, &status(900, vec![name_stat("Bob")]), 20);
        assert!(!t.local_summons.contains_key(&900));
        assert!(!t.summon_owners.contains_key(&900));
    }

    #[test]
    fn owner_credited_packet_damage_lands_on_the_pets_own_row() {
        let mut t = pet_packet_tracker(&[406, 407, 408], 32643);
        // The server credits the pet's hits to the player: attacker = the player.
        t.on_damage(500, 1000, 280, 200);
        t.on_damage(500, 1000, 280, 300);
        let fight = t.fights.remove(&500).expect("open fight");
        let done = t.finalize_fight(fight, 1_000).expect("finalize");
        let me = local_row(&done);
        assert_eq!((me.summon_damage, me.summon_hits), (560, 2));
        let row = me.summons.first().expect("a pet row");
        assert!(row.is_pet);
        assert_eq!(row.pet_type, 32643);
        assert_eq!((row.damage, row.hits), (560, 2));
        // The raw packet channel keeps its own tally.
        assert_eq!((me.packet_damage, me.packet_hits), (560, 2));
    }

    #[test]
    fn a_savage_only_pet_claims_nothing() {
        // Heal, magic heal and Savage: the pet cannot deal damage at all, so the
        // packets stay unattributed instead of being credited to it.
        let mut t = pet_packet_tracker(&[408, 407, 409], 32625);
        assert_eq!(t.damage_capable_pet_near_local(), None);
        t.on_damage(500, 1000, 130, 200);
        let fight = t.fights.remove(&500).expect("open fight");
        let done = t.finalize_fight(fight, 1_000).expect("finalize");
        let me = local_row(&done);
        assert_eq!((me.summon_damage, me.summon_hits), (0, 0));
        assert!(me.summons.is_empty());
        assert_eq!((me.packet_damage, me.packet_hits), (130, 1));
    }

    #[test]
    fn a_creature_nothing_in_the_loadout_declares_is_never_tracked() {
        // The loadout here declares nothing (no assets are loaded in a unit test),
        // so a spawn that merely looks like a summon is left alone: attribution
        // never guesses from a type it cannot place.
        let mut t = pet_packet_tracker(&[], 0);
        t.on_object_spawn(900, 0x500a, &status_at(900, 1.0, 1.0, vec![]), 0);
        assert!(t.local_summons.is_empty());
        assert_eq!(t.local_summon_attacking(500, 150), None);
    }

    #[test]
    fn a_tracked_summon_claims_only_what_is_in_reach_and_forgets_despawns() {
        let mut t = pet_packet_tracker(&[], 0);
        t.local_summons.insert(
            900,
            LocalSummonEntity {
                object_type: 0x500a,
                first_seen_ms: 100,
                last_seen_ms: 100,
                x: 1.0,
                y: 1.0,
            },
        );
        // Seen on the boss: it can have hit it.
        assert_eq!(t.local_summon_attacking(500, 150), Some((900, 0x500a)));
        // Walked away from the target it would have had to hit.
        t.local_summons.get_mut(&900).unwrap().x = 40.0;
        t.local_summons.get_mut(&900).unwrap().y = 40.0;
        assert_eq!(t.local_summon_attacking(500, 150), None);
        // Back in reach, but despawned: no longer a candidate at all.
        t.local_summons.get_mut(&900).unwrap().x = 1.0;
        t.local_summons.get_mut(&900).unwrap().y = 1.0;
        t.on_object_removed(900, 200);
        assert_eq!(t.local_summon_attacking(500, 150), None);
    }

    #[test]
    fn a_teammates_damage_pet_cannot_claim_the_local_players_packets() {
        // In a group every pet stands near someone. Proximity alone made the
        // nearest damage-capable pet the local player's, so a teammate's pet beside
        // them claimed the local player's own unattributed packets: a fight whose
        // only pet of theirs was a Decoy pet still grew a pet row worth 943 damage,
        // which was really a teammate's Great Jelly standing nearby.
        let mut t = pet_packet_tracker(&[], 0);
        // A teammate (a Necromancer) with their Electric pet at their side; the
        // local player is at the origin, well inside pet range of it.
        t.on_object_spawn(
            600,
            0x0321,
            &status_at(600, 1.0, 1.0, vec![name_stat("Friend")]),
            0,
        );
        t.on_object_spawn(
            901,
            0x9999,
            &status_at(
                901,
                1.0,
                2.0,
                vec![
                    stat(StatType::PetType, 32_645),
                    stat(StatType::PetFirstAbilityType, 406),
                ],
            ),
            0,
        );
        assert_eq!(
            t.pet_tracks.get(&901).map(|p| p.votes.clone()),
            Some(HashMap::from([(600, 1)])),
            "the pet follows the teammate, not the local player"
        );
        assert_eq!(
            t.damage_capable_pet_near_local(),
            None,
            "a teammate's pet is not the local player's, however close it stands"
        );
    }

    #[test]
    fn only_the_pet_the_character_cache_names_can_claim_their_packets() {
        // The pet ATTRIBUTES shows is the character's own, and the wire reports
        // every pet in the world with its own type id, so the cache's type decides
        // which one that is -- distance cannot, because in a group every pet stands
        // near someone.
        let mut t = pet_packet_tracker(&[], 0);
        // 32548 is this character's pet (Blue Ant); 32645 is a teammate's (Great
        // Jelly) standing nearer to the local player than their own pet is.
        t.set_local_snapshot(None, false, 24_724, 32_548);
        t.on_object_spawn(
            901,
            0x9999,
            &status_at(
                901,
                0.5,
                0.5,
                vec![
                    stat(StatType::PetType, 32_645),
                    stat(StatType::PetFirstAbilityType, 406),
                ],
            ),
            0,
        );
        assert_eq!(
            t.damage_capable_pet_near_local(),
            None,
            "a Great Jelly is not this character's Blue Ant"
        );
        // Their own pet out, and able to damage: it is the claimant.
        t.on_object_spawn(
            902,
            0x9999,
            &status_at(
                902,
                2.0,
                2.0,
                vec![
                    stat(StatType::PetType, 32_548),
                    stat(StatType::PetFirstAbilityType, 406),
                ],
            ),
            0,
        );
        assert_eq!(
            t.damage_capable_pet_near_local(),
            Some(902),
            "the pet the cache names is the one that can claim"
        );
    }

    #[test]
    fn a_shooter_that_is_no_creature_of_mine_is_my_own_hit() {
        // A secondary projectile names a shooter id that can belong to an object that
        // is no creature of the player's -- ids are reused as objects despawn -- and
        // reading that object as the source invented pet rows out of other players'
        // pets: rows for a Battle God, a Poison Drake, a Great Jelly, none of them
        // the player's, on the player's own row.
        let mut t = pet_packet_tracker(&[], 0);
        t.set_local_snapshot(None, false, 24_724, 32_548);
        t.on_object_spawn(
            700,
            0x9999,
            &status_at(
                700,
                3.0,
                3.0,
                vec![
                    stat(StatType::PetType, 32_645),
                    stat(StatType::PetFirstAbilityType, 406),
                ],
            ),
            0,
        );
        // The local player's own hit, reported with that pet's id as its shooter.
        t.on_local_hit(500, 5, 700, 1000, 10);
        let accum = t
            .fights
            .get(&500)
            .and_then(|f| f.attackers.get(&1000))
            .expect("the local player's row");
        assert!(
            accum.summons.is_empty(),
            "no creature of the local player's fired this shot"
        );
        assert_eq!((accum.summon_damage, accum.summon_hits), (0, 0));
        assert_eq!(accum.hits, 1, "it stays a hit of theirs");
    }

    #[test]
    fn an_unexplained_packet_is_credited_to_the_object_that_vanished() {
        // A barrel pushed into the boss: the server credits the player, and the only
        // thing on the wire that names the source is the barrel leaving the world as
        // the hit lands (measured on a barrel-only Nightmare Colony: 105 of 105 hits,
        // same tick, and the whole fight on one row).
        let mut t = pet_packet_tracker(&[], 0);
        for (i, at) in [400, 500, 600, 700].into_iter().enumerate() {
            let id = 700 + i as i32;
            t.on_object_spawn(id, 0x0786, &status_at(id, 1.0, 1.0, vec![]), 0);
            t.on_object_removed(id, at);
        }
        t.on_damage(500, 1000, 1_006, 700);
        let fight = t.fights.remove(&500).expect("open fight");
        let done = t.finalize_fight(fight, 1_000).expect("finalize");
        let me = local_row(&done);
        let row = me.summons.first().expect("a row for the barrel");
        assert!(!row.is_pet);
        assert_eq!(row.object_type, 0x0786, "named and drawn as the barrel");
        assert_eq!((row.damage, row.hits), (1_006, 1));
        assert_eq!((me.packet_damage, me.packet_hits), (1_006, 1));
    }

    #[test]
    fn an_object_that_despawns_once_claims_nothing() {
        // A boss room is full of things that despawn -- a trap, a wall piece, a mob --
        // and one of them leaving the world as a barrel hit lands is not the barrel.
        // Only an object that is such a mechanic is a source at all, so this hit stays
        // unattributed rather than being credited to the nearest despawn.
        let mut t = pet_packet_tracker(&[], 0);
        t.on_object_spawn(700, 0x0813, &status_at(700, 1.0, 1.0, vec![]), 0);
        t.on_object_removed(700, 700);
        t.on_damage(500, 1000, 1_006, 700);
        let fight = t.fights.remove(&500).expect("open fight");
        let done = t.finalize_fight(fight, 1_000).expect("finalize");
        let me = local_row(&done);
        assert!(me.summons.is_empty(), "a wall piece explains nothing");
        assert_eq!((me.packet_damage, me.packet_hits), (1_006, 1));
    }

    #[test]
    fn both_barrel_types_report_as_the_one_that_draws() {
        // The room has two barrels and the game gives them different object types;
        // only one has a texture in the atlas, so reporting them separately showed the
        // same name twice, once without a sprite. One mechanic, one row.
        let mut t = pet_packet_tracker(&[], 0);
        t.on_object_spawn(700, 0x0786, &status_at(700, 1.0, 1.0, vec![]), 0);
        t.on_object_removed(700, 500);
        t.on_damage(500, 1000, 1_100, 500);
        t.on_object_spawn(701, 0x0796, &status_at(701, 1.0, 1.0, vec![]), 0);
        t.on_object_removed(701, 700);
        t.on_damage(500, 1000, 900, 700);
        let fight = t.fights.remove(&500).expect("open fight");
        let done = t.finalize_fight(fight, 1_000).expect("finalize");
        let me = local_row(&done);
        assert_eq!(me.summons.len(), 1, "one row for the mechanic");
        assert_eq!(
            me.summons[0].object_type, 0x0786,
            "drawn by the type that renders"
        );
        assert_eq!((me.summons[0].damage, me.summons[0].hits), (2_000, 2));
    }

    #[test]
    fn a_wall_piece_despawns_with_the_barrel_and_stays_out_of_it() {
        // Both the barrel and a wall piece leave the world as the hit lands. The row
        // is the barrel's -- the wall is not a mechanic that can damage the boss, so it
        // never becomes a candidate, however close in time it went.
        let mut t = pet_packet_tracker(&[], 0);
        t.on_object_spawn(800, 0x0813, &status_at(800, 2.0, 2.0, vec![]), 0);
        t.on_object_removed(800, 500);
        t.on_object_spawn(700, 0x0786, &status_at(700, 1.0, 1.0, vec![]), 0);
        t.on_object_removed(700, 500);
        t.on_damage(500, 1000, 1_088, 500);
        let fight = t.fights.remove(&500).expect("open fight");
        let done = t.finalize_fight(fight, 1_000).expect("finalize");
        let me = local_row(&done);
        assert_eq!(me.summons.len(), 1, "one row, not one per despawn");
        assert_eq!(me.summons[0].object_type, 0x0786);
        assert_eq!((me.summons[0].damage, me.summons[0].hits), (1_088, 1));
    }

    #[test]
    fn a_removal_away_from_the_hit_explains_nothing() {
        // The correlation is place-bound as well as tick-bound: a barrel leaving the
        // world on the far side of the instance is not this packet's source, and the
        // damage stays unattributed rather than being credited to it.
        let mut t = pet_packet_tracker(&[], 0);
        for (i, at) in [400, 500, 600, 700].into_iter().enumerate() {
            let id = 700 + i as i32;
            t.on_object_spawn(id, 0x0786, &status_at(id, 60.0, 60.0, vec![]), 0);
            t.on_object_removed(id, at);
        }
        t.on_damage(500, 1000, 1_006, 700);
        let fight = t.fights.remove(&500).expect("open fight");
        let done = t.finalize_fight(fight, 1_000).expect("finalize");
        let me = local_row(&done);
        assert!(me.summons.is_empty(), "nothing explains this hit");
        assert_eq!((me.packet_damage, me.packet_hits), (1_006, 1));
    }

    #[test]
    fn an_electric_pet_is_a_candidate_only_while_it_is_out_and_in_range() {
        let mut t = pet_packet_tracker(&[406], 32643);
        assert_eq!(t.damage_capable_pet_near_local(), Some(900));
        // Out of range of its owner: it cannot be the shooter.
        t.on_object_status(900, &status_at(900, 40.0, 40.0, vec![]), 10);
        assert_eq!(t.damage_capable_pet_near_local(), None);
        // Back in range but with Savage instead of a damage ability.
        t.on_object_status(
            900,
            &status_at(
                900,
                1.0,
                1.0,
                vec![stat(StatType::PetFirstAbilityType, 409)],
            ),
            20,
        );
        assert_eq!(t.damage_capable_pet_near_local(), None);
    }

    #[test]
    fn a_fight_is_stamped_with_the_state_it_was_fought_under() {
        // The processor pushes the character cache's state (the ritual and the
        // crucible never reach the engine as numeric stats). It is written into the
        // fight's own record at finalize, so switching the ritual off afterwards
        // cannot rewrite what the fight says it was fought under.
        let mut t = pet_packet_tracker(&[], 0);
        t.set_local_snapshot(Some("tier-a".to_string()), true, 0, 0);
        t.on_damage(500, 1000, 280, 200);
        let fight = t.fights.remove(&500).expect("open fight");
        let done = t.finalize_fight(fight, 1_000).expect("finalize");
        let was_on = |f: &CompletedFight, label: &str| {
            f.boosts.iter().any(|b| b.stat == label && b.to > b.from)
        };
        assert!(was_on(&done, "Blood Ritual"));
        assert!(was_on(&done, "Crucible"));
        assert_eq!(
            done.boosts
                .iter()
                .find(|b| b.stat == "Blood Ritual")
                .map(|b| b.source.as_str()),
            Some("character state:tier-a"),
            "the fight keeps the tier it was fought under"
        );

        t.set_local_snapshot(None, false, 0, 0);
        assert!(
            was_on(&done, "Blood Ritual"),
            "a recorded fight does not follow a later switch-off"
        );
        assert!(was_on(&done, "Crucible"));

        // A fight fought without them carries neither, so a row recorded before
        // this was tracked reads as off rather than as a live value.
        let mut off = pet_packet_tracker(&[], 0);
        off.on_damage(500, 1000, 280, 200);
        let fight = off.fights.remove(&500).expect("open fight");
        let plain = off.finalize_fight(fight, 1_000).expect("finalize");
        assert!(!was_on(&plain, "Blood Ritual"));
        assert!(!was_on(&plain, "Crucible"));
    }

    #[test]
    fn a_creature_that_only_carries_a_pet_name_is_not_a_pet() {
        // A Pocket Rat arrives with a `PetName` and no pet type. Crediting its hits
        // to a nameless "Pet" row hides both the creature and the item that made it,
        // so pet-ness requires an identity -- and this row then shows the rat, from
        // the rat.
        let mut t = pet_packet_tracker(&[], 0);
        t.on_object_spawn(
            700,
            0x4683,
            &status(700, vec![pet_name_stat("Pocket Rat")]),
            0,
        );
        t.on_ally_shot(700, 1000, 5, 1, 50, 0x4683, 0);
        t.on_local_hit(500, 5, 700, 1000, 10);
        let fight = t.fights.remove(&500).expect("open fight");
        let done = t.finalize_fight(fight, 1_000).expect("finalize");
        let me = local_row(&done);
        let row = me.summons.first().expect("a row");
        assert!(!row.is_pet, "a name with no pet type is a creature");
        assert_eq!(row.object_type, 0x4683, "named and drawn by its own type");
        assert_eq!(row.pet_type, 0, "no pet skin to draw");
    }

    #[test]
    fn the_local_pets_row_carries_the_skin_the_cache_reports() {
        // The wire's pet id is a *type*, not a sprite: ATTRIBUTES resolves the pet
        // from the account cache, and the fight's own row does the same, so the row
        // draws the pet and not a placeholder.
        let mut t = pet_packet_tracker(&[406, 407, 408], 32643);
        t.set_local_snapshot(None, false, 12_345, 0);
        t.on_damage(500, 1000, 280, 200);
        let fight = t.fights.remove(&500).expect("open fight");
        let done = t.finalize_fight(fight, 1_000).expect("finalize");
        let me = local_row(&done);
        let row = me.summons.first().expect("a pet row");
        assert!(row.is_pet);
        assert_eq!(
            row.pet_type, 12_345,
            "the cache's skin, not the wire's type"
        );
    }

    fn ability_use_with(time_ms: i64, item_id: i32, enchants: &[u16]) -> LocalAbilityUse {
        LocalAbilityUse {
            time_ms,
            item_id,
            enchants: enchants.to_vec(),
            server_effect_window_ms: None,
        }
    }

    fn no_enchants() -> Vec<Vec<u16>> {
        vec![Vec::new(), Vec::new(), Vec::new(), Vec::new()]
    }

    #[test]
    fn a_bear_is_credited_to_the_mace_that_was_used_not_to_its_twin() {
        // Both a Sovereign Mace and its (SB) twin declare `Summon Bear`, so the
        // catalog cannot tell them apart -- and the row used to read "from 2
        // items". Only one of them was activated.
        let mut uses = VecDeque::new();
        uses.push_back(ability_use_with(1_000, 0x9581, &[338]));
        let candidates = [
            SummonCandidate {
                slot: 1,
                item: 0x3849,
                trigger: SummonTrigger::Use,
                lifetime_ms: 10_000,
            },
            SummonCandidate {
                slot: 2,
                item: 0x9581,
                trigger: SummonTrigger::Use,
                lifetime_ms: 10_000,
            },
        ];
        let enchants = [Vec::new(), Vec::new(), vec![338], Vec::new()];
        assert_eq!(
            pick_summon_source(&candidates, &uses, &enchants, 2_000),
            Some((0x9581, vec![338]))
        );
        // Past the bear's own declared 10s lifetime, that activation can no longer
        // explain the creature, and two candidates means nothing is claimed.
        assert_eq!(
            pick_summon_source(&candidates, &uses, &enchants, 20_000),
            None
        );
    }

    #[test]
    fn an_ability_spawned_spirit_names_the_item_that_declares_it() {
        // A Kimono's spirits are created by *any* ability activation, so the item
        // credited is the Kimono itself, never the ability that was pressed.
        let mut uses = VecDeque::new();
        uses.push_back(ability_use_with(5_000, 999, &[]));
        let candidates = [SummonCandidate {
            slot: 2,
            item: 0x4ff4,
            trigger: SummonTrigger::OnAbilityUse,
            lifetime_ms: 0,
        }];
        let enchants = [Vec::new(), Vec::new(), vec![6], Vec::new()];
        assert_eq!(
            pick_summon_source(&candidates, &uses, &enchants, 5_500),
            Some((0x4ff4, vec![6])),
            "the declaring item's own enchants name the row, never the pressed ability's"
        );
        // Even when the activation carries enchants of its own, they belong to the
        // ability that was pressed, not to the Kimono.
        let mut other = VecDeque::new();
        other.push_back(ability_use_with(5_000, 999, &[42, 43]));
        assert_eq!(
            pick_summon_source(&candidates, &other, &enchants, 5_500),
            Some((0x4ff4, vec![6])),
            "an unrelated ability's enchants must not split the Kimono's row"
        );
        // With no `lifeTime` declared, the shortest documented summon life bounds
        // the *search*, but with only one item able to spawn the creature the
        // answer is still unambiguous: nothing else could have made it.
        assert_eq!(
            pick_summon_source(&candidates, &uses, &enchants, 12_000),
            Some((0x4ff4, vec![6]))
        );
    }

    #[test]
    fn a_summon_cast_before_the_fight_still_names_its_item() {
        // The activation log is a rolling session log, so a bear summoned before
        // the fight started is still traceable to its mace.
        let mut uses = VecDeque::new();
        uses.push_back(ability_use_with(-4_000, 0x3849, &[]));
        let candidates = [SummonCandidate {
            slot: 1,
            item: 0x3849,
            trigger: SummonTrigger::Use,
            lifetime_ms: 10_000,
        }];
        assert_eq!(
            pick_summon_source(&candidates, &uses, &no_enchants(), 1_000),
            Some((0x3849, Vec::new()))
        );
        // Outside that lifetime the activation no longer counts, but the mace is
        // still the only item that could have made this creature.
        assert_eq!(
            pick_summon_source(&candidates, &uses, &no_enchants(), 20_000),
            Some((0x3849, Vec::new()))
        );
        // Two candidates and a stale activation: nothing is claimed rather than
        // one of them guessed.
        let twin = [
            candidates[0],
            SummonCandidate {
                slot: 2,
                item: 0x9581,
                trigger: SummonTrigger::Use,
                lifetime_ms: 10_000,
            },
        ];
        assert_eq!(
            pick_summon_source(&twin, &uses, &no_enchants(), 20_000),
            None
        );
    }

    #[test]
    fn a_shoot_spawned_summon_falls_back_to_the_single_equipped_item() {
        // Nothing in the activation log triggers a shoot-summon: the one equipped
        // item that declares it is the answer, and only when it is alone.
        let uses = VecDeque::new();
        let candidates = [SummonCandidate {
            slot: 1,
            item: 0x6102,
            trigger: SummonTrigger::OnShoot,
            lifetime_ms: 0,
        }];
        let enchants = vec![Vec::new(), vec![17], Vec::new(), Vec::new()];
        assert_eq!(
            pick_summon_source(&candidates, &uses, &enchants, 1_000),
            Some((0x6102, vec![17]))
        );
        let two = [
            candidates[0],
            SummonCandidate {
                slot: 2,
                item: 0x3849,
                trigger: SummonTrigger::OnShoot,
                lifetime_ms: 0,
            },
        ];
        assert_eq!(pick_summon_source(&two, &uses, &enchants, 1_000), None);
    }
}
