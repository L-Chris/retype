use retype_learning::protocol::SyncLearning;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet};

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct Stamp {
    pub revision: u64,
    pub device: String,
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Field {
    pub stamp: Stamp,
    pub ancestors: BTreeSet<Stamp>,
    pub value: Value,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Conflict {
    pub key: String,
    pub local: Field,
    pub remote: Field,
    pub device_name: String,
}
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct State {
    pub initialized: bool,
    pub revision: u64,
    pub fields: BTreeMap<String, Field>,
    pub conflicts: Vec<Conflict>,
    pub confirmed: bool,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Snapshot {
    pub version: u32,
    pub device: String,
    pub name: String,
    pub fields: BTreeMap<String, Field>,
    pub learning: SyncLearning,
    pub statistics: Vec<crate::statistics::Bucket>,
}
impl State {
    pub fn capture(&mut self, device: &str, values: &BTreeMap<String, Value>) {
        for (key, value) in values {
            if self.fields.get(key).is_some_and(|f| f.value == *value) {
                continue;
            }
            self.revision = self.revision.saturating_add(1);
            let mut ancestors = self
                .fields
                .get(key)
                .map_or_else(BTreeSet::new, |f| f.ancestors.clone());
            if let Some(old) = self.fields.get(key) {
                ancestors.insert(old.stamp.clone());
            }
            // Resolving a conflict explicitly acknowledges both branches.
            for c in self.conflicts.iter().filter(|c| &c.key == key) {
                ancestors.extend(c.remote.ancestors.clone());
                ancestors.insert(c.remote.stamp.clone());
            }
            self.fields.insert(
                key.clone(),
                Field {
                    stamp: Stamp {
                        revision: self.revision,
                        device: device.into(),
                    },
                    ancestors,
                    value: value.clone(),
                },
            );
            self.conflicts.retain(|c| &c.key != key);
        }
    }
    pub fn merge(&mut self, remote: &Snapshot) {
        for (key, other) in &remote.fields {
            self.revision = self.revision.max(other.stamp.revision);
            let Some(local) = self.fields.get(key).cloned() else {
                self.fields.insert(key.clone(), other.clone());
                continue;
            };
            if local.stamp == other.stamp || local.ancestors.contains(&other.stamp) {
                continue;
            }
            if other.ancestors.contains(&local.stamp) {
                self.fields.insert(key.clone(), other.clone());
                continue;
            }
            if local.value == other.value {
                let (mut winner, loser) = if local.stamp > other.stamp {
                    (local, other.clone())
                } else {
                    (other.clone(), local)
                };
                winner.ancestors.extend(loser.ancestors);
                winner.ancestors.insert(loser.stamp);
                self.fields.insert(key.clone(), winner);
            } else if !self
                .conflicts
                .iter()
                .any(|c| c.key == *key && c.remote.stamp == other.stamp)
            {
                self.conflicts.push(Conflict {
                    key: key.clone(),
                    local,
                    remote: other.clone(),
                    device_name: remote.name.clone(),
                });
            }
        }
        self.conflicts.retain(|c| {
            !self
                .fields
                .get(&c.key)
                .is_some_and(|f| f.ancestors.contains(&c.remote.stamp))
        });
    }
    pub fn resolve(
        &mut self,
        device: &str,
        key: &str,
        use_remote: bool,
        stamp: Option<&Stamp>,
    ) -> bool {
        let Some(conflict) = self
            .conflicts
            .iter()
            .find(|c| c.key == key && stamp.is_none_or(|stamp| &c.remote.stamp == stamp))
            .cloned()
        else {
            return false;
        };
        let mut selected = if use_remote {
            conflict.remote.clone()
        } else {
            self.fields.get(key).cloned().unwrap_or(conflict.local)
        };
        selected.ancestors.insert(selected.stamp.clone());
        for c in self.conflicts.iter().filter(|c| c.key == key) {
            selected.ancestors.extend(c.local.ancestors.clone());
            selected.ancestors.insert(c.local.stamp.clone());
            selected.ancestors.extend(c.remote.ancestors.clone());
            selected.ancestors.insert(c.remote.stamp.clone());
        }
        self.revision = self.revision.saturating_add(1);
        selected.stamp = Stamp {
            revision: self.revision,
            device: device.into(),
        };
        self.fields.insert(key.into(), selected);
        self.conflicts.retain(|c| c.key != key);
        true
    }
    pub fn values(&self) -> BTreeMap<String, Value> {
        self.fields
            .iter()
            .map(|(k, f)| (k.clone(), f.value.clone()))
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn snapshot(state: &State) -> Snapshot {
        Snapshot {
            version: 1,
            device: "b".repeat(32),
            name: "B".into(),
            fields: state.fields.clone(),
            learning: Default::default(),
            statistics: vec![],
        }
    }
    #[test]
    fn concurrent_edits_require_resolution_and_roundtrips_are_idempotent() {
        let mut a = State::default();
        a.capture("a", &BTreeMap::from([("mode".into(), Value::from(0))]));
        let mut b = a.clone();
        a.capture("a", &BTreeMap::from([("mode".into(), Value::from(1))]));
        b.capture("b", &BTreeMap::from([("mode".into(), Value::from(2))]));
        a.merge(&snapshot(&b));
        assert_eq!(a.conflicts.len(), 1);
        a.merge(&snapshot(&b));
        assert_eq!(a.conflicts.len(), 1);
        assert!(a.resolve("a", "mode", true, None));
        assert_eq!(a.values()["mode"], 2);
        b.merge(&snapshot(&a));
        assert!(b.conflicts.is_empty());
        assert_eq!(b.values()["mode"], 2);
    }
}
