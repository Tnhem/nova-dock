//! 1-bit drawing surface with the handful of primitives the pages need (PIL-like, inclusive coordinates).

pub const W: i32 = 128;
pub const H: i32 = 64;

pub const BAYER: [[f32; 4]; 4] = [
    [0.5 / 16.0, 8.5 / 16.0, 2.5 / 16.0, 10.5 / 16.0],
    [12.5 / 16.0, 4.5 / 16.0, 14.5 / 16.0, 6.5 / 16.0],
    [3.5 / 16.0, 11.5 / 16.0, 1.5 / 16.0, 9.5 / 16.0],
    [15.5 / 16.0, 7.5 / 16.0, 13.5 / 16.0, 5.5 / 16.0],
];

/// Ordered-dither threshold for a pixel.
pub fn bayer(x: i32, y: i32) -> f32 {
    BAYER[(y & 3) as usize][(x & 3) as usize]
}

#[derive(Clone, PartialEq)]
pub struct Canvas {
    pub w: i32,
    pub h: i32,
    pub px: Vec<bool>,
}

impl Canvas {
    pub fn new(w: i32, h: i32) -> Self {
        Canvas { w, h, px: vec![false; (w.max(0) * h.max(0)) as usize] }
    }

    pub fn screen() -> Self {
        Self::new(W, H)
    }

    #[inline]
    pub fn get(&self, x: i32, y: i32) -> bool {
        x >= 0 && y >= 0 && x < self.w && y < self.h && self.px[(y * self.w + x) as usize]
    }

    #[inline]
    pub fn set(&mut self, x: i32, y: i32, v: bool) {
        if x >= 0 && y >= 0 && x < self.w && y < self.h {
            self.px[(y * self.w + x) as usize] = v;
        }
    }

    #[inline]
    pub fn on(&mut self, x: i32, y: i32) {
        self.set(x, y, true);
    }

    pub fn line(&mut self, x0: i32, y0: i32, x1: i32, y1: i32) {
        let (mut x, mut y) = (x0, y0);
        let dx = (x1 - x0).abs();
        let dy = -(y1 - y0).abs();
        let sx = if x0 < x1 { 1 } else { -1 };
        let sy = if y0 < y1 { 1 } else { -1 };
        let mut err = dx + dy;
        loop {
            self.on(x, y);
            if x == x1 && y == y1 {
                break;
            }
            let e2 = 2 * err;
            if e2 >= dy {
                err += dy;
                x += sx;
            }
            if e2 <= dx {
                err += dx;
                y += sy;
            }
        }
    }

    pub fn linef(&mut self, x0: f32, y0: f32, x1: f32, y1: f32) {
        self.line(x0.round() as i32, y0.round() as i32, x1.round() as i32, y1.round() as i32);
    }

    /// Connect points with lines (PIL `line` with a point list).
    pub fn polyline(&mut self, pts: &[(i32, i32)]) {
        for w in pts.windows(2) {
            self.line(w[0].0, w[0].1, w[1].0, w[1].1);
        }
        if pts.len() == 1 {
            self.on(pts[0].0, pts[0].1);
        }
    }

    pub fn fill_rect(&mut self, x0: i32, y0: i32, x1: i32, y1: i32, v: bool) {
        for y in y0.max(0)..=y1.min(self.h - 1) {
            for x in x0.max(0)..=x1.min(self.w - 1) {
                self.px[(y * self.w + x) as usize] = v;
            }
        }
    }

    pub fn rect(&mut self, x0: i32, y0: i32, x1: i32, y1: i32) {
        self.line(x0, y0, x1, y0);
        self.line(x0, y1, x1, y1);
        self.line(x0, y0, x0, y1);
        self.line(x1, y0, x1, y1);
    }

    /// Ellipse inside the box (inclusive), filled with `v` or outlined.
    pub fn ellipse(&mut self, x0: f32, y0: f32, x1: f32, y1: f32, fill: Option<bool>) {
        let (cx, cy) = ((x0 + x1) / 2.0, (y0 + y1) / 2.0);
        let (rx, ry) = (((x1 - x0) / 2.0).max(0.5), ((y1 - y0) / 2.0).max(0.5));
        match fill {
            Some(v) => {
                for y in y0.floor() as i32..=y1.ceil() as i32 {
                    for x in x0.floor() as i32..=x1.ceil() as i32 {
                        let (dx, dy) = ((x as f32 - cx) / (rx + 0.5), (y as f32 - cy) / (ry + 0.5));
                        if dx * dx + dy * dy <= 1.0 {
                            self.set(x, y, v);
                        }
                    }
                }
            }
            None => {
                let steps = ((rx + ry) * 4.0).max(16.0) as i32;
                let mut prev = None;
                for i in 0..=steps {
                    let a = i as f32 / steps as f32 * std::f32::consts::TAU;
                    let p = ((cx + rx * a.cos()).round() as i32, (cy + ry * a.sin()).round() as i32);
                    if let Some((px, py)) = prev {
                        self.line(px, py, p.0, p.1);
                    }
                    prev = Some(p);
                }
            }
        }
    }

    /// Even-odd scanline polygon fill, sampling pixel centres.
    pub fn polygon(&mut self, pts: &[(f32, f32)], v: bool) {
        if pts.len() < 3 {
            return;
        }
        let ymin = pts.iter().map(|p| p.1).fold(f32::MAX, f32::min).floor().max(0.0) as i32;
        let ymax = pts.iter().map(|p| p.1).fold(f32::MIN, f32::max).ceil().min((self.h - 1) as f32) as i32;
        let mut xs = Vec::with_capacity(8);
        for y in ymin..=ymax {
            let sy = y as f32 + 0.5;
            xs.clear();
            for i in 0..pts.len() {
                let (a, b) = (pts[i], pts[(i + 1) % pts.len()]);
                if (a.1 <= sy && b.1 > sy) || (b.1 <= sy && a.1 > sy) {
                    xs.push(a.0 + (sy - a.1) / (b.1 - a.1) * (b.0 - a.0));
                }
            }
            xs.sort_by(|a, b| a.partial_cmp(b).unwrap());
            for pair in xs.chunks(2) {
                if pair.len() == 2 {
                    let (xa, xb) = ((pair[0] - 0.5).ceil() as i32, (pair[1] - 0.5).floor() as i32);
                    for x in xa..=xb {
                        self.set(x, y, v);
                    }
                }
            }
        }
        // outline too, so thin shapes don't vanish (PIL draws the outline with the fill)
        for i in 0..pts.len() {
            let (a, b) = (pts[i], pts[(i + 1) % pts.len()]);
            if v {
                self.linef(a.0, a.1, b.0, b.1);
            }
        }
    }

    /// Copy `src` (including its off pixels) to (x, y), clipped.
    pub fn paste(&mut self, src: &Canvas, x: i32, y: i32) {
        for sy in 0..src.h {
            for sx in 0..src.w {
                self.set(x + sx, y + sy, src.px[(sy * src.w + sx) as usize]);
            }
        }
    }

    /// OR `src`'s lit pixels onto (x, y).
    pub fn overlay(&mut self, src: &Canvas, x: i32, y: i32) {
        for sy in 0..src.h {
            for sx in 0..src.w {
                if src.px[(sy * src.w + sx) as usize] {
                    self.on(x + sx, y + sy);
                }
            }
        }
    }

    /// Bounding box of lit pixels: (x0, y0, x1 exclusive, y1 exclusive).
    pub fn bbox(&self) -> Option<(i32, i32, i32, i32)> {
        let (mut x0, mut y0, mut x1, mut y1) = (i32::MAX, i32::MAX, -1, -1);
        for y in 0..self.h {
            for x in 0..self.w {
                if self.px[(y * self.w + x) as usize] {
                    x0 = x0.min(x);
                    y0 = y0.min(y);
                    x1 = x1.max(x);
                    y1 = y1.max(y);
                }
            }
        }
        (x1 >= 0).then_some((x0, y0, x1 + 1, y1 + 1))
    }

    pub fn crop(&self, x0: i32, y0: i32, x1: i32, y1: i32) -> Canvas {
        let mut c = Canvas::new(x1 - x0, y1 - y0);
        for y in 0..c.h {
            for x in 0..c.w {
                c.px[(y * c.w + x) as usize] = self.get(x0 + x, y0 + y);
            }
        }
        c
    }

    pub fn scale(&self, k: i32) -> Canvas {
        let mut c = Canvas::new(self.w * k, self.h * k);
        for y in 0..c.h {
            for x in 0..c.w {
                c.px[(y * c.w + x) as usize] = self.px[((y / k) * self.w + x / k) as usize];
            }
        }
        c
    }
}
