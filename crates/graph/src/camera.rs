//! Caméra 2D : déplacement et zoom, conversions monde ↔ écran.

use crate::vec2::Vec2;

pub const MIN_ZOOM: f32 = 0.05;
pub const MAX_ZOOM: f32 = 8.0;

/// `center` : point du monde affiché au centre de la vue.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Camera2D {
    pub center: Vec2,
    pub zoom: f32,
}

impl Default for Camera2D {
    fn default() -> Self {
        Self {
            center: Vec2::ZERO,
            zoom: 1.0,
        }
    }
}

impl Camera2D {
    pub fn to_screen(&self, world: Vec2, view_center: Vec2) -> Vec2 {
        (world - self.center) * self.zoom + view_center
    }

    pub fn to_world(&self, screen: Vec2, view_center: Vec2) -> Vec2 {
        (screen - view_center) / self.zoom + self.center
    }

    /// Zoome d'un facteur en gardant fixe le point du monde sous `screen`.
    pub fn zoom_at(&mut self, screen: Vec2, view_center: Vec2, factor: f32) {
        let before = self.to_world(screen, view_center);
        self.zoom = (self.zoom * factor).clamp(MIN_ZOOM, MAX_ZOOM);
        let after = self.to_world(screen, view_center);
        self.center += before - after;
    }

    /// Déplace la vue d'un déplacement exprimé à l'écran.
    pub fn pan(&mut self, screen_delta: Vec2) {
        self.center -= screen_delta / self.zoom;
    }

    /// Cadre tous les points dans une vue de taille `view_size`, avec une
    /// marge (en pixels). Sans point, ne fait rien.
    pub fn fit(&mut self, points: impl IntoIterator<Item = Vec2>, view_size: Vec2, margin: f32) {
        let mut it = points.into_iter();
        let Some(first) = it.next() else {
            return;
        };
        let (mut lo, mut hi) = (first, first);
        for p in it {
            lo = Vec2::new(lo.x.min(p.x), lo.y.min(p.y));
            hi = Vec2::new(hi.x.max(p.x), hi.y.max(p.y));
        }
        self.center = (lo + hi) * 0.5;
        let w = (hi.x - lo.x).max(1.0);
        let h = (hi.y - lo.y).max(1.0);
        let avail_w = (view_size.x - 2.0 * margin).max(10.0);
        let avail_h = (view_size.y - 2.0 * margin).max(10.0);
        self.zoom = (avail_w / w).min(avail_h / h).clamp(MIN_ZOOM, 2.0);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trip() {
        let cam = Camera2D {
            center: Vec2::new(10.0, -5.0),
            zoom: 2.5,
        };
        let vc = Vec2::new(400.0, 300.0);
        let w = Vec2::new(33.0, 7.0);
        let back = cam.to_world(cam.to_screen(w, vc), vc);
        assert!((back - w).length() < 1e-4);
        assert_eq!(cam.to_screen(cam.center, vc), vc);
    }

    #[test]
    fn zoom_keeps_point_under_cursor() {
        let mut cam = Camera2D::default();
        let vc = Vec2::new(400.0, 300.0);
        let cursor = Vec2::new(520.0, 180.0);
        let before = cam.to_world(cursor, vc);
        cam.zoom_at(cursor, vc, 3.0);
        assert!((cam.to_world(cursor, vc) - before).length() < 1e-3);
        cam.zoom_at(cursor, vc, 1e6);
        assert_eq!(cam.zoom, MAX_ZOOM);
    }

    #[test]
    fn pan_moves_opposite_in_world() {
        let mut cam = Camera2D {
            center: Vec2::ZERO,
            zoom: 2.0,
        };
        cam.pan(Vec2::new(20.0, 0.0));
        assert_eq!(cam.center, Vec2::new(-10.0, 0.0));
    }

    #[test]
    fn fit_contains_all_points() {
        let mut cam = Camera2D::default();
        let pts = [
            Vec2::new(-100.0, 0.0),
            Vec2::new(300.0, 50.0),
            Vec2::new(0.0, 200.0),
        ];
        let size = Vec2::new(800.0, 600.0);
        cam.fit(pts, size, 40.0);
        for p in pts {
            let s = cam.to_screen(p, size * 0.5);
            assert!(
                s.x >= 39.0 && s.x <= 761.0 && s.y >= 39.0 && s.y <= 561.0,
                "{s:?}"
            );
        }
    }
}
