//! Platform-neutral pointer input and gesture recognition.
//!
//! * pen, or one finger while no pen has been seen → draw (or erase)
//! * two fingers → pan + pinch zoom around the gesture centre, with inertia
//! * once a pen was seen, fingers never draw (palm rejection); one finger pans instead
//! * mouse: primary button draws, secondary/middle pans, the wheel zooms (handled by the app)

use std::collections::HashMap;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PointerKind {
    Mouse,
    Touch,
    Pen,
    /// Pen held with its eraser end / eraser button.
    PenEraser,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Phase {
    Down,
    Move,
    Up,
    Cancel,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MouseButton {
    Primary,
    Secondary,
    Middle,
}

#[derive(Clone, Copy, Debug)]
pub struct PointerEvent {
    pub id: u64,
    pub kind: PointerKind,
    pub phase: Phase,
    /// Physical pixels.
    pub pos: [f64; 2],
    /// 0..1
    pub pressure: f32,
    /// Seconds, monotonic.
    pub time: f64,
    pub button: MouseButton,
}

/// What the app should do in response to input.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Gesture {
    /// Start/continue/finish an ink stroke. `erase` = eraser end of the pen.
    StrokeBegin { pos: [f64; 2], pressure: f32, time: f64, erase: bool },
    StrokeMove { pos: [f64; 2], pressure: f32, time: f64 },
    StrokeEnd,
    /// Throw away the stroke in progress (a second finger landed right after the first).
    StrokeCancel,
    Pan { delta: [f64; 2] },
    Zoom { center: [f64; 2], dlog: f64 },
    /// Rotate the view by `angle` radians around `center` (screen px).
    Rotate { center: [f64; 2], angle: f64 },
    /// Two-finger gesture finished (the view may snap to an upright angle).
    PinchEnd { center: [f64; 2] },
    /// Gesture released with this velocity: pan px/s and zoom log2/s.
    Fling { velocity: [f64; 2], zoom_velocity: f64, center: [f64; 2] },
    /// Any touch started: stop inertia.
    Stop,
}

#[derive(Clone, Copy, Debug)]
struct Tracked {
    kind: PointerKind,
    pos: [f64; 2],
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum Mode {
    Idle,
    Draw { id: u64, started: f64 },
    Pan { id: u64 },
    Pinch { a: u64, b: u64 },
    /// Wait until every finger is lifted (after a pinch).
    Blocked,
}

/// A stroke started by a finger is discarded when a second finger lands within this time.
const CANCEL_WINDOW: f64 = 0.25;
/// Two fingers must twist this much (radians, ~10°) before the view starts rotating, so plain
/// pinch-zooming never tilts the canvas by accident.
const ROTATE_THRESHOLD: f64 = 0.17;

pub struct Gestures {
    pointers: HashMap<u64, Tracked>,
    mode: Mode,
    pub pen_seen: bool,
    last_time: f64,
    velocity: [f64; 2],
    zoom_velocity: f64,
    last_center: [f64; 2],
    last_dist: f64,
    last_angle: f64,
    /// Twist accumulated before rotation engages (see `ROTATE_THRESHOLD`).
    twist: f64,
    rotating: bool,
}

impl Default for Gestures {
    fn default() -> Self {
        Self {
            pointers: HashMap::new(),
            mode: Mode::Idle,
            pen_seen: false,
            last_time: 0.0,
            velocity: [0.0; 2],
            zoom_velocity: 0.0,
            last_center: [0.0; 2],
            last_dist: 1.0,
            last_angle: 0.0,
            twist: 0.0,
            rotating: false,
        }
    }
}

fn mid(a: [f64; 2], b: [f64; 2]) -> [f64; 2] {
    [(a[0] + b[0]) * 0.5, (a[1] + b[1]) * 0.5]
}

fn angle(a: [f64; 2], b: [f64; 2]) -> f64 {
    (b[1] - a[1]).atan2(b[0] - a[0])
}

fn dist(a: [f64; 2], b: [f64; 2]) -> f64 {
    ((a[0] - b[0]).powi(2) + (a[1] - b[1]).powi(2)).sqrt().max(1.0)
}

impl Gestures {
    pub fn is_drawing(&self) -> bool {
        matches!(self.mode, Mode::Draw { .. })
    }

    fn touches(&self) -> Vec<u64> {
        let mut v: Vec<u64> =
            self.pointers.iter().filter(|(_, t)| t.kind == PointerKind::Touch).map(|(&id, _)| id).collect();
        v.sort_unstable();
        v
    }

    fn start_pinch(&mut self, a: u64, b: u64, time: f64) {
        let (pa, pb) = (self.pointers[&a].pos, self.pointers[&b].pos);
        self.last_center = mid(pa, pb);
        self.last_dist = dist(pa, pb);
        self.last_angle = angle(pa, pb);
        self.twist = 0.0;
        self.rotating = false;
        self.last_time = time;
        self.velocity = [0.0; 2];
        self.zoom_velocity = 0.0;
        self.mode = Mode::Pinch { a, b };
    }

    fn track_velocity(&mut self, delta: [f64; 2], dlog: f64, time: f64) {
        let dt = (time - self.last_time).clamp(1e-3, 0.1);
        self.last_time = time;
        let k = 0.35; // smoothing of the velocity estimate
        self.velocity[0] = self.velocity[0] * (1.0 - k) + delta[0] / dt * k;
        self.velocity[1] = self.velocity[1] * (1.0 - k) + delta[1] / dt * k;
        self.zoom_velocity = self.zoom_velocity * (1.0 - k) + dlog / dt * k;
    }

    fn fling(&self, time: f64) -> Option<Gesture> {
        // No fling if the fingers rested before lifting.
        if time - self.last_time > 0.08 {
            return None;
        }
        Some(Gesture::Fling { velocity: self.velocity, zoom_velocity: self.zoom_velocity, center: self.last_center })
    }

    pub fn handle(&mut self, e: &PointerEvent, out: &mut Vec<Gesture>) {
        let is_pen = matches!(e.kind, PointerKind::Pen | PointerKind::PenEraser);
        if is_pen {
            self.pen_seen = true;
        }
        match e.phase {
            Phase::Down => {
                out.push(Gesture::Stop);
                self.pointers.insert(e.id, Tracked { kind: e.kind, pos: e.pos });
                match e.kind {
                    PointerKind::Pen | PointerKind::PenEraser => {
                        // The pen always wins: drop whatever the fingers were doing.
                        if let Mode::Draw { .. } = self.mode {
                            out.push(Gesture::StrokeCancel);
                        }
                        self.mode = Mode::Draw { id: e.id, started: e.time };
                        out.push(Gesture::StrokeBegin {
                            pos: e.pos,
                            pressure: e.pressure,
                            time: e.time,
                            erase: e.kind == PointerKind::PenEraser,
                        });
                    }
                    PointerKind::Mouse => {
                        if self.mode == Mode::Idle {
                            if e.button == MouseButton::Primary {
                                self.mode = Mode::Draw { id: e.id, started: e.time };
                                out.push(Gesture::StrokeBegin { pos: e.pos, pressure: e.pressure, time: e.time, erase: false });
                            } else {
                                self.mode = Mode::Pan { id: e.id };
                                self.last_time = e.time;
                                self.velocity = [0.0; 2];
                                self.zoom_velocity = 0.0;
                            }
                        }
                    }
                    PointerKind::Touch => {
                        let touches = self.touches();
                        match self.mode {
                            Mode::Idle if touches.len() == 1 => {
                                if self.pen_seen {
                                    self.mode = Mode::Pan { id: e.id };
                                    self.last_time = e.time;
                                    self.velocity = [0.0; 2];
                                    self.zoom_velocity = 0.0;
                                } else {
                                    self.mode = Mode::Draw { id: e.id, started: e.time };
                                    out.push(Gesture::StrokeBegin { pos: e.pos, pressure: e.pressure, time: e.time, erase: false });
                                }
                            }
                            Mode::Draw { id, started } if self.pointers.get(&id).map(|t| t.kind) == Some(PointerKind::Touch) => {
                                if e.time - started < CANCEL_WINDOW {
                                    out.push(Gesture::StrokeCancel);
                                } else {
                                    out.push(Gesture::StrokeEnd);
                                }
                                self.start_pinch(id, e.id, e.time);
                            }
                            Mode::Pan { id } if self.pointers.get(&id).map(|t| t.kind) == Some(PointerKind::Touch) => {
                                self.start_pinch(id, e.id, e.time);
                            }
                            Mode::Idle | Mode::Blocked if touches.len() >= 2 => {
                                self.start_pinch(touches[0], touches[1], e.time);
                            }
                            _ => {}
                        }
                    }
                }
            }
            Phase::Move => {
                let Some(t) = self.pointers.get_mut(&e.id) else { return };
                let prev = t.pos;
                t.pos = e.pos;
                match self.mode {
                    Mode::Draw { id, .. } if id == e.id => {
                        out.push(Gesture::StrokeMove { pos: e.pos, pressure: e.pressure, time: e.time });
                    }
                    Mode::Pan { id } if id == e.id => {
                        let delta = [e.pos[0] - prev[0], e.pos[1] - prev[1]];
                        self.last_center = e.pos;
                        self.track_velocity(delta, 0.0, e.time);
                        out.push(Gesture::Pan { delta });
                    }
                    Mode::Pinch { a, b } if a == e.id || b == e.id => {
                        let (pa, pb) = (self.pointers[&a].pos, self.pointers[&b].pos);
                        let c = mid(pa, pb);
                        let d = dist(pa, pb);
                        let delta = [c[0] - self.last_center[0], c[1] - self.last_center[1]];
                        let dlog = (d / self.last_dist).log2();
                        self.last_center = c;
                        self.last_dist = d;
                        self.track_velocity(delta, dlog, e.time);
                        out.push(Gesture::Pan { delta });
                        out.push(Gesture::Zoom { center: c, dlog });
                        let a = angle(pa, pb);
                        let da = canvas_core::camera::wrap_angle(a - self.last_angle);
                        self.last_angle = a;
                        if self.rotating {
                            out.push(Gesture::Rotate { center: c, angle: da });
                        } else {
                            self.twist += da;
                            if self.twist.abs() > ROTATE_THRESHOLD {
                                self.rotating = true;
                                out.push(Gesture::Rotate { center: c, angle: self.twist });
                            }
                        }
                    }
                    _ => {}
                }
            }
            Phase::Up | Phase::Cancel => {
                if self.pointers.remove(&e.id).is_none() {
                    return;
                }
                match self.mode {
                    Mode::Draw { id, .. } if id == e.id => {
                        out.push(if e.phase == Phase::Up { Gesture::StrokeEnd } else { Gesture::StrokeCancel });
                        self.mode = if self.touches().is_empty() { Mode::Idle } else { Mode::Blocked };
                    }
                    Mode::Pan { id } if id == e.id => {
                        if let Some(f) = self.fling(e.time) {
                            out.push(f);
                        }
                        self.mode = if self.touches().is_empty() { Mode::Idle } else { Mode::Blocked };
                    }
                    Mode::Pinch { a, b } if a == e.id || b == e.id => {
                        if let Some(f) = self.fling(e.time) {
                            out.push(f);
                        }
                        out.push(Gesture::PinchEnd { center: self.last_center });
                        self.mode = Mode::Blocked;
                    }
                    _ => {}
                }
                if self.mode == Mode::Blocked && self.touches().is_empty() {
                    self.mode = Mode::Idle;
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ev(id: u64, kind: PointerKind, phase: Phase, pos: [f64; 2], time: f64) -> PointerEvent {
        PointerEvent { id, kind, phase, pos, pressure: 0.5, time, button: MouseButton::Primary }
    }

    #[test]
    fn finger_draws_until_pen_seen() {
        let mut g = Gestures::default();
        let mut out = vec![];
        g.handle(&ev(1, PointerKind::Touch, Phase::Down, [0.0, 0.0], 0.0), &mut out);
        assert!(out.iter().any(|g| matches!(g, Gesture::StrokeBegin { .. })));
        g.handle(&ev(1, PointerKind::Touch, Phase::Up, [0.0, 0.0], 0.1), &mut out);
        g.handle(&ev(9, PointerKind::Pen, Phase::Down, [0.0, 0.0], 1.0), &mut out);
        g.handle(&ev(9, PointerKind::Pen, Phase::Up, [0.0, 0.0], 1.1), &mut out);
        out.clear();
        g.handle(&ev(2, PointerKind::Touch, Phase::Down, [0.0, 0.0], 2.0), &mut out);
        g.handle(&ev(2, PointerKind::Touch, Phase::Move, [5.0, 0.0], 2.01), &mut out);
        assert!(!out.iter().any(|g| matches!(g, Gesture::StrokeBegin { .. })));
        assert!(out.contains(&Gesture::Pan { delta: [5.0, 0.0] }));
    }

    #[test]
    fn second_finger_cancels_and_pinches() {
        let mut g = Gestures::default();
        let mut out = vec![];
        g.handle(&ev(1, PointerKind::Touch, Phase::Down, [0.0, 0.0], 0.0), &mut out);
        g.handle(&ev(2, PointerKind::Touch, Phase::Down, [100.0, 0.0], 0.05), &mut out);
        assert!(out.contains(&Gesture::StrokeCancel));
        out.clear();
        g.handle(&ev(2, PointerKind::Touch, Phase::Move, [200.0, 0.0], 0.06), &mut out);
        let zoom = out.iter().find_map(|g| if let Gesture::Zoom { dlog, .. } = g { Some(*dlog) } else { None });
        assert!((zoom.unwrap() - 1.0).abs() < 1e-9);
        // Lifting one finger must not start drawing with the other.
        g.handle(&ev(2, PointerKind::Touch, Phase::Up, [200.0, 0.0], 0.07), &mut out);
        out.clear();
        g.handle(&ev(1, PointerKind::Touch, Phase::Move, [3.0, 0.0], 0.08), &mut out);
        assert!(out.iter().all(|g| !matches!(g, Gesture::StrokeMove { .. } | Gesture::StrokeBegin { .. })));
    }

    #[test]
    fn twist_rotates_only_past_threshold() {
        let mut g = Gestures::default();
        let mut out = Vec::new();
        g.handle(&ev(1, PointerKind::Touch, Phase::Down, [100.0, 500.0], 0.0), &mut out);
        g.handle(&ev(2, PointerKind::Touch, Phase::Down, [300.0, 500.0], 0.01), &mut out);
        let rot = |out: &Vec<Gesture>| out.iter().filter_map(|g| if let Gesture::Rotate { angle, .. } = g { Some(*angle) } else { None }).sum::<f64>();
        // A small wobble (~3°) does not rotate.
        out.clear();
        g.handle(&ev(2, PointerKind::Touch, Phase::Move, [300.0, 505.0], 0.02), &mut out);
        assert_eq!(rot(&out), 0.0);
        // Twisting to 45° rotates by the full twist, not just the part past the threshold.
        out.clear();
        let mut total = 0.0;
        for k in 1..=10 {
            let a = (k as f64 / 10.0) * std::f64::consts::FRAC_PI_4;
            g.handle(&ev(2, PointerKind::Touch, Phase::Move, [200.0 + 100.0 * a.cos(), 500.0 + 100.0 * a.sin()], 0.02 + k as f64 * 0.01), &mut out);
            total += rot(&out);
            out.clear();
        }
        g.handle(&ev(2, PointerKind::Touch, Phase::Move, [200.0 + 100.0 * std::f64::consts::FRAC_PI_4.cos(), 500.0 + 100.0 * std::f64::consts::FRAC_PI_4.sin()], 0.2), &mut out);
        // Finger 1 is at (100, 500), so the pair's angle is measured from it, not the centre.
        let expected = angle([100.0, 500.0], [200.0 + 100.0 * std::f64::consts::FRAC_PI_4.cos(), 500.0 + 100.0 * std::f64::consts::FRAC_PI_4.sin()]);
        assert!((total - expected).abs() < 1e-9, "{total} vs {expected}");
        out.clear();
        g.handle(&ev(1, PointerKind::Touch, Phase::Up, [100.0, 500.0], 0.3), &mut out);
        assert!(out.iter().any(|g| matches!(g, Gesture::PinchEnd { .. })));
    }
}
