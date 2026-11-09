use std::collections::BTreeMap;
use std::time::{Duration, Instant};

use super::contract::DiscoveryState;
use chrono::{DateTime, Local};
use realmhound_core::GameEvent;

pub const CRYSTAL_TYPE: i32 = 0x9A7;
pub const JAILER_TYPE: i32 = 0xAD31;
/// Totalia the Malevolent, the Halloween realm event boss. She has no chat
/// announcement, so her object update is the only presence signal.
pub const TOTALIA_TYPE: i32 = 0x3CCF;
pub const ABSENCE_TIMEOUT: Duration = Duration::from_secs(30);

#[derive(Debug, Clone)]
pub enum DiscoveryEvent {
    Reset {
        at: Instant,
    },
    Discovered {
        object_id: i32,
        object_type: i32,
        notify: bool,
        timestamp: DateTime<Local>,
    },
    CrystalMessage {
        object_id: Option<i32>,
        notify: bool,
        at: Instant,
        timestamp: DateTime<Local>,
        packet_seen: bool,
        absent_since: Option<Instant>,
    },
    Removed {
        object_id: i32,
        at: Instant,
    },
    Defeated {
        object_id: i32,
        at: Instant,
    },
}

struct TrackedObject {
    object_type: i32,
    present: bool,
    defeated: bool,
    absent_since: Option<Instant>,
    timestamp: DateTime<Local>,
    last_message: Option<Instant>,
    defeated_at: Option<Instant>,
}

#[derive(Default)]
pub struct RealmDiscovery {
    active: bool,
    crystal_discovered: bool,
    objects: BTreeMap<i32, TrackedObject>,
    scope: u64,
    reset_at: Option<Instant>,
    crystal_message_state: Option<DiscoveryState>,
}

impl RealmDiscovery {
    /// Durable per-object state, published outside the bounded event history.
    pub fn state(&self) -> Vec<DiscoveryState> {
        let mut states: Vec<_> = self
            .objects
            .iter()
            .map(|(id, object)| DiscoveryState {
                scope: self.scope,
                object_id: Some(*id),
                object_type: object.object_type,
                timestamp: object.timestamp,
                present: object.present,
                defeated: object.defeated,
                packet_seen: true,
                absent_since: object.absent_since,
                last_message: object.last_message,
                defeated_at: object.defeated_at,
            })
            .collect();
        if !states.iter().any(|state| state.object_type == CRYSTAL_TYPE) {
            states.extend(self.crystal_message_state.clone());
        }
        states
    }

    pub fn scope(&self) -> u64 {
        self.scope
    }

    pub fn reset_at(&self) -> Option<Instant> {
        self.reset_at
    }

    pub fn reset(&mut self, active: bool) -> DiscoveryEvent {
        let at = Instant::now();
        *self = Self {
            active,
            scope: self.scope.wrapping_add(1),
            reset_at: Some(at),
            ..Self::default()
        };
        DiscoveryEvent::Reset { at }
    }

    pub fn on_event(&mut self, event: &GameEvent) -> Vec<DiscoveryEvent> {
        let mut events = Vec::new();
        if let GameEvent::MapChanged {
            max_realm_score, ..
        } = event
        {
            events.push(self.reset(*max_realm_score > 0));
            return events;
        }
        if !self.active {
            return events;
        }
        if let GameEvent::TextReceived(text) = event {
            if text.recipient.is_empty() && text.text == "..." {
                if let Some(jailer) = self
                    .objects
                    .get_mut(&text.object_id)
                    .filter(|object| object.object_type == JAILER_TYPE && !object.defeated)
                {
                    jailer.defeated = true;
                    let at = Instant::now();
                    jailer.defeated_at = Some(at);
                    events.push(DiscoveryEvent::Defeated {
                        object_id: text.object_id,
                        at,
                    });
                }
            }
        }
        let GameEvent::UpdateReceived(update, _) = event else {
            return events;
        };
        let now = Instant::now();
        for object in &update.new_objects {
            let object_type = i32::from(object.object_type);
            if !matches!(object_type, CRYSTAL_TYPE | JAILER_TYPE | TOTALIA_TYPE) {
                continue;
            }
            let object_id = object.object_id();
            let known = self.objects.contains_key(&object_id);
            let tracked = self.objects.entry(object_id).or_insert(TrackedObject {
                object_type,
                present: false,
                defeated: false,
                absent_since: None,
                timestamp: Local::now(),
                last_message: (object_type == CRYSTAL_TYPE)
                    .then(|| {
                        self.crystal_message_state
                            .as_ref()
                            .and_then(|state| state.last_message)
                    })
                    .flatten(),
                defeated_at: None,
            });
            if !tracked.present && !tracked.defeated {
                let absent_for = tracked.absent_since.take().map(|at| now.duration_since(at));
                let first_discovery =
                    !known && (object_type != CRYSTAL_TYPE || !self.crystal_discovered);
                let notify =
                    first_discovery || absent_for.is_some_and(|elapsed| elapsed >= ABSENCE_TIMEOUT);
                if object_type == CRYSTAL_TYPE {
                    self.crystal_discovered = true;
                }
                tracked.timestamp = Local::now();
                events.push(DiscoveryEvent::Discovered {
                    object_id,
                    object_type,
                    notify,
                    timestamp: tracked.timestamp,
                });
            }
            tracked.present = true;
        }
        for object_id in &update.drops {
            if let Some(object) = self.objects.get_mut(object_id) {
                if object.present {
                    object.absent_since = Some(now);
                    events.push(DiscoveryEvent::Removed {
                        object_id: *object_id,
                        at: now,
                    });
                }
                object.present = false;
            }
        }
        events
    }

    pub fn crystal_message(&mut self, sender_id: i32) -> Option<DiscoveryEvent> {
        if !self.active {
            return None;
        }
        let notify = !self.crystal_discovered;
        self.crystal_discovered = true;
        let at = Instant::now();
        let timestamp = Local::now();
        let crystal = self
            .objects
            .iter_mut()
            .find(|(_, object)| object.object_type == CRYSTAL_TYPE);
        let (object_id, packet_seen, absent_since) = if let Some((&id, crystal)) = crystal {
            crystal.last_message = Some(at);
            crystal.timestamp = timestamp;
            (Some(id), true, crystal.absent_since)
        } else {
            let object_id = (sender_id >= 0).then_some(sender_id);
            self.crystal_message_state = Some(DiscoveryState {
                scope: self.scope,
                object_id,
                object_type: CRYSTAL_TYPE,
                timestamp,
                present: false,
                defeated: false,
                packet_seen: false,
                absent_since: None,
                last_message: Some(at),
                defeated_at: None,
            });
            (object_id, false, None)
        };
        Some(DiscoveryEvent::CrystalMessage {
            object_id,
            notify,
            at,
            timestamp,
            packet_seen,
            absent_since,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use realmhound_core::protocol::data::{ObjectData, ObjectStatusData, WorldPosData};
    use realmhound_core::protocol::packets::{TextPacket, UpdatePacket};

    fn update(object_type: i32, object_ids: &[i32], drops: &[i32]) -> GameEvent {
        GameEvent::UpdateReceived(
            UpdatePacket {
                pos: WorldPosData { x: 0.0, y: 0.0 },
                level_type: 0,
                tiles: Vec::new(),
                new_objects: object_ids
                    .iter()
                    .map(|&object_id| ObjectData {
                        object_type: u16::try_from(object_type).unwrap(),
                        status: ObjectStatusData {
                            object_id,
                            pos: WorldPosData { x: 0.0, y: 0.0 },
                            stats: Vec::new(),
                        },
                    })
                    .collect(),
                drops: drops.to_vec(),
            },
            0,
        )
    }

    fn text(object_id: i32, message: &str) -> GameEvent {
        GameEvent::TextReceived(TextPacket {
            name: "#Spectral Jailer".to_string(),
            object_id,
            num_stars: 0,
            bubble_time: 3,
            recipient: String::new(),
            text: message.to_string(),
            clean_text: String::new(),
            is_supporter: false,
            star_background: 0,
        })
    }

    #[test]
    fn discovery_rearms_only_after_continuous_absence() {
        for object_type in [CRYSTAL_TYPE, JAILER_TYPE, TOTALIA_TYPE] {
            let mut tracker = RealmDiscovery::default();
            tracker.reset(true);
            let present = update(object_type, &[1], &[]);
            let removed = update(object_type, &[], &[1]);

            assert!(matches!(
                tracker.on_event(&present).as_slice(),
                [DiscoveryEvent::Discovered { notify: true, .. }]
            ));
            assert!(tracker.on_event(&present).is_empty());
            assert!(matches!(
                tracker.on_event(&removed).as_slice(),
                [DiscoveryEvent::Removed { object_id: 1, .. }]
            ));

            let short_absence = Instant::now() - ABSENCE_TIMEOUT / 2;
            tracker.objects.get_mut(&1).unwrap().absent_since = Some(short_absence);
            assert!(tracker.on_event(&removed).is_empty());
            assert_eq!(tracker.objects[&1].absent_since, Some(short_absence));
            assert!(matches!(
                tracker.on_event(&present).as_slice(),
                [DiscoveryEvent::Discovered { notify: false, .. }]
            ));
            assert_eq!(tracker.objects[&1].absent_since, None);

            tracker.on_event(&removed);
            let long_absence = Instant::now() - ABSENCE_TIMEOUT;
            tracker.objects.get_mut(&1).unwrap().absent_since = Some(long_absence);
            if object_type == CRYSTAL_TYPE {
                assert!(matches!(
                    tracker.crystal_message(1),
                    Some(DiscoveryEvent::CrystalMessage {
                        notify: false,
                        absent_since: Some(at),
                        ..
                    }) if at == long_absence
                ));
            }
            assert!(tracker.on_event(&removed).is_empty());
            assert_eq!(tracker.objects[&1].absent_since, Some(long_absence));
            assert!(matches!(
                tracker.on_event(&present).as_slice(),
                [DiscoveryEvent::Discovered { notify: true, .. }]
            ));
            assert!(tracker.on_event(&present).is_empty());
        }
    }

    #[test]
    fn jailer_defeat_is_instance_scoped_and_latched_until_reset() {
        let mut tracker = RealmDiscovery::default();
        tracker.reset(true);
        let present = update(JAILER_TYPE, &[1, 2], &[]);
        assert!(matches!(
            tracker.on_event(&present).as_slice(),
            [
                DiscoveryEvent::Discovered {
                    object_id: 1,
                    notify: true,
                    ..
                },
                DiscoveryEvent::Discovered {
                    object_id: 2,
                    notify: true,
                    ..
                }
            ]
        ));
        assert!(tracker.on_event(&present).is_empty());
        for phase in ["...?", "...!"] {
            assert!(tracker.on_event(&text(1, phase)).is_empty());
        }
        assert!(matches!(
            tracker.on_event(&text(1, "...")).as_slice(),
            [DiscoveryEvent::Defeated { object_id: 1, .. }]
        ));
        assert!(tracker.objects[&1].defeated);
        assert!(!tracker.objects[&2].defeated);
        assert!(tracker.on_event(&text(1, "...")).is_empty());
        tracker.on_event(&update(JAILER_TYPE, &[], &[1]));
        assert!(tracker.on_event(&update(JAILER_TYPE, &[1], &[])).is_empty());

        tracker.on_event(&update(JAILER_TYPE, &[], &[2]));
        assert!(matches!(
            tracker.on_event(&text(2, "...")).as_slice(),
            [DiscoveryEvent::Defeated { object_id: 2, .. }]
        ));
        assert!(tracker.on_event(&update(JAILER_TYPE, &[2], &[])).is_empty());

        tracker.reset(true);
        assert!(matches!(
            tracker.on_event(&present).as_slice(),
            [
                DiscoveryEvent::Discovered { notify: true, .. },
                DiscoveryEvent::Discovered { notify: true, .. }
            ]
        ));
        assert!(tracker.objects.values().all(|object| !object.defeated));
    }

    #[test]
    fn published_discovery_keeps_original_lifecycle_times_and_scope() {
        let mut tracker = RealmDiscovery::default();
        tracker.reset(true);
        let scope = tracker.scope();
        let message = tracker.crystal_message(-1).unwrap();
        let message_at = match message {
            DiscoveryEvent::CrystalMessage { at, .. } => at,
            other => panic!("expected crystal message, got {other:?}"),
        };
        let message_only = tracker.state();
        assert_eq!(message_only[0].object_id, None);
        assert!(!message_only[0].packet_seen);
        tracker.on_event(&update(CRYSTAL_TYPE, &[7], &[]));
        tracker.on_event(&update(CRYSTAL_TYPE, &[], &[7]));
        let published = tracker.state();
        assert_eq!(published[0].scope, scope);
        assert_eq!(published[0].last_message, Some(message_at));
        assert_eq!(published[0].absent_since, tracker.objects[&7].absent_since);
        assert!(published[0].absent_since.is_some());

        tracker.on_event(&update(JAILER_TYPE, &[8], &[]));
        tracker.on_event(&text(8, "..."));
        let defeated = tracker
            .state()
            .into_iter()
            .find(|state| state.object_id == Some(8))
            .unwrap();
        assert!(defeated.defeated);
        assert_eq!(defeated.defeated_at, tracker.objects[&8].defeated_at);
        assert!(defeated.defeated_at.is_some());
        tracker.reset(true);
        assert_ne!(tracker.scope(), scope);
        assert!(tracker.state().is_empty());
    }
}
