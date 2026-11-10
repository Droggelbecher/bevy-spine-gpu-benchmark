//! Keyframe interpolation curves for bone timelines (Spine 4.x semantics).
//!
//! In Spine, the `curve` field of key *k* describes the segment from key *k* to key *k+1*
//! (the curve of the last key is meaningless and ignored):
//!
//! * absent: linear interpolation,
//! * `"stepped"`: hold key *k*'s value until key *k+1* is reached,
//! * an array of 4 numbers `cx1, cy1, cx2, cy2` **per value component**: a cubic bezier from
//!   `(t_k, v_k)` via `(cx1, cy1)` and `(cx2, cy2)` to `(t_k+1, v_k+1)`. The control points are
//!   *absolute*: x is a time in seconds, y is a value in the timeline's units (degrees for
//!   rotation), not normalised to 0..1.
//!
//! Rotate timelines have one component, translate and scale timelines two (x, then y), and every
//! component has its own curve. That is why [`CurveTimeline`] is generic over the component count.
//!
//! Evaluating a bezier at a given time means solving `x(u) = time` for the curve parameter `u`.
//! Like the official runtimes we avoid doing that every frame: at load time each bezier segment is
//! sampled at [`BEZIER_SEGMENTS`] evenly spaced parameter values, and at runtime we linearly
//! interpolate between the two neighbouring samples. Sampling runs per bone, per skeleton, per
//! frame, so this keeps it to a short scan and one lerp, without allocating.

/// Number of straight pieces a bezier segment is approximated by.
///
/// The curve is sampled at `BEZIER_SEGMENTS - 1` interior parameter values (the end points are
/// the keys themselves). Spine's own runtimes use the same count.
pub(crate) const BEZIER_SEGMENTS: usize = 10;

/// A segment curve as authored in the Spine file.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub(crate) enum CurveDef {
    #[default]
    Linear,
    Stepped,
    /// Absolute control points `[cx1, cy1, cx2, cy2]`.
    Bezier([f32; 4]),
}

/// A segment curve in its runtime form (bezier segments are pre-sampled).
#[derive(Clone, Copy, Debug, PartialEq)]
enum Curve {
    Linear,
    Stepped,
    /// Interior sample points `(time, value)` of the bezier, in increasing parameter order.
    Bezier([[f32; 2]; BEZIER_SEGMENTS - 1]),
}

/// A key as authored: time, value per component and the curve per component leading to the next key.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct KeyDef<const N: usize> {
    pub(crate) time: f32,
    pub(crate) value: [f32; N],
    pub(crate) curve: [CurveDef; N],
}

#[derive(Clone, Copy, Debug)]
struct Key<const N: usize> {
    time: f32,
    value: [f32; N],
    curve: [Curve; N],
}

/// Keys of a numeric bone timeline with `N` value components, each with its own segment curves.
#[derive(Clone, Debug)]
pub(crate) struct CurveTimeline<const N: usize> {
    keys: Vec<Key<N>>,
}

impl<const N: usize> Default for CurveTimeline<N> {
    fn default() -> Self {
        Self { keys: vec![] }
    }
}

impl Curve {
    /// Pre-sample the bezier with absolute control points `[cx1, cy1, cx2, cy2]` running from
    /// `(t0, v0)` to `(t1, v1)`.
    fn bezier(t0: f32, v0: f32, [cx1, cy1, cx2, cy2]: [f32; 4], t1: f32, v1: f32) -> Self {
        let cubic = |p0: f32, p1: f32, p2: f32, p3: f32, u: f32| {
            let w = 1.0 - u;
            w * w * w * p0 + 3.0 * w * w * u * p1 + 3.0 * w * u * u * p2 + u * u * u * p3
        };
        let samples = std::array::from_fn(|i| {
            let u = (i + 1) as f32 / BEZIER_SEGMENTS as f32;
            [cubic(t0, cx1, cx2, t1, u), cubic(v0, cy1, cy2, v1, u)]
        });
        Curve::Bezier(samples)
    }

    /// Value at `time` on the segment `(t0, v0)..(t1, v1)`, with `t0 <= time <= t1`.
    fn eval(&self, t0: f32, v0: f32, t1: f32, v1: f32, time: f32) -> f32 {
        match self {
            Curve::Stepped => v0,
            Curve::Linear => lerp_segment([t0, v0], [t1, v1], time),
            Curve::Bezier(samples) => {
                let mut prev = [t0, v0];
                for &sample in samples {
                    if sample[0] >= time {
                        return lerp_segment(prev, sample, time);
                    }
                    prev = sample;
                }
                lerp_segment(prev, [t1, v1], time)
            }
        }
    }
}

/// Linear interpolation of y at x = `time` between two points; returns `a`'s y for a zero-width span.
fn lerp_segment(a: [f32; 2], b: [f32; 2], time: f32) -> f32 {
    let dx = b[0] - a[0];
    if dx <= 0.0 {
        return a[1];
    }
    a[1] + (b[1] - a[1]) * ((time - a[0]) / dx)
}

impl<const N: usize> CurveTimeline<N> {
    /// Build a timeline from authored keys, which must be sorted by time. Bezier segments are
    /// sampled here, so this is load-time work.
    pub(crate) fn new(defs: impl IntoIterator<Item = KeyDef<N>>) -> Self {
        let defs: Vec<_> = defs.into_iter().collect();
        let keys = defs
            .iter()
            .enumerate()
            .map(|(i, def)| {
                // The last key has no following segment; its curve is never evaluated.
                let next = defs.get(i + 1);
                let curve = std::array::from_fn(|c| match (def.curve[c], next) {
                    (CurveDef::Bezier(points), Some(next)) => {
                        Curve::bezier(def.time, def.value[c], points, next.time, next.value[c])
                    }
                    (CurveDef::Stepped, _) => Curve::Stepped,
                    _ => Curve::Linear,
                });
                Key {
                    time: def.time,
                    value: def.value,
                    curve,
                }
            })
            .collect();
        Self { keys }
    }

    pub(crate) fn last_time(&self) -> Option<f32> {
        self.keys.last().map(|key| key.time)
    }

    /// Value of all components at `time`. Before the first key this is the first value, after the
    /// last key the last value. Returns `None` for a timeline without keys.
    pub(crate) fn at(&self, time: f32) -> Option<[f32; N]> {
        let right = self.keys.partition_point(|key| key.time <= time);
        let Some(left) = right.checked_sub(1) else {
            // Spine actually returns the setup pose value before the first key
            // but we dont care for this behaviour and this is easier
            return self.keys.first().map(|key| key.value);
        };
        let Some(next) = self.keys.get(right) else {
            return self.keys.last().map(|key| key.value);
        };

        let key = &self.keys[left];
        Some(std::array::from_fn(|c| {
            key.curve[c].eval(key.time, key.value[c], next.time, next.value[c], time)
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key1(time: f32, value: f32, curve: CurveDef) -> KeyDef<1> {
        KeyDef {
            time,
            value: [value],
            curve: [curve],
        }
    }

    fn at1(timeline: &CurveTimeline<1>, t: f32) -> f32 {
        timeline.at(t).unwrap()[0]
    }

    #[test]
    fn empty_timeline_has_no_value() {
        assert_eq!(CurveTimeline::<1>::default().at(0.5), None);
        assert_eq!(CurveTimeline::<1>::default().last_time(), None);
    }

    #[test]
    fn linear_interpolates_and_clamps() {
        let tl = CurveTimeline::new([
            key1(1.0, 10.0, CurveDef::Linear),
            key1(2.0, 20.0, CurveDef::Linear),
        ]);
        assert_eq!(at1(&tl, 0.0), 10.0);
        assert_eq!(at1(&tl, 1.0), 10.0);
        assert!((at1(&tl, 1.25) - 12.5).abs() < 1e-5);
        assert_eq!(at1(&tl, 2.0), 20.0);
        assert_eq!(at1(&tl, 3.0), 20.0);
        assert_eq!(tl.last_time(), Some(2.0));
    }

    #[test]
    fn stepped_holds_left_value_until_next_key() {
        let tl = CurveTimeline::new([
            key1(0.0, 1.0, CurveDef::Stepped),
            key1(1.0, 5.0, CurveDef::Linear),
            key1(2.0, 9.0, CurveDef::Linear),
        ]);
        assert_eq!(at1(&tl, 0.0), 1.0);
        assert_eq!(at1(&tl, 0.999), 1.0);
        assert_eq!(at1(&tl, 1.0), 5.0);
        assert!((at1(&tl, 1.5) - 7.0).abs() < 1e-5);
    }

    #[test]
    fn bezier_on_the_diagonal_is_linear() {
        // Control points at 1/3 and 2/3 of the way, on the straight line.
        let (t0, v0, t1, v1) = (0.5_f32, -10.0_f32, 1.5_f32, 50.0_f32);
        let c = CurveDef::Bezier([
            t0 + (t1 - t0) / 3.0,
            v0 + (v1 - v0) / 3.0,
            t0 + (t1 - t0) * 2.0 / 3.0,
            v0 + (v1 - v0) * 2.0 / 3.0,
        ]);
        let tl = CurveTimeline::new([key1(t0, v0, c), key1(t1, v1, CurveDef::Linear)]);
        for i in 0..=20 {
            let t = t0 + (t1 - t0) * i as f32 / 20.0;
            let expected = v0 + (v1 - v0) * (t - t0) / (t1 - t0);
            assert!((at1(&tl, t) - expected).abs() < 1e-3, "t={t}");
        }
    }

    /// Independent reference: solve x(u) = t by bisection, return y(u).
    fn reference_bezier(p: [f32; 8], t: f32) -> f32 {
        let cubic = |a: f64, b: f64, c: f64, d: f64, u: f64| {
            let w = 1.0 - u;
            w * w * w * a + 3.0 * w * w * u * b + 3.0 * w * u * u * c + u * u * u * d
        };
        let p = p.map(f64::from);
        let (mut lo, mut hi) = (0.0, 1.0);
        for _ in 0..60 {
            let mid = (lo + hi) / 2.0;
            if cubic(p[0], p[2], p[4], p[6], mid) < t as f64 {
                lo = mid;
            } else {
                hi = mid;
            }
        }
        cubic(p[1], p[3], p[5], p[7], (lo + hi) / 2.0) as f32
    }

    #[test]
    fn ease_bezier_matches_reference_solution() {
        // Strong ease-in/ease-out from (0, 0) to (1, 100).
        let c = [0.6, 0.0, 0.9, 100.0];
        let tl = CurveTimeline::new([
            key1(0.0, 0.0, CurveDef::Bezier(c)),
            key1(1.0, 100.0, CurveDef::Linear),
        ]);
        for i in 1..100 {
            let t = i as f32 / 100.0;
            let expected = reference_bezier([0.0, 0.0, c[0], c[1], c[2], c[3], 1.0, 100.0], t);
            // 10 linear pieces on a steep curve: allow 1.5% of the 100 unit range.
            assert!(
                (at1(&tl, t) - expected).abs() < 1.5,
                "t={t}: got {}, expected {expected}",
                at1(&tl, t)
            );
        }
    }

    #[test]
    fn bezier_endpoints_are_exact() {
        let tl = CurveTimeline::new([
            key1(0.25, 3.0, CurveDef::Bezier([0.4, -20.0, 0.6, 30.0])),
            key1(0.75, -7.0, CurveDef::Linear),
        ]);
        assert_eq!(at1(&tl, 0.25), 3.0);
        assert_eq!(at1(&tl, 0.75), -7.0);
        assert_eq!(at1(&tl, 0.0), 3.0);
        assert_eq!(at1(&tl, 1.0), -7.0);
    }

    #[test]
    fn components_use_their_own_curves() {
        let tl = CurveTimeline::<2>::new([
            KeyDef {
                time: 0.0,
                value: [0.0, 0.0],
                curve: [CurveDef::Stepped, CurveDef::Linear],
            },
            KeyDef {
                time: 1.0,
                value: [10.0, 10.0],
                curve: [CurveDef::Linear; 2],
            },
        ]);
        assert_eq!(tl.at(0.5), Some([0.0, 5.0]));
        assert_eq!(tl.at(1.0), Some([10.0, 10.0]));
    }

    #[test]
    fn duplicate_key_times_do_not_produce_nan() {
        let tl = CurveTimeline::new([
            key1(1.0, 1.0, CurveDef::Bezier([1.0, 0.0, 1.0, 2.0])),
            key1(1.0, 2.0, CurveDef::Linear),
        ]);
        assert!(at1(&tl, 1.0).is_finite());
        assert!(at1(&tl, 0.5).is_finite());
    }
}
