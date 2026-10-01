//! Smoothed homophone probabilities, expressed in the original lexical score scale.
//! Parameters are shared across words and schemes; candidate position is not evidence.
use retype_types::SyllableId;

/// Equivalent prior sample size. Sparse feedback stays close to the base dictionary.
pub const PRIOR_STRENGTH: f64 = 32.0;
/// Half-life in subsequent learned selections, not elapsed wall-clock time.
pub const RECENT_HALF_LIFE: f64 = 128.0;

#[derive(Debug, Clone, PartialEq)]
pub struct Usage {
    pub syllables: Vec<SyllableId>,
    pub text: String,
    pub count: u64,
    pub recent: f64,
    pub last_tick: u64,
    pub prior_logp: Option<f32>,
}
impl Usage {
    pub fn recent_at(&self, tick: u64) -> f64 {
        self.recent * (-((tick.saturating_sub(self.last_tick)) as f64) / RECENT_HALF_LIFE).exp2()
    }
    pub fn evidence(&self, tick: u64) -> f64 {
        // Keep exact lifetime counts, but give old evidence diminishing influence so a
        // long history does not prevent a user from changing their current preference.
        (self.count as f64).ln_1p() + self.recent_at(tick)
    }
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct UsageSnapshot {
    pub tick: u64,
    pub records: Vec<Usage>,
}
