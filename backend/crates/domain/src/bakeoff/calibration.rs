//! Calibration bins and expected calibration error (S7 5.13).

use serde::Serialize;

use super::CALIBRATION_BINS;

/// One equal-width confidence bin.
#[derive(Clone, Copy, Debug, PartialEq, Serialize)]
pub struct CalibrationBin {
    /// Inclusive lower confidence bound.
    pub confidence_from: f64,
    /// Exclusive upper confidence bound.
    pub confidence_to: f64,
    /// Points in the bin.
    pub count: u64,
    /// Mean stated confidence, `None` when the bin is empty.
    pub mean_confidence: Option<f64>,
    /// Observed accuracy, `None` when the bin is empty.
    pub observed_accuracy: Option<f64>,
}

/// Ten equal-width bins over confidence 0 to 1 and the expected calibration
/// error; ECE is `None` when no point is usable.
///
/// Points with non-finite confidence or confidence outside 0 to 1 are skipped
/// (the classifiers already validate).
#[must_use]
pub fn calibration(points: &[(f64, bool)]) -> (Vec<CalibrationBin>, Option<f64>) {
    let mut counts = [0_u64; CALIBRATION_BINS];
    let mut confidence_sum = [0.0_f64; CALIBRATION_BINS];
    let mut correct = [0_u64; CALIBRATION_BINS];
    let mut used: u64 = 0;
    for &(confidence, is_correct) in points {
        if !confidence.is_finite() || confidence < 0.0 || confidence > 1.0 {
            continue;
        }
        let bin = ((confidence * 10.0).floor() as usize).min(CALIBRATION_BINS - 1);
        counts[bin] += 1;
        confidence_sum[bin] += confidence;
        if is_correct {
            correct[bin] += 1;
        }
        used += 1;
    }

    let bins = (0..CALIBRATION_BINS)
        .map(|bin| CalibrationBin {
            confidence_from: bin as f64 / 10.0,
            confidence_to: (bin + 1) as f64 / 10.0,
            count: counts[bin],
            mean_confidence: (counts[bin] > 0).then(|| confidence_sum[bin] / counts[bin] as f64),
            observed_accuracy: (counts[bin] > 0).then(|| correct[bin] as f64 / counts[bin] as f64),
        })
        .collect();

    let ece = if used == 0 {
        None
    } else {
        let mut error = 0.0;
        for bin in 0..CALIBRATION_BINS {
            if counts[bin] == 0 {
                continue;
            }
            let accuracy = correct[bin] as f64 / counts[bin] as f64;
            let mean_confidence = confidence_sum[bin] / counts[bin] as f64;
            error += (counts[bin] as f64 / used as f64) * (accuracy - mean_confidence).abs();
        }
        Some(error)
    };
    (bins, ece)
}
