//! Caméra orbitale 3D en perspective : rotation autour d'un point visé,
//! zoom par la distance, déplacement dans le plan de l'écran, cadrage.
//!
//! Repère : l'axe `y` du monde pointe vers le bas de l'écran (comme en 2D),
//! si bien qu'à `yaw = pitch = 0` la vue 3D ressemble à la vue 2D.

use crate::vec2::Vec2;
use crate::vec3::Vec3;

pub const MIN_DISTANCE: f32 = 20.0;
pub const MAX_DISTANCE: f32 = 200_000.0;
/// Points plus proches que ça de la caméra (en profondeur) : non dessinés.
pub const NEAR: f32 = 1.0;
const MAX_PITCH: f32 = 1.55;
/// Radians par pixel de glisser.
const ORBIT_SPEED: f32 = 0.008;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct OrbitCamera {
    pub target: Vec3,
    /// Rotation autour de l'axe vertical (radians).
    pub yaw: f32,
    /// Inclinaison (radians, bornée à ±89°).
    pub pitch: f32,
    pub distance: f32,
    /// Champ de vision vertical (radians).
    pub fov_y: f32,
}

impl Default for OrbitCamera {
    fn default() -> Self {
        Self {
            target: Vec3::ZERO,
            yaw: 0.5,
            pitch: 0.35,
            distance: 800.0,
            fov_y: 50f32.to_radians(),
        }
    }
}

/// Point projeté à l'écran.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Projected {
    pub screen: Vec2,
    /// Distance le long de l'axe de visée (plus grand = plus loin).
    pub depth: f32,
    /// Pixels par unité du monde à cette profondeur.
    pub scale: f32,
}

/// Repère de la caméra.
#[derive(Clone, Copy, Debug)]
struct Basis {
    eye: Vec3,
    right: Vec3,
    up: Vec3,
    forward: Vec3,
}

/// Projection d'une caméra figée pour une vue donnée.
#[derive(Clone, Copy, Debug)]
pub struct Projector {
    basis: Basis,
    focal: f32,
    view_center: Vec2,
}

impl Projector {
    /// Projette un point ; `None` s'il est derrière la caméra.
    pub fn project(&self, p: Vec3) -> Option<Projected> {
        let b = &self.basis;
        let d = p - b.eye;
        let depth = d.dot(b.forward);
        if depth < NEAR {
            return None;
        }
        let scale = self.focal / depth;
        let screen = self.view_center + Vec2::new(d.dot(b.right), -d.dot(b.up)) * scale;
        Some(Projected {
            screen,
            depth,
            scale,
        })
    }
}

impl OrbitCamera {
    fn basis(&self) -> Basis {
        let (sy, cy) = self.yaw.sin_cos();
        let (sp, cp) = self.pitch.sin_cos();
        let forward = Vec3::new(sy * cp, sp, cy * cp);
        let world_up = Vec3::new(0.0, -1.0, 0.0);
        let right = forward.cross(world_up).normalized();
        let up = right.cross(forward);
        Basis {
            eye: self.target - forward * self.distance,
            right,
            up,
            forward,
        }
    }

    /// Position de l'œil.
    pub fn eye(&self) -> Vec3 {
        self.basis().eye
    }

    /// Distance focale en pixels pour une vue de hauteur `view_h`.
    pub fn focal(&self, view_h: f32) -> f32 {
        (view_h.max(1.0) * 0.5) / (self.fov_y * 0.5).tan()
    }

    /// Projette un point ; `None` s'il est derrière la caméra.
    pub fn project(&self, p: Vec3, view_center: Vec2, view_h: f32) -> Option<Projected> {
        self.projector(view_center, view_h).project(p)
    }

    /// Projecteur au repère précalculé (pour projeter beaucoup de points).
    pub fn projector(&self, view_center: Vec2, view_h: f32) -> Projector {
        Projector {
            basis: self.basis(),
            focal: self.focal(view_h),
            view_center,
        }
    }

    /// Point du monde affiché en `screen` à la profondeur `depth`.
    pub fn unproject(&self, screen: Vec2, depth: f32, view_center: Vec2, view_h: f32) -> Vec3 {
        let b = self.basis();
        let k = depth / self.focal(view_h);
        let s = (screen - view_center) * k;
        b.eye + b.right * s.x - b.up * s.y + b.forward * depth
    }

    /// Tourne autour du point visé (glisser à l'écran).
    pub fn orbit(&mut self, screen_delta: Vec2) {
        self.yaw -= screen_delta.x * ORBIT_SPEED;
        self.pitch = (self.pitch + screen_delta.y * ORBIT_SPEED).clamp(-MAX_PITCH, MAX_PITCH);
    }

    /// Déplace le point visé dans le plan de l'écran (le monde suit le
    /// pointeur au niveau du point visé).
    pub fn pan(&mut self, screen_delta: Vec2, view_h: f32) {
        let b = self.basis();
        let k = self.distance / self.focal(view_h);
        self.target -= b.right * (screen_delta.x * k) - b.up * (screen_delta.y * k);
    }

    /// Rapproche (`factor` < 1) ou éloigne la caméra.
    pub fn dolly(&mut self, factor: f32) {
        self.distance = (self.distance * factor).clamp(MIN_DISTANCE, MAX_DISTANCE);
    }

    /// Cadre tous les points (sphère englobante) dans une vue de taille
    /// `view_size`, avec une marge en pixels. Sans point, ne fait rien.
    pub fn fit(
        &mut self,
        points: impl IntoIterator<Item = Vec3> + Clone,
        view_size: Vec2,
        margin: f32,
    ) {
        let mut it = points.clone().into_iter();
        let Some(first) = it.next() else {
            return;
        };
        let (mut lo, mut hi) = (first, first);
        for p in it {
            lo = lo.min(p);
            hi = hi.max(p);
        }
        let center = (lo + hi) * 0.5;
        let radius = points
            .into_iter()
            .map(|p| (p - center).length())
            .fold(0.0f32, f32::max)
            .max(30.0);
        let h = view_size.y.max(10.0);
        let w = view_size.x.max(10.0);
        // Marge : on vise un disque apparent plus petit que la vue.
        let shrink = ((h.min(w) - 2.0 * margin) / h.min(w)).clamp(0.3, 1.0);
        let half_y = self.fov_y * 0.5;
        let half_x = ((half_y.tan()) * w / h).atan();
        let half = half_y.min(half_x);
        self.target = center;
        self.distance =
            (radius / (half.tan() * shrink) + radius * 0.5).clamp(MIN_DISTANCE, MAX_DISTANCE);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const VC: Vec2 = Vec2::new(400.0, 300.0);
    const H: f32 = 600.0;

    fn front() -> OrbitCamera {
        OrbitCamera {
            yaw: 0.0,
            pitch: 0.0,
            ..OrbitCamera::default()
        }
    }

    #[test]
    fn front_view_matches_2d_orientation() {
        let cam = front();
        let c = cam.project(Vec3::ZERO, VC, H).unwrap();
        assert!((c.screen - VC).length() < 1e-3);
        let r = cam.project(Vec3::new(10.0, 0.0, 0.0), VC, H).unwrap();
        let d = cam.project(Vec3::new(0.0, 10.0, 0.0), VC, H).unwrap();
        assert!(r.screen.x > VC.x, "x vers la droite");
        assert!(d.screen.y > VC.y, "y vers le bas");
        // Plus loin = plus petit.
        let far = cam.project(Vec3::new(10.0, 0.0, 300.0), VC, H).unwrap();
        assert!(far.scale < r.scale && far.depth > r.depth);
    }

    #[test]
    fn behind_camera_is_culled() {
        let cam = front();
        let behind = cam.eye() - Vec3::new(0.0, 0.0, 10.0);
        assert!(cam.project(behind, VC, H).is_none());
    }

    #[test]
    fn unproject_round_trip() {
        let cam = OrbitCamera {
            target: Vec3::new(30.0, -20.0, 5.0),
            yaw: 1.1,
            pitch: -0.4,
            distance: 500.0,
            ..OrbitCamera::default()
        };
        let p = Vec3::new(70.0, 15.0, -40.0);
        let s = cam.project(p, VC, H).unwrap();
        let back = cam.unproject(s.screen, s.depth, VC, H);
        assert!((back - p).length() < 1e-2, "{back:?}");
    }

    #[test]
    fn orbit_keeps_target_centered_and_clamps_pitch() {
        let mut cam = OrbitCamera {
            target: Vec3::new(5.0, 6.0, 7.0),
            ..OrbitCamera::default()
        };
        cam.orbit(Vec2::new(120.0, 10_000.0));
        assert!(cam.pitch <= MAX_PITCH);
        let c = cam.project(cam.target, VC, H).unwrap();
        assert!((c.screen - VC).length() < 1e-2);
        assert!((c.depth - cam.distance).abs() < 1e-2);
    }

    #[test]
    fn pan_follows_pointer_and_dolly_clamps() {
        let mut cam = OrbitCamera::default();
        let before = cam.project(cam.target, VC, H).unwrap().screen;
        let p = cam.target;
        cam.pan(Vec2::new(40.0, -25.0), H);
        let after = cam.project(p, VC, H).unwrap().screen;
        assert!((after - before - Vec2::new(40.0, -25.0)).length() < 1e-2);
        cam.dolly(1e-9);
        assert_eq!(cam.distance, MIN_DISTANCE);
    }

    #[test]
    fn fit_contains_all_points() {
        let mut cam = OrbitCamera::default();
        let pts = vec![
            Vec3::new(-300.0, 0.0, 50.0),
            Vec3::new(400.0, 120.0, -200.0),
            Vec3::new(0.0, -250.0, 300.0),
            Vec3::new(10.0, 10.0, 10.0),
        ];
        let size = Vec2::new(800.0, 600.0);
        cam.fit(pts.iter().copied(), size, 40.0);
        for p in pts {
            let s = cam.project(p, size * 0.5, size.y).unwrap().screen;
            assert!(
                s.x >= 0.0 && s.x <= size.x && s.y >= 0.0 && s.y <= size.y,
                "{s:?}"
            );
        }
    }
}
