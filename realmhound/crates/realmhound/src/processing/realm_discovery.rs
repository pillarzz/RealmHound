use std::collections::BTreeMap;
use std::time::{Duration, Instant};

use chrono::{DateTime, Local};
use realmhound_core::GameEvent;

pub const CRYSTAL_TYPE: i32 = 0x9A7;
pub const JAILER_TYPE: i32 = 0xAD31;
pub const ABSENCE_TIMEOUT: Duration = Duration::from_secs(30);

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
}

#[derive(Default)]
pub struct RealmDiscovery {
    active: bool,
    crystal_discovered: bool,
    objects: BTreeMap<i32, TrackedObject>,
}

impl RealmDiscovery {
    pub fn reset(&mut self, active: bool) -> DiscoveryEvent {
        *self = Self {
            active,
            ..Self::default()
        };
        DiscoveryEvent::Reset { at: Instant::now() }
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
                    events.push(DiscoveryEvent::Defeated {
                        object_id: text.object_id,
                        at: Instant::now(),
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
            if !matches!(object_type, CRYSTAL_TYPE | JAILER_TYPE) {
                continue;
            }
            let object_id = object.object_id();
            let known = self.objects.contains_key(&object_id);
            let tracked = self.objects.entry(object_id).or_insert(TrackedObject {
                object_type,
                present: false,
                defeated: false,
                absent_since: None,
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
                events.push(DiscoveryEvent::Discovered {
                    object_id,
                    object_type,
                    notify,
                    timestamp: Local::now(),
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
        let crystal = self
            .objects
            .iter()
            .find(|(_, object)| object.object_type == CRYSTAL_TYPE);
        let object_id = crystal
            .map(|(&id, _)| id)
            .or_else(|| (sender_id >= 0).then_some(sender_id));
        Some(DiscoveryEvent::CrystalMessage {
            object_id,
            notify,
            at: Instant::now(),
            timestamp: Local::now(),
            packet_seen: crystal.is_some(),
            absent_since: crystal.and_then(|(_, object)| object.absent_since),
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
        for object_type in [CRYSTAL_TYPE, JAILER_TYPE] {
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
}
