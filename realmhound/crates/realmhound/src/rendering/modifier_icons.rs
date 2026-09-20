//! Tier-specific dungeon-modifier icons embedded in the binary.
//!
//! 37 modifier bases each have four tier icons: the 20 boss/minion mods
//! (Elite/Feeble/Ferocious/Glass/Healthy/Resilient/Stone/Tame/Tough/Weak x
//! Boss/Minions) and the 17 player stat mods (Anemia, Armored, Dexterous, ...).
//! They are preloaded by [`crate::rendering::SpriteRenderer`] and drawn as the
//! leading icon of a modifier chip in the Live Feed dungeon callout.

/// One of the 37 modifier bases, identified by its canonical token base
/// (e.g. `WEAKBOSS`, `ARMORED`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ModIconBase {
    // Boss & Minions.
    EliteBoss,
    EliteMinions,
    FeebleBoss,
    FeebleMinions,
    FerociousBoss,
    FerociousMinions,
    GlassBoss,
    GlassMinions,
    HealthyBoss,
    HealthyMinions,
    ResilientBoss,
    ResilientMinions,
    StoneBoss,
    StoneMinions,
    TameBoss,
    TameMinions,
    ToughBoss,
    ToughMinions,
    WeakBoss,
    WeakMinions,
    // Player stat mods.
    Anemia,
    Apathetic,
    Armored,
    Clumsy,
    Dexterous,
    Drained,
    Energetic,
    Exposed,
    Foolish,
    Frenzied,
    Lively,
    Sluggish,
    Speedy,
    Strengthened,
    Vigorous,
    Weak,
    Wise,
}

impl ModIconBase {
    /// Every modifier base, used to preload all tier textures up front.
    pub const ALL: &'static [ModIconBase] = &[
        ModIconBase::EliteBoss,
        ModIconBase::EliteMinions,
        ModIconBase::FeebleBoss,
        ModIconBase::FeebleMinions,
        ModIconBase::FerociousBoss,
        ModIconBase::FerociousMinions,
        ModIconBase::GlassBoss,
        ModIconBase::GlassMinions,
        ModIconBase::HealthyBoss,
        ModIconBase::HealthyMinions,
        ModIconBase::ResilientBoss,
        ModIconBase::ResilientMinions,
        ModIconBase::StoneBoss,
        ModIconBase::StoneMinions,
        ModIconBase::TameBoss,
        ModIconBase::TameMinions,
        ModIconBase::ToughBoss,
        ModIconBase::ToughMinions,
        ModIconBase::WeakBoss,
        ModIconBase::WeakMinions,
        ModIconBase::Anemia,
        ModIconBase::Apathetic,
        ModIconBase::Armored,
        ModIconBase::Clumsy,
        ModIconBase::Dexterous,
        ModIconBase::Drained,
        ModIconBase::Energetic,
        ModIconBase::Exposed,
        ModIconBase::Foolish,
        ModIconBase::Frenzied,
        ModIconBase::Lively,
        ModIconBase::Sluggish,
        ModIconBase::Speedy,
        ModIconBase::Strengthened,
        ModIconBase::Vigorous,
        ModIconBase::Weak,
        ModIconBase::Wise,
    ];

    /// Map a canonical token base (e.g. `WEAKBOSS`, `ARMORED`) to its base.
    pub fn from_canonical_base(base: &str) -> Option<ModIconBase> {
        Some(match base {
            "ELITEBOSS" => ModIconBase::EliteBoss,
            "ELITEMINIONS" => ModIconBase::EliteMinions,
            "FEEBLEBOSS" => ModIconBase::FeebleBoss,
            "FEEBLEMINIONS" => ModIconBase::FeebleMinions,
            "FEROCIOUSBOSS" => ModIconBase::FerociousBoss,
            "FEROCIOUSMINIONS" => ModIconBase::FerociousMinions,
            "GLASSBOSS" => ModIconBase::GlassBoss,
            "GLASSMINIONS" => ModIconBase::GlassMinions,
            "HEALTHYBOSS" => ModIconBase::HealthyBoss,
            "HEALTHYMINIONS" => ModIconBase::HealthyMinions,
            "RESILIENTBOSS" => ModIconBase::ResilientBoss,
            "RESILIENTMINIONS" => ModIconBase::ResilientMinions,
            "STONEBOSS" => ModIconBase::StoneBoss,
            "STONEMINIONS" => ModIconBase::StoneMinions,
            "TAMEBOSS" => ModIconBase::TameBoss,
            "TAMEMINIONS" => ModIconBase::TameMinions,
            "TOUGHBOSS" => ModIconBase::ToughBoss,
            "TOUGHMINIONS" => ModIconBase::ToughMinions,
            "WEAKBOSS" => ModIconBase::WeakBoss,
            "WEAKMINIONS" => ModIconBase::WeakMinions,
            "ANEMIA" => ModIconBase::Anemia,
            "APATHETIC" => ModIconBase::Apathetic,
            "ARMORED" => ModIconBase::Armored,
            "CLUMSY" => ModIconBase::Clumsy,
            "DEXTEROUS" => ModIconBase::Dexterous,
            "DRAINED" => ModIconBase::Drained,
            "ENERGETIC" => ModIconBase::Energetic,
            "EXPOSED" => ModIconBase::Exposed,
            "FOOLISH" => ModIconBase::Foolish,
            "FRENZIED" => ModIconBase::Frenzied,
            "LIVELY" => ModIconBase::Lively,
            "SLUGGISH" => ModIconBase::Sluggish,
            "SPEEDY" => ModIconBase::Speedy,
            "STRENGTHENED" => ModIconBase::Strengthened,
            "VIGOROUS" => ModIconBase::Vigorous,
            "WEAK" => ModIconBase::Weak,
            "WISE" => ModIconBase::Wise,
            _ => return None,
        })
    }
}

/// A tier-specific modifier icon (`base` + `tier` in 1..=4). Construct via
/// [`ModTierIcon::new`] so invalid tiers are rejected.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct ModTierIcon {
    base: ModIconBase,
    tier: u8,
}

impl ModTierIcon {
    /// Build an icon for `base` at `tier`, returning `None` unless `tier` is 1..=4.
    pub fn new(base: ModIconBase, tier: u8) -> Option<ModTierIcon> {
        if (1..=4).contains(&tier) {
            Some(ModTierIcon { base, tier })
        } else {
            None
        }
    }

    /// Resolve a modifier token (e.g. `WEAKBOSS_3`, `ARMORED_2`) to its icon, if
    /// it is a known tier-icon modifier with a valid tier.
    pub fn from_token(token: &str) -> Option<ModTierIcon> {
        let (base, tier) = realmhound_core::dungeon_modifiers::tier_icon_base_tier(token)?;
        let base = ModIconBase::from_canonical_base(&base)?;
        ModTierIcon::new(base, tier)
    }

    /// The raw PNG bytes embedded for this icon.
    pub fn bytes(self) -> &'static [u8] {
        macro_rules! tier_bytes {
            ($stem:literal) => {
                match self.tier {
                    1 => include_bytes!(concat!("../../assets/mods/tier/", $stem, "_1.png")),
                    2 => include_bytes!(concat!("../../assets/mods/tier/", $stem, "_2.png")),
                    3 => include_bytes!(concat!("../../assets/mods/tier/", $stem, "_3.png")),
                    _ => include_bytes!(concat!("../../assets/mods/tier/", $stem, "_4.png")),
                }
            };
        }
        match self.base {
            ModIconBase::EliteBoss => tier_bytes!("eliteboss"),
            ModIconBase::EliteMinions => tier_bytes!("eliteminions"),
            ModIconBase::FeebleBoss => tier_bytes!("feebleboss"),
            ModIconBase::FeebleMinions => tier_bytes!("feebleminions"),
            ModIconBase::FerociousBoss => tier_bytes!("ferociousboss"),
            ModIconBase::FerociousMinions => tier_bytes!("ferociousminions"),
            ModIconBase::GlassBoss => tier_bytes!("glassboss"),
            ModIconBase::GlassMinions => tier_bytes!("glassminions"),
            ModIconBase::HealthyBoss => tier_bytes!("healthyboss"),
            ModIconBase::HealthyMinions => tier_bytes!("healthyminions"),
            ModIconBase::ResilientBoss => tier_bytes!("resilientboss"),
            ModIconBase::ResilientMinions => tier_bytes!("resilientminions"),
            ModIconBase::StoneBoss => tier_bytes!("stoneboss"),
            ModIconBase::StoneMinions => tier_bytes!("stoneminions"),
            ModIconBase::TameBoss => tier_bytes!("tameboss"),
            ModIconBase::TameMinions => tier_bytes!("tameminions"),
            ModIconBase::ToughBoss => tier_bytes!("toughboss"),
            ModIconBase::ToughMinions => tier_bytes!("toughminions"),
            ModIconBase::WeakBoss => tier_bytes!("weakboss"),
            ModIconBase::WeakMinions => tier_bytes!("weakminions"),
            ModIconBase::Anemia => tier_bytes!("anemia"),
            ModIconBase::Apathetic => tier_bytes!("apathetic"),
            ModIconBase::Armored => tier_bytes!("armored"),
            ModIconBase::Clumsy => tier_bytes!("clumsy"),
            ModIconBase::Dexterous => tier_bytes!("dexterous"),
            ModIconBase::Drained => tier_bytes!("drained"),
            ModIconBase::Energetic => tier_bytes!("energetic"),
            ModIconBase::Exposed => tier_bytes!("exposed"),
            ModIconBase::Foolish => tier_bytes!("foolish"),
            ModIconBase::Frenzied => tier_bytes!("frenzied"),
            ModIconBase::Lively => tier_bytes!("lively"),
            ModIconBase::Sluggish => tier_bytes!("sluggish"),
            ModIconBase::Speedy => tier_bytes!("speedy"),
            ModIconBase::Strengthened => tier_bytes!("strengthened"),
            ModIconBase::Vigorous => tier_bytes!("vigorous"),
            ModIconBase::Weak => tier_bytes!("weak"),
            ModIconBase::Wise => tier_bytes!("wise"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_base_tier_decodes() {
        for &base in ModIconBase::ALL {
            for tier in 1..=4u8 {
                let icon = ModTierIcon::new(base, tier).expect("valid tier");
                let img = image::load_from_memory(icon.bytes())
                    .unwrap_or_else(|_| panic!("decode {:?} tier {}", base, tier));
                assert!(img.width() > 0 && img.height() > 0);
            }
        }
    }

    #[test]
    fn from_token_maps_known_mods() {
        assert_eq!(
            ModTierIcon::from_token("WEAKBOSS_3"),
            ModTierIcon::new(ModIconBase::WeakBoss, 3)
        );
        assert_eq!(
            ModTierIcon::from_token("GLASSMINIONS_1"),
            ModTierIcon::new(ModIconBase::GlassMinions, 1)
        );
        // Player stat mod, distinct from the boss/minion WEAK* bases.
        assert_eq!(
            ModTierIcon::from_token("WEAK_2"),
            ModTierIcon::new(ModIconBase::Weak, 2)
        );
        assert_eq!(
            ModTierIcon::from_token("ARMORED_4"),
            ModTierIcon::new(ModIconBase::Armored, 4)
        );
        assert_eq!(ModTierIcon::from_token("LOOTING"), None);
    }

    #[test]
    fn invalid_tier_rejected() {
        assert_eq!(ModTierIcon::new(ModIconBase::WeakBoss, 0), None);
        assert_eq!(ModTierIcon::new(ModIconBase::WeakBoss, 5), None);
    }

    #[test]
    fn all_core_bases_map_to_icon_base() {
        for &base in realmhound_core::dungeon_modifiers::TIER_ICON_BASES {
            assert!(
                ModIconBase::from_canonical_base(base).is_some(),
                "core base {base} has no ModIconBase"
            );
        }
    }
}
