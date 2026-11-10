//! Small vector paths keep the original run icons sharp at every scale.
use crate::scene::Scene;

const LOGO_RING_RADIUS: f32 = 156.0;
const LOGO_RING_STROKE: f32 = 24.0;

fn logo_scale(size: f32) -> f32 {
    size / 512.0 * 1.3
}
pub(crate) fn logo_outer_radius(size: f32) -> f32 {
    (LOGO_RING_RADIUS + LOGO_RING_STROKE * 0.5) * logo_scale(size)
}
#[derive(Clone, Copy)]
pub(crate) enum Icon {
    Play,
    Scope,
    Pin(bool),
    More,
    Stop,
    Settings,
    Theme,
    Sliders,
    Knob,
    Save,
    Unload,
    Reset,
    Chevron(bool),
}
impl Icon {
    /// Space before the drawn stroke inside a 28px header action button.
    pub(crate) fn header_inset(self) -> f32 {
        match self {
            Self::Settings => 14.0 - 7.0 - 1.5 * 0.5,
            Self::Save => 14.0 - 6.0 - 1.5 * 0.5,
            Self::Unload => 14.0 - 6.0,
            _ => unreachable!("only Settings, Save and Unload are header actions"),
        }
    }
}
impl Scene {
    pub fn line(&mut self, a: [f32; 2], b: [f32; 2], width: f32, color: [f32; 4]) {
        let dx = b[0] - a[0];
        let dy = b[1] - a[1];
        let r = width * 0.5;
        self.primitive(
            [
                a[0].min(b[0]) - r,
                a[1].min(b[1]) - r,
                dx.abs() + width,
                dy.abs() + width,
            ],
            color,
            [r, 2.0, dx, dy],
        );
    }
    pub fn triangle(&mut self, a: [f32; 2], b: [f32; 2], c: [f32; 2], color: [f32; 4]) {
        self.primitive([a[0], a[1], b[0], b[1]], color, [0.0, 4.0, c[0], c[1]]);
    }
    pub fn ring(&mut self, center: [f32; 2], radius: f32, width: f32, color: [f32; 4]) {
        self.primitive(
            [
                center[0] - radius,
                center[1] - radius,
                radius * 2.0,
                radius * 2.0,
            ],
            color,
            [width, 5.0, 0.0, 0.0],
        );
    }
    pub fn icon(&mut self, icon: Icon, center: [f32; 2], color: [f32; 4]) {
        let [x, y] = center;
        let p = |a, b| [x + a, y + b];
        match icon {
            Icon::Theme => {
                self.ring(center, 6.5, 1.2, color);
                let point = |i: f32| {
                    let angle = std::f32::consts::FRAC_PI_2 + i * std::f32::consts::PI / 16.0;
                    p(angle.cos() * 6.5, angle.sin() * 6.5)
                };
                for i in 0..16 {
                    self.triangle(center, point(i as f32), point((i + 1) as f32), color);
                }
            }
            Icon::Sliders => {
                for (dx, dy) in [(-5.0, -2.0), (0.0, 3.0), (5.0, -3.0)] {
                    self.line(p(dx, -6.0), p(dx, 6.0), 1.0, color);
                    self.shape([x + dx - 1.5, y + dy - 1.5, 3.0, 3.0], color, 0.7);
                }
            }
            Icon::Knob => {
                self.ring([x, y], 6.0, 1.2, color);
                self.line(p(0.0, -2.0), p(0.0, -5.0), 1.2, color);
            }
            Icon::More => {
                for dx in [-5.0, 0.0, 5.0] {
                    self.shape([x + dx - 1.2, y - 1.2, 2.4, 2.4], color, 1.2);
                }
            }
            Icon::Scope => {
                let point = |t: f32| p((t - 0.5) * 14.0, -(t * std::f32::consts::TAU).sin() * 4.0);
                let mut previous = point(0.0);
                for i in 1..=32 {
                    let next = point(i as f32 / 32.0);
                    self.line(previous, next, 1.2, color);
                    previous = next;
                }
            }
            Icon::Pin(pinned) => {
                let q = |a: f32, b: f32| {
                    if pinned {
                        p(a, b)
                    } else {
                        p(a * 0.82 - b * 0.57, a * 0.57 + b * 0.82)
                    }
                };
                let points = [
                    q(-3.0, -6.0),
                    q(3.0, -6.0),
                    q(2.0, -1.0),
                    q(5.0, 3.0),
                    q(-5.0, 3.0),
                    q(-2.0, -1.0),
                    q(-3.0, -6.0),
                ];
                for pair in points.windows(2) {
                    self.line(pair[0], pair[1], 1.2, color);
                }
                self.line(q(0.0, 3.0), q(0.0, 8.0), 1.2, color);
            }
            Icon::Play => self.triangle(p(-4.0, -6.0), p(6.0, 0.0), p(-4.0, 6.0), color),
            Icon::Stop => self.shape([x - 6.0, y - 6.0, 12.0, 12.0], color, 1.0),
            Icon::Chevron(open) => {
                if open {
                    self.triangle(p(-4.0, -2.0), p(4.0, -2.0), p(0.0, 3.0), color)
                } else {
                    self.triangle(p(-2.0, -4.0), p(3.0, 0.0), p(-2.0, 4.0), color)
                }
            }
            Icon::Settings => {
                for (a, b) in [(-5.0, -3.0), (0.0, 3.0), (5.0, -1.0)] {
                    self.line(p(-7.0, a), p(b - 2.0, a), 1.5, color);
                    self.line(p(b + 2.0, a), p(7.0, a), 1.5, color);
                    self.ring(p(b, a), 2.0, 1.5, color);
                }
            }
            Icon::Save => {
                for points in [
                    vec![
                        p(-6.0, -7.0),
                        p(3.0, -7.0),
                        p(6.0, -4.0),
                        p(6.0, 7.0),
                        p(-6.0, 7.0),
                        p(-6.0, -7.0),
                    ],
                    vec![p(-3.0, -7.0), p(-3.0, -2.0), p(3.0, -2.0), p(3.0, -7.0)],
                    vec![p(-3.0, 7.0), p(-3.0, 2.0), p(3.0, 2.0), p(3.0, 7.0)],
                ] {
                    for points in points.windows(2) {
                        self.line(points[0], points[1], 1.5, color);
                    }
                }
            }
            Icon::Unload => {
                self.triangle(p(0.0, -6.0), p(6.0, 2.0), p(-6.0, 2.0), color);
                self.shape([x - 6.0, y + 4.0, 12.0, 2.0], color, 1.0);
            }
            Icon::Reset => {
                let mut previous = p(-5.5, 0.0);
                for i in 1..=32 {
                    let a = std::f32::consts::PI - i as f32 / 32.0 * 1.75 * std::f32::consts::PI;
                    let next = p(a.cos() * 5.5, a.sin() * 5.5);
                    self.line(previous, next, 1.5, color);
                    previous = next;
                }
                self.line(p(-5.5, -6.0), p(-5.5, -2.5), 1.5, color);
                self.line(p(-5.5, -2.5), p(-2.0, -2.5), 1.5, color);
            }
        }
    }
    pub fn logo(&mut self, rect: [f32; 4], color: [f32; 4]) {
        let scale = logo_scale(rect[2]);
        let point = |p: [f32; 2]| {
            [
                rect[0] + rect[2] * 0.5 + (p[0] - 256.0) * scale,
                rect[1] + rect[3] * 0.5 + (p[1] - 256.0) * scale,
            ]
        };
        self.ring(
            point([256.0, 256.0]),
            LOGO_RING_RADIUS * scale,
            LOGO_RING_STROKE * scale,
            color,
        );
        // One distance field for the whole wave avoids dark seams and swollen
        // edges where independently antialiased line segments overlap.
        let origin = point([156.0, 203.0]);
        self.primitive(
            [origin[0], origin[1], 200.0 * scale, 106.0 * scale],
            color,
            [scale, 7.0, 0.0, 0.0],
        );
    }
}
