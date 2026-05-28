//! Campaign data layer: loads `data/poe2_campaign.json` plus optional local
//! override, indexed by normalized zone name. Mirrors `python/campaign.py`.

use std::collections::HashMap;
use std::fs;
use std::path::Path;

use anyhow::{Context, Result};
use serde::Deserialize;

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
pub struct Task {
    pub description: String,
    #[serde(default)]
    pub reward: String,
    #[serde(default)]
    pub optional: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ZoneEntry {
    pub zone: String,
    pub act: String,
    pub tasks: Vec<Task>,
    pub next_zone: Option<String>,
}

pub type CampaignGuide = HashMap<String, ZoneEntry>;

#[derive(Debug, Deserialize)]
struct RawCampaign {
    acts: Vec<RawAct>,
}

#[derive(Debug, Deserialize)]
struct RawAct {
    act: String,
    zones: Vec<RawZone>,
}

#[derive(Debug, Deserialize)]
struct RawZone {
    zone: String,
    #[serde(default)]
    tasks: Vec<Task>,
    #[serde(default)]
    next_zone: Option<String>,
}

/// Loads the campaign guide from an embedded default JSON, then overlays
/// an optional on-disk override file. The override may add new zones or
/// fully replace existing entries by normalized zone name.
pub fn load_campaign_guide(
    default_json: &str,
    override_path: Option<&Path>,
) -> Result<CampaignGuide> {
    let mut guide = CampaignGuide::new();
    load_str_into(&mut guide, default_json, "<embedded>")?;
    if let Some(p) = override_path
        && p.exists()
    {
        let content = fs::read_to_string(p)
            .with_context(|| format!("reading campaign data: {}", p.display()))?;
        load_str_into(&mut guide, &content, &p.display().to_string())?;
    }
    Ok(guide)
}

fn load_str_into(guide: &mut CampaignGuide, content: &str, source: &str) -> Result<()> {
    let raw: RawCampaign = serde_json::from_str(content)
        .with_context(|| format!("parsing campaign data: {source}"))?;

    for act_block in raw.acts {
        let act_label = act_block.act;
        let zones = act_block.zones;
        let next_names: Vec<Option<String>> = (0..zones.len())
            .map(|i| zones.get(i + 1).map(|z| z.zone.clone()))
            .collect();

        for (i, zone) in zones.into_iter().enumerate() {
            let next_zone = zone.next_zone.or_else(|| next_names[i].clone());
            let key = normalize(&zone.zone);
            let entry = ZoneEntry {
                zone: zone.zone,
                act: act_label.clone(),
                tasks: zone.tasks,
                next_zone,
            };
            guide.insert(key, entry);
        }
    }
    Ok(())
}

pub fn lookup_zone<'a>(guide: &'a CampaignGuide, zone_name: &str) -> Option<&'a ZoneEntry> {
    guide.get(&normalize(zone_name))
}

#[cfg_attr(not(test), allow(dead_code))]
pub fn all_zones(guide: &CampaignGuide) -> Vec<String> {
    guide.values().map(|e| e.zone.clone()).collect()
}

fn normalize(name: &str) -> String {
    name.trim().to_lowercase()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;
    use std::sync::OnceLock;

    const DEFAULT_JSON: &str = include_str!("../data/poe2_campaign.json");

    fn guide() -> &'static CampaignGuide {
        static GUIDE: OnceLock<CampaignGuide> = OnceLock::new();
        GUIDE.get_or_init(|| {
            load_campaign_guide(DEFAULT_JSON, None).expect("load default campaign guide")
        })
    }

    fn require(zone_name: &str) -> &'static ZoneEntry {
        lookup_zone(guide(), zone_name).unwrap_or_else(|| panic!("zone not found: {zone_name}"))
    }

    fn write_override(dir: impl AsRef<Path>, contents: &str) -> PathBuf {
        let path = dir.as_ref().join("poe2_campaign.local.json");
        fs::write(&path, contents).expect("write override");
        path
    }

    #[test]
    fn no_duplicate_zone_names_in_default_guide() {
        // The loader inserts into a HashMap keyed by normalized name, so a
        // duplicate (including a "The "/case variant of an existing zone)
        // silently overwrites the earlier entry. Compare the raw zone count to
        // the loaded entry count to catch that.
        let v: serde_json::Value = serde_json::from_str(DEFAULT_JSON).unwrap();
        let raw_total: usize = v["acts"]
            .as_array()
            .unwrap()
            .iter()
            .map(|a| a["zones"].as_array().unwrap().len())
            .sum();
        assert_eq!(
            raw_total,
            guide().len(),
            "duplicate normalized zone name collapsed {} raw entries into {}",
            raw_total,
            guide().len()
        );
    }

    #[test]
    fn all_next_zones_resolve_to_a_known_zone() {
        // A dangling next_zone (typo / "The " mismatch) leaves the "Next:"
        // footer pointing at a zone the guide can't find.
        for entry in guide().values() {
            if let Some(next) = &entry.next_zone {
                assert!(
                    lookup_zone(guide(), next).is_some(),
                    "zone {:?} has next_zone {:?} which matches no known zone",
                    entry.zone,
                    next
                );
            }
        }
    }

    #[test]
    fn no_empty_zone_act_or_task_fields() {
        for entry in guide().values() {
            assert!(
                !entry.zone.trim().is_empty(),
                "found a zone with an empty name"
            );
            assert!(
                !entry.act.trim().is_empty(),
                "zone {:?} has an empty act label",
                entry.zone
            );
            for task in &entry.tasks {
                assert!(
                    !task.description.trim().is_empty(),
                    "zone {:?} has a task with an empty description",
                    entry.zone
                );
            }
        }
    }

    #[test]
    fn exact_match() {
        assert_eq!(require("Clearfell").zone, "Clearfell");
    }

    #[test]
    fn case_insensitive() {
        assert!(lookup_zone(guide(), "clearfell").is_some());
        assert!(lookup_zone(guide(), "CLEARFELL").is_some());
        assert!(lookup_zone(guide(), "ClearFell").is_some());
    }

    #[test]
    fn leading_trailing_whitespace() {
        assert!(lookup_zone(guide(), "  Clearfell  ").is_some());
    }

    #[test]
    fn unknown_zone_returns_none() {
        assert!(lookup_zone(guide(), "Wraeclast Town").is_none());
        assert!(lookup_zone(guide(), "").is_none());
        assert!(lookup_zone(guide(), "   ").is_none());
    }

    #[test]
    fn all_acts_have_zones() {
        assert!(!all_zones(guide()).is_empty());
    }

    #[test]
    fn all_listed_zones_are_lookupable() {
        for zone_name in all_zones(guide()) {
            assert!(
                lookup_zone(guide(), &zone_name).is_some(),
                "Zone not found: {zone_name}"
            );
        }
    }

    #[test]
    fn override_replaces_matching_zone() {
        let tmp = tempdir();
        let override_path = write_override(
            &tmp,
            r#"{
                "acts": [
                    {
                        "act": "My Act 1 Notes",
                        "zones": [
                            {
                                "zone": "Clearfell",
                                "next_zone": "The Grelwood",
                                "tasks": [
                                    {
                                        "description": "My custom Clearfell note",
                                        "reward": "Personal route",
                                        "optional": true
                                    }
                                ]
                            }
                        ]
                    }
                ]
            }"#,
        );

        let g = load_campaign_guide(DEFAULT_JSON, Some(&override_path)).unwrap();
        let entry = lookup_zone(&g, "Clearfell").expect("clearfell present");
        assert_eq!(entry.act, "My Act 1 Notes");
        assert_eq!(entry.next_zone.as_deref(), Some("The Grelwood"));
        let descriptions: Vec<&str> = entry.tasks.iter().map(|t| t.description.as_str()).collect();
        assert_eq!(descriptions, vec!["My custom Clearfell note"]);
    }

    #[test]
    fn override_can_add_new_local_zone() {
        let tmp = tempdir();
        let override_path = write_override(
            &tmp,
            r#"{
                "acts": [
                    {
                        "act": "Custom Notes",
                        "zones": [
                            {
                                "zone": "My Custom Zone",
                                "tasks": [
                                    {
                                        "description": "Remember this weird layout",
                                        "reward": "",
                                        "optional": false
                                    }
                                ]
                            }
                        ]
                    }
                ]
            }"#,
        );

        let g = load_campaign_guide(DEFAULT_JSON, Some(&override_path)).unwrap();
        let entry = lookup_zone(&g, "My Custom Zone").expect("custom zone present");
        assert_eq!(entry.act, "Custom Notes");
        assert_eq!(entry.tasks[0].description, "Remember this weird layout");
    }

    #[test]
    fn missing_override_path_is_ok() {
        let missing = PathBuf::from("/definitely/does/not/exist/override.json");
        let g = load_campaign_guide(DEFAULT_JSON, Some(&missing)).unwrap();
        assert!(lookup_zone(&g, "Clearfell").is_some());
    }

    #[test]
    fn clearfell_is_act1() {
        assert_eq!(require("Clearfell").act, "Act 1");
    }

    #[test]
    fn clearfell_has_cold_resist_reward() {
        let rewards: Vec<&str> = require("Clearfell")
            .tasks
            .iter()
            .map(|t| t.reward.as_str())
            .collect();
        assert!(rewards.iter().any(|r| r.contains("Cold Resistance")));
    }

    #[test]
    fn hunting_grounds_has_passive_points() {
        let rewards: Vec<&str> = require("Hunting Grounds")
            .tasks
            .iter()
            .map(|t| t.reward.as_str())
            .collect();
        assert!(rewards.iter().any(|r| r.contains("Passive Skill Points")));
    }

    #[test]
    fn freythorn_has_spirit_reward() {
        let rewards: Vec<&str> = require("Freythorn")
            .tasks
            .iter()
            .map(|t| t.reward.as_str())
            .collect();
        assert!(rewards.iter().any(|r| r.contains("Spirit")));
    }

    #[test]
    fn ogham_farmlands_passive_points() {
        let rewards: Vec<&str> = require("Ogham Farmlands")
            .tasks
            .iter()
            .map(|t| t.reward.as_str())
            .collect();
        assert!(rewards.iter().any(|r| r.contains("Passive Skill Points")));
    }

    #[test]
    fn ogham_manor_life_reward() {
        let rewards: Vec<&str> = require("Ogham Manor")
            .tasks
            .iter()
            .map(|t| t.reward.as_str())
            .collect();
        assert!(rewards.iter().any(|r| r.contains("Life")));
    }

    #[test]
    fn clearfell_beira_is_required() {
        let required: Vec<&str> = require("Clearfell")
            .tasks
            .iter()
            .filter(|t| !t.optional)
            .map(|t| t.description.as_str())
            .collect();
        assert!(required.iter().any(|d| d.contains("Beira")));
    }

    #[test]
    fn clearfell_next_zone() {
        assert_eq!(
            require("Clearfell").next_zone.as_deref(),
            Some("Mud Burrow")
        );
    }

    #[test]
    fn keth_is_act2() {
        assert_eq!(require("Keth").act, "Act 2");
    }

    #[test]
    fn keth_passive_points() {
        let rewards: Vec<&str> = require("Keth")
            .tasks
            .iter()
            .map(|t| t.reward.as_str())
            .collect();
        assert!(rewards.iter().any(|r| r.contains("Passive Skill Points")));
    }

    #[test]
    fn spires_of_deshar_lightning_resist() {
        let rewards: Vec<&str> = require("The Spires of Deshar")
            .tasks
            .iter()
            .map(|t| t.reward.as_str())
            .collect();
        assert!(rewards.iter().any(|r| r.contains("Lightning Resistance")));
    }

    #[test]
    fn deshar_passive_points() {
        let rewards: Vec<&str> = require("Deshar")
            .tasks
            .iter()
            .map(|t| t.reward.as_str())
            .collect();
        assert!(rewards.iter().any(|r| r.contains("Passive Skill Points")));
    }

    #[test]
    fn venom_crypts_permanent_choice_warning() {
        let entry = require("The Venom Crypts");
        let has_warning = entry.tasks.iter().any(|t| {
            let d = t.description.to_lowercase();
            d.contains("permanent") || d.contains("cannot change")
        });
        assert!(has_warning);
    }

    #[test]
    fn azak_bog_spirit_reward() {
        let rewards: Vec<&str> = require("The Azak Bog")
            .tasks
            .iter()
            .map(|t| t.reward.as_str())
            .collect();
        assert!(rewards.iter().any(|r| r.contains("Spirit")));
    }

    #[test]
    fn aggorat_passive_points() {
        let rewards: Vec<&str> = require("Aggorat")
            .tasks
            .iter()
            .map(|t| t.reward.as_str())
            .collect();
        assert!(rewards.iter().any(|r| r.contains("Passive Skill Points")));
    }

    #[test]
    fn jungle_ruins_passive_points() {
        let rewards: Vec<&str> = require("Jungle Ruins")
            .tasks
            .iter()
            .map(|t| t.reward.as_str())
            .collect();
        assert!(rewards.iter().any(|r| r.contains("Passive Skill Points")));
    }

    #[test]
    fn halls_of_dead_has_three_trials() {
        let trials = require("Halls of the Dead")
            .tasks
            .iter()
            .filter(|t| t.description.contains("Test"))
            .count();
        assert_eq!(trials, 3);
    }

    #[test]
    fn heart_of_the_tribe_is_act4() {
        assert_eq!(require("Heart of the Tribe").act, "Act 4");
    }

    #[test]
    fn journeys_end_passive_points() {
        let rewards: Vec<&str> = require("Journey's End")
            .tasks
            .iter()
            .map(|t| t.reward.as_str())
            .collect();
        assert!(rewards.iter().any(|r| r.contains("Passive Skill Points")));
    }

    #[test]
    fn whakapanu_island_spirit_reward() {
        let rewards: Vec<&str> = require("Whakapanu Island")
            .tasks
            .iter()
            .map(|t| t.reward.as_str())
            .collect();
        assert!(rewards.iter().any(|r| r.contains("Spirit")));
    }

    #[test]
    fn entry_has_zone_and_act() {
        let entry = require("Clearfell");
        assert!(!entry.zone.is_empty());
        assert!(!entry.act.is_empty());
    }

    #[test]
    fn last_zone_has_no_next() {
        // Heart of the Tribe is the final zone in Act 4 with no explicit
        // next_zone and no following zone in its act.
        assert!(require("Heart of the Tribe").next_zone.is_none());
    }

    /// Minimal scoped temp directory that cleans up on drop. Avoids pulling
    /// in `tempfile` as a dev-dependency just for the override tests.
    struct TempDir(PathBuf);

    impl TempDir {
        fn path(&self) -> &Path {
            &self.0
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    impl AsRef<Path> for TempDir {
        fn as_ref(&self) -> &Path {
            self.path()
        }
    }

    fn tempdir() -> TempDir {
        use std::sync::atomic::{AtomicU64, Ordering};
        static COUNTER: AtomicU64 = AtomicU64::new(0);
        let n = COUNTER.fetch_add(1, Ordering::Relaxed);
        let pid = std::process::id();
        let scope = module_path!().replace("::", "-");
        let path = std::env::temp_dir().join(format!("rsexile-test-{scope}-{pid}-{n}"));
        fs::create_dir_all(&path).expect("create tempdir");
        TempDir(path)
    }
}
