//! Small vector paths keep the original run icons sharp at every scale.
use crate::scene::Scene;
#[derive(Clone, Copy)]
pub(crate) enum Icon {
    Play,
    Stop,
    Settings,
    Save,
    Unload,
    Reset,
    Chevron(bool),
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
        let scale = rect[2] / 512.0 * 1.3;
        let point = |p: [f32; 2]| {
            [
                rect[0] + rect[2] * 0.5 + (p[0] - 256.0) * scale,
                rect[1] + rect[3] * 0.5 + (p[1] - 256.0) * scale,
            ]
        };
        self.ring(point([256.0, 256.0]), 156.0 * scale, 24.0 * scale, color);
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
