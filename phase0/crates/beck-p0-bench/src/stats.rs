//! Percentiles, reported honestly.
//!
//! §13.7 asks for statistical rigour on budget gates. Phase 0 is establishing baselines rather
//! than gating merges, so this is deliberately simple — but it reports the sample size and the
//! maximum alongside the percentiles, because a p99 without an n is a number you cannot argue
//! with.

use serde_json::{json, Value};

pub struct Summary {
    pub n: usize,
    pub mean: f64,
    pub p50: f64,
    pub p90: f64,
    pub p99: f64,
    pub max: f64,
}

impl Summary {
    pub fn of(mut samples: Vec<f64>) -> Summary {
        assert!(!samples.is_empty(), "no samples");
        samples.sort_by(|a, b| a.partial_cmp(b).expect("no NaNs in a timing sample"));
        let pick = |q: f64| {
            let index = ((samples.len() as f64 - 1.0) * q).round() as usize;
            samples[index]
        };
        Summary {
            n: samples.len(),
            mean: samples.iter().sum::<f64>() / samples.len() as f64,
            p50: pick(0.50),
            p90: pick(0.90),
            p99: pick(0.99),
            max: *samples.last().expect("non-empty"),
        }
    }

    pub fn to_json(&self) -> Value {
        json!({
            "n": self.n,
            "mean": round(self.mean),
            "p50": round(self.p50),
            "p90": round(self.p90),
            "p99": round(self.p99),
            "max": round(self.max),
        })
    }
}

impl std::fmt::Display for Summary {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "n={} mean={:.3} p50={:.3} p90={:.3} p99={:.3} max={:.3}",
            self.n, self.mean, self.p50, self.p90, self.p99, self.max
        )
    }
}

pub fn round(x: f64) -> f64 {
    (x * 1000.0).round() / 1000.0
}
