//! Default microphone and system slider gains, and the soft limiter.
//!
//! The limiter leaves samples inside the knee untouched and bends louder
//! sums toward full scale.

/// Initial microphone mix gain.
pub(crate) const MIC_GAIN_DEFAULT: f32 = 1.0;
/// Initial system-audio mix gain. Users move it with the system slider.
pub(crate) const SYSTEM_GAIN_DEFAULT: f32 = 0.8;
/// Samples at or under this absolute value pass through the limiter.
pub(crate) const LIMIT_KNEE: f32 = 0.8;
const SLIDER_MAX: f32 = 4.0;

pub(crate) fn clamp_slider_gain(gain: f32) -> f32 {
    if gain.is_finite() {
        gain.clamp(0.0, SLIDER_MAX)
    } else {
        MIC_GAIN_DEFAULT
    }
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

#[cfg(test)]
mod tests {
    use super::{
        db_fs, format_db, meter_band, meter_fraction, soft_limit, MeterBand, LIMIT_KNEE,
        MIC_GAIN_DEFAULT, SYSTEM_GAIN_DEFAULT,
    };

    #[test]
    fn limiter_keeps_quiet_samples_and_bends_hot_sums() {
        // Arrange
        let quiet = 0.25_f32;
        let hot = 1.0 + SYSTEM_GAIN_DEFAULT;

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
}
