use std::collections::{BTreeMap, VecDeque};

use serde::{Deserialize, Serialize};

pub const SEEN_RING: usize = 200;
pub const RECENT_MAX: usize = 20;

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct State {
    pub version: u32,
    pub instances: BTreeMap<String, InstanceState>,
    pub recent: VecDeque<RecentMessage>,
    pub gaps: Vec<GapFlag>,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct InstanceState {
    pub watermark: Option<i64>,
    pub last_message: Option<SeenMessage>,
    pub seen_ids: VecDeque<String>,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct SeenMessage {
    pub id: String,
    pub time: i64,
    pub expires: Option<i64>,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct RecentMessage {
    pub instance_id: String,
    pub topic: String,
    pub id: String,
    pub time: i64,
    pub title: String,
    pub body: String,
    pub click: Option<String>,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct GapFlag {
    pub instance_id: String,
    pub topic: String,
    pub detected_at: i64,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn state_roundtrips_and_tolerates_missing_fields() {
        let mut s = State::default();
        s.instances.insert("a".into(), InstanceState::default());
        let json = serde_json::to_string(&s).unwrap();
        assert_eq!(serde_json::from_str::<State>(&json).unwrap(), s);
        assert_eq!(
            serde_json::from_str::<State>("{}").unwrap(),
            State::default()
        );
    }
}
