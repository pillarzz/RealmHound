//! JSON export of stored combat records.
//!
//! Serde DTOs that mirror the combat DB records so selected fights can be
//! written to a shareable file for investigation. A dedicated DTO decouples the
//! wire format from the DB schema, so a schema change never silently alters the
//! export shape.

use serde::Serialize;

use super::database::{EncounterRecord, FightRecord, ParticipantRecord, SCHEMA_VERSION};

/// Export JSON shape version. Bump when the DTO layout changes.
pub const EXPORT_FORMAT_VERSION: u32 = 1;

/// Top-level export envelope written to disk.
#[derive(Debug, Clone, Serialize)]
pub struct ExportBundle {
    /// Export shape version (see [`EXPORT_FORMAT_VERSION`]).
    pub format_version: u32,
    /// RealmHound app version that produced the file.
    pub app_version: String,
    /// Combat DB schema version the records were read from.
    pub schema_version: i32,
    /// Export timestamp (RFC 3339, UTC).
    pub exported_at: String,
    /// Number of standalone fights in `fights`.
    pub fight_count: usize,
    /// Number of grouped encounters in `encounters`.
    pub encounter_count: usize,
    /// Selected standalone fights.
    pub fights: Vec<FightDto>,
    /// Selected multi-phase encounters.
    pub encounters: Vec<EncounterDto>,
}

/// A standalone fight record.
#[derive(Debug, Clone, Serialize)]
pub struct FightDto {
    pub id: i64,
    pub started_at: i64,
    pub ended_at: i64,
    pub duration_ms: i64,
    pub dungeon: String,
    pub map_seed: i32,
    pub boss_object_type: i32,
    pub boss_name: String,
    pub boss_max_hp: i32,
    pub boss_start_hp: i32,
    pub local_object_id: i32,
    pub local_char_id: i32,
    pub killed: bool,
    pub participants: Vec<ParticipantDto>,
}

/// A grouped multi-phase encounter record.
#[derive(Debug, Clone, Serialize)]
pub struct EncounterDto {
    pub run_id: String,
    pub encounter_id: String,
    pub display_name: String,
    pub dungeon: String,
    pub started_at: i64,
    pub ended_at: i64,
    pub killed: bool,
    pub anchor_object_type: i32,
    pub phases: Vec<FightDto>,
    pub roster: Vec<ParticipantDto>,
}

/// A single participant's contribution.
#[derive(Debug, Clone, Serialize)]
pub struct ParticipantDto {
    pub object_id: i32,
    pub object_type: i32,
    pub skin_id: i32,
    pub tex1: u32,
    pub tex2: u32,
    pub name: String,
    pub equipment: [i32; 4],
    pub equipment_enchants: [Vec<u16>; 4],
    pub damage: i64,
    pub hits: i64,
    pub is_local: bool,
    pub provenance: String,
    pub end_status: String,
    pub grave_type: i32,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pet: Option<PetDto>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub damage_taken: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub damage_taken_provenance: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub damage_blocked: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub guarded_damage: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub guarded_hits: Option<i64>,
}

/// A participant's active pet.
#[derive(Debug, Clone, Serialize)]
pub struct PetDto {
    pub pet_type: i32,
    pub abilities: Vec<i32>,
}

impl From<&ParticipantRecord> for ParticipantDto {
    fn from(p: &ParticipantRecord) -> Self {
        ParticipantDto {
            object_id: p.object_id,
            object_type: p.object_type,
            skin_id: p.skin_id,
            tex1: p.tex1,
            tex2: p.tex2,
            name: p.name.clone(),
            equipment: p.equipment,
            equipment_enchants: p.equipment_enchants.clone(),
            damage: p.damage,
            hits: p.hits,
            is_local: p.is_local,
            provenance: p.provenance.as_str().to_string(),
            end_status: p.end_status.as_str().to_string(),
            grave_type: p.end_status.grave_type(),
            pet: p.pet.as_ref().map(|pet| PetDto {
                pet_type: pet.pet_type,
                abilities: pet.abilities.clone(),
            }),
            damage_taken: p.damage_taken,
            damage_taken_provenance: p
                .damage_taken
                .map(|_| p.damage_taken_provenance.as_str().to_string()),
            damage_blocked: p.damage_blocked,
            guarded_damage: p.guarded_damage,
            guarded_hits: p.guarded_hits,
        }
    }
}

impl From<&FightRecord> for FightDto {
    fn from(f: &FightRecord) -> Self {
        FightDto {
            id: f.id,
            started_at: f.started_at,
            ended_at: f.ended_at,
            duration_ms: f.duration_ms(),
            dungeon: f.dungeon.clone(),
            map_seed: f.map_seed,
            boss_object_type: f.boss_object_type,
            boss_name: f.boss_name.clone(),
            boss_max_hp: f.boss_max_hp,
            boss_start_hp: f.boss_start_hp,
            local_object_id: f.local_object_id,
            local_char_id: f.local_char_id,
            killed: f.killed,
            participants: f.participants.iter().map(ParticipantDto::from).collect(),
        }
    }
}

impl From<&EncounterRecord> for EncounterDto {
    fn from(e: &EncounterRecord) -> Self {
        EncounterDto {
            run_id: e.run_id.clone(),
            encounter_id: e.encounter_id.clone(),
            display_name: e.display_name.clone(),
            dungeon: e.dungeon.clone(),
            started_at: e.started_at,
            ended_at: e.ended_at,
            killed: e.killed,
            anchor_object_type: e.anchor_object_type,
            phases: e.phases.iter().map(FightDto::from).collect(),
            roster: e.roster.iter().map(ParticipantDto::from).collect(),
        }
    }
}

/// Builds an [`ExportBundle`] from selected records.
pub fn build_bundle(
    app_version: &str,
    fights: &[FightRecord],
    encounters: &[EncounterRecord],
) -> ExportBundle {
    ExportBundle {
        format_version: EXPORT_FORMAT_VERSION,
        app_version: app_version.to_string(),
        schema_version: SCHEMA_VERSION,
        exported_at: chrono::Utc::now().to_rfc3339(),
        fight_count: fights.len(),
        encounter_count: encounters.len(),
        fights: fights.iter().map(FightDto::from).collect(),
        encounters: encounters.iter().map(EncounterDto::from).collect(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::combat::{DamageProvenance, DamageTakenProvenance, ParticipantEndStatus};

    fn sample_participant(damage_taken: Option<i64>) -> ParticipantRecord {
        ParticipantRecord {
            object_id: 1,
            object_type: 782,
            skin_id: 12,
            tex1: 3,
            tex2: 4,
            name: "Tester".to_string(),
            equipment: [100, 101, 102, 103],
            equipment_enchants: [vec![1], vec![], vec![2, 3], vec![]],
            damage: 5000,
            hits: 42,
            is_local: true,
            provenance: DamageProvenance::SelfComputed,
            end_status: ParticipantEndStatus::Died { grave_type: 55 },
            pet: None,
            damage_taken,
            damage_taken_provenance: DamageTakenProvenance::Estimated,
            damage_blocked: None,
            guarded_damage: Some(120),
            guarded_hits: Some(2),
        }
    }

    fn sample_fight() -> FightRecord {
        FightRecord {
            id: 7,
            started_at: 1000,
            ended_at: 4000,
            dungeon: "The Shatters".to_string(),
            dungeon_entered_at: None,
            map_seed: 42,
            boss_object_type: 5000,
            boss_name: "The Forgotten King".to_string(),
            boss_max_hp: 200000,
            boss_start_hp: 200000,
            local_object_id: 1,
            local_char_id: 99,
            killed: true,
            local_close_calls: 0,
            aux_member_count: None,
            participants: vec![sample_participant(Some(300))],
        }
    }

    #[test]
    fn bundle_counts_and_envelope() {
        let fights = vec![sample_fight()];
        let bundle = build_bundle("1.2.3", &fights, &[]);
        assert_eq!(bundle.format_version, EXPORT_FORMAT_VERSION);
        assert_eq!(bundle.app_version, "1.2.3");
        assert_eq!(bundle.schema_version, SCHEMA_VERSION);
        assert_eq!(bundle.fight_count, 1);
        assert_eq!(bundle.encounter_count, 0);
        let fight = &bundle.fights[0];
        assert_eq!(fight.duration_ms, 3000);
        let p = &fight.participants[0];
        assert_eq!(p.provenance, "self_computed");
        assert_eq!(p.end_status, "died");
        assert_eq!(p.grave_type, 55);
        assert_eq!(p.damage_taken_provenance.as_deref(), Some("estimated"));
    }

    #[test]
    fn damage_taken_provenance_omitted_when_value_absent() {
        let mut fight = sample_fight();
        fight.participants = vec![sample_participant(None)];
        let bundle = build_bundle("1.0.0", &[fight], &[]);
        assert_eq!(bundle.fights[0].participants[0].damage_taken, None);
        assert_eq!(
            bundle.fights[0].participants[0].damage_taken_provenance, None,
            "provenance must be omitted when damage_taken is unavailable"
        );
    }

    #[test]
    fn serializes_to_json() {
        let bundle = build_bundle("1.0.0", &[sample_fight()], &[]);
        let json = serde_json::to_string(&bundle).expect("serialize");
        assert!(json.contains("\"boss_name\":\"The Forgotten King\""));
        assert!(json.contains("\"format_version\":1"));
    }
}
