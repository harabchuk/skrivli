//! Microphone gain, the fixed system mix gain, AGC, and the soft limiter.
//!
//! AGC is off unless the recording asks for it. The limiter leaves samples
//! inside the knee untouched and bends louder sums toward full scale.

/// Initial microphone mix gain.
pub(crate) const MIC_GAIN_DEFAULT: f32 = 1.0;
/// Fixed system-audio mix gain. The system meter is display-only.
pub(crate) const SYSTEM_GAIN: f32 = 0.8;
/// Samples at or under this absolute value pass through the limiter.
pub(crate) const LIMIT_KNEE: f32 = 0.8;
const AGC_MIN: f32 = 0.5;
const AGC_MAX: f32 = 4.0;
const AGC_TAU_SECONDS: f32 = 0.25;

pub(crate) fn clamp_slider_gain(gain: f32) -> f32 {
    if gain.is_finite() {
        gain.clamp(0.0, AGC_MAX)
    } else {
        MIC_GAIN_DEFAULT
    }
}

pub(crate) fn db_to_linear(db: f32) -> f32 {
    10_f32.powf(db / 20.0)
}

pub(crate) fn db_fs(linear: f32) -> f32 {
    if linear <= 0.0 || !linear.is_finite() {
        return -60.0;
    }
    let db = 20.0 * linear.log10();
    if db.is_finite() {
        db.clamp(-60.0, 0.0)
    } else {
        -60.0
    }
}

pub(crate) fn meter_fraction(db: f32) -> f32 {
    ((db.clamp(-60.0, 0.0) + 60.0) / 60.0).clamp(0.0, 1.0)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum MeterBand {
    Normal,
    Warm,
    Hot,
}

pub(crate) fn meter_band(db: f32) -> MeterBand {
    if db >= -6.0 {
        MeterBand::Hot
    } else if db >= -18.0 {
        MeterBand::Warm
    } else {
        MeterBand::Normal
    }
}

pub(crate) fn format_db(db: f32) -> String {
    let db = if db.is_finite() {
        db.clamp(-60.0, 0.0)
    } else {
        -60.0
    };
    if db >= -0.05 {
        "0 dB".to_string()
    } else {
        format!("\u{2212}{:.0} dB", db.abs())
    }
}

pub(crate) fn format_gain(gain: f32) -> String {
    format!("{:.1}×", clamp_slider_gain(gain))
}

/// Compress values outside [`LIMIT_KNEE`] toward ±1 without hard clipping.
pub(crate) fn soft_limit(sample: f32) -> f32 {
    let x = if sample.is_finite() {
        sample.clamp(-8.0, 8.0)
    } else {
        return 0.0;
    };
    let ax = x.abs();
    if ax <= LIMIT_KNEE {
        return x;
    }
    let sign = if x.is_sign_negative() { -1.0 } else { 1.0 };
    let over = ax - LIMIT_KNEE;
    let shaped = LIMIT_KNEE + (1.0 - LIMIT_KNEE) * (over / (1.0 + over));
    sign * shaped.min(1.0)
}

#[derive(Clone, Debug)]
pub(crate) struct Agc {
    gain: f32,
    sum_sq: f64,
    window: std::collections::VecDeque<f32>,
}

impl Agc {
    pub(crate) fn new(gain: f32) -> Self {
        Self {
            gain: clamp_slider_gain(gain).clamp(AGC_MIN, AGC_MAX),
            sum_sq: 0.0,
            window: std::collections::VecDeque::new(),
        }
    }

    /// Stereo interleaved pre-gain microphone samples.
    /// Returns the gain to apply. When `enabled` is false this is `slider`.
    pub(crate) fn process(&mut self, frames: &[f32], enabled: bool, slider: f32) -> f32 {
        let slider = clamp_slider_gain(slider);
        if !enabled {
            self.gain = slider.clamp(AGC_MIN, AGC_MAX);
            self.sum_sq = 0.0;
            self.window.clear();
            return slider;
        }
        let mut fed = 0_u32;
        for pair in frames.as_chunks::<2>().0 {
            let energy = 0.5
                * (f64::from(pair[0]) * f64::from(pair[0])
                    + f64::from(pair[1]) * f64::from(pair[1]));
            self.push_energy(energy);
            fed = fed.saturating_add(1);
        }
        let count = self.window.len();
        if count == 0 || fed == 0 {
            return self.gain;
        }
        let rms = (self.sum_sq / count as f64).sqrt() as f32;
        let gate = db_to_linear(-45.0);
        if rms >= gate && rms > 0.0 {
            let desired = (db_to_linear(-18.0) / rms).clamp(AGC_MIN, AGC_MAX);
            let dt = fed as f32 / 48_000.0;
            let alpha = 1.0 - (-dt / AGC_TAU_SECONDS).exp();
            self.gain += (desired - self.gain) * alpha;
            self.gain = self.gain.clamp(AGC_MIN, AGC_MAX);
        }
        self.gain
    }

    fn push_energy(&mut self, energy: f64) {
        const WINDOW: usize = 36_000;
        let stored = energy as f32;
        if self.window.len() == WINDOW {
            if let Some(old) = self.window.pop_front() {
                self.sum_sq -= f64::from(old);
            }
        }
        self.window.push_back(stored);
        self.sum_sq += energy;
        if self.sum_sq < 0.0 {
            self.sum_sq = 0.0;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{
        db_fs, format_db, meter_band, meter_fraction, soft_limit, Agc, MeterBand, LIMIT_KNEE,
        MIC_GAIN_DEFAULT, SYSTEM_GAIN,
    };

    #[test]
    fn limiter_keeps_quiet_samples_and_bends_hot_sums() {
        // Arrange
        let quiet = 0.25_f32;
        let hot = 1.0 + SYSTEM_GAIN;

        // Act
        let passed = soft_limit(quiet);
        let limited = soft_limit(hot);
        let negative = soft_limit(-hot);

        // Assert
        assert_eq!(passed, quiet);
        assert!(hot > LIMIT_KNEE);
        assert!(limited < 1.0);
        assert!(limited > LIMIT_KNEE);
        assert!((negative + limited).abs() < 0.0001);
        assert_eq!(soft_limit(f32::NAN), 0.0);
        assert_eq!(MIC_GAIN_DEFAULT, 1.0);
    }

    #[test]
    fn meter_scale_runs_from_minus_60_to_zero() {
        // Arrange
        let mid = db_fs(0.1);

        // Act
        let silent = db_fs(0.0);
        let full = db_fs(1.0);
        let fraction = meter_fraction(mid);

        // Assert
        assert_eq!(silent, -60.0);
        assert_eq!(full, 0.0);
        assert!(mid > -60.0 && mid < 0.0);
        assert!(fraction > 0.0 && fraction < 1.0);
        assert_eq!(meter_band(-30.0), MeterBand::Normal);
        assert_eq!(meter_band(-12.0), MeterBand::Warm);
        assert_eq!(meter_band(-3.0), MeterBand::Hot);
        assert!(format_db(-12.4).contains('2'));
        assert!(format_db(-12.4).contains("dB"));
    }

    #[test]
    fn agc_targets_speech_and_ignores_the_noise_gate() {
        // Arrange
        let speech = db_to_pair(-30.0);
        let noise = db_to_pair(-50.0);
        let mut voiced = Agc::new(1.0);
        let mut quiet = Agc::new(1.0);
        let mut manual = Agc::new(1.0);

        // Act
        let mut speech_gain = 1.0_f32;
        for _ in 0..200 {
            speech_gain = voiced.process(&speech, true, 1.0);
        }
        let mut noise_gain = 1.0_f32;
        for _ in 0..50 {
            noise_gain = quiet.process(&noise, true, 1.0);
        }
        let slider = manual.process(&speech, false, 1.6);

        // Assert
        assert!(speech_gain > 2.0 && speech_gain <= 4.0);
        assert!((noise_gain - 1.0).abs() < 0.001);
        assert!((slider - 1.6).abs() < 0.001);
        let one_step = Agc::new(1.0).process(&speech, true, 1.0);
        assert!(one_step < speech_gain);
        assert!(one_step > 1.0);
    }

    fn db_to_pair(db: f32) -> Vec<f32> {
        let sample = super::db_to_linear(db);
        vec![sample; 960]
    }
}
