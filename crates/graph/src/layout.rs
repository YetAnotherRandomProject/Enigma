//! Disposition en forces 2D ou 3D, dans l'esprit de d3-force(-3d) :
//! - répulsion entre tous les nœuds (approximation Barnes-Hut, O(n log n),
//!   octree — qui dégénère en quadtree quand tout est dans le plan) ;
//! - ressorts le long des arêtes (longueur cible, force ∝ 1/degré) ;
//! - légère gravité vers l'origine ;
//! - amortissement des vitesses et « température » `alpha` qui décroît
//!   jusqu'à l'arrêt et qu'on relance lors des interactions.
//!
//! En mode plan (`flat`), la coordonnée `z` reste nulle. En 3D, un nœud peut
//! avoir une profondeur imposée (`z_target`, ex. axe Z lié à un champ).
//! Les nœuds épinglés ne bougent pas mais exercent leurs forces.

use crate::vec3::Vec3;

/// Paramètres de la simulation (unités du monde ≈ pixels au zoom 1).
#[derive(Clone, Copy, Debug)]
pub struct LayoutParams {
    /// Intensité de la répulsion (négative : les nœuds se repoussent).
    pub charge: f32,
    /// Longueur au repos des arêtes.
    pub link_distance: f32,
    /// Attraction vers l'origine.
    pub gravity: f32,
    /// Part de la vitesse perdue à chaque pas (0..1).
    pub velocity_decay: f32,
    /// Critère Barnes-Hut (taille de cellule / distance).
    pub theta: f32,
    /// Vitesse de refroidissement.
    pub alpha_decay: f32,
    /// En dessous, la simulation s'arrête.
    pub alpha_min: f32,
}

impl Default for LayoutParams {
    fn default() -> Self {
        let alpha_min: f32 = 0.001;
        Self {
            charge: -150.0,
            link_distance: 70.0,
            gravity: 0.03,
            velocity_decay: 0.4,
            theta: 0.9,
            // Comme d3 : environ 300 pas pour passer de 1 à alpha_min.
            alpha_decay: 1.0 - alpha_min.powf(1.0 / 300.0),
            alpha_min,
        }
    }
}

/// Simulation de disposition.
#[derive(Clone, Debug)]
pub struct Layout {
    pub pos: Vec<Vec3>,
    vel: Vec<Vec3>,
    pub pinned: Vec<bool>,
    /// Profondeur imposée (3D uniquement) ; `None` : libre.
    pub z_target: Vec<Option<f32>>,
    flat: bool,
    edges: Vec<(u32, u32)>,
    degree: Vec<u32>,
    alpha: f32,
    alpha_target: f32,
    params: LayoutParams,
    tree: Octree,
}

impl Layout {
    /// `edges` : paires d'indices ; boucles et doublons sont ignorés.
    /// La disposition démarre en mode plan (`z` mis à 0).
    pub fn new(positions: Vec<Vec3>, edges: &[(usize, usize)], params: LayoutParams) -> Layout {
        let n = positions.len();
        let mut uniq: Vec<(u32, u32)> = edges
            .iter()
            .filter(|(a, b)| a != b && *a < n && *b < n)
            .map(|&(a, b)| {
                let (a, b) = (a.min(b), a.max(b));
                (a as u32, b as u32)
            })
            .collect();
        uniq.sort_unstable();
        uniq.dedup();
        let mut degree = vec![0u32; n];
        for &(a, b) in &uniq {
            degree[a as usize] += 1;
            degree[b as usize] += 1;
        }
        let mut layout = Layout {
            vel: vec![Vec3::ZERO; n],
            pinned: vec![false; n],
            z_target: vec![None; n],
            flat: true,
            pos: positions,
            edges: uniq,
            degree,
            alpha: 1.0,
            alpha_target: 0.0,
            params,
            tree: Octree::default(),
        };
        layout.flatten();
        layout
    }

    /// Comme [`Layout::new`], mais en 3D : les positions sont gardées telles
    /// quelles.
    pub fn new_3d(positions: Vec<Vec3>, edges: &[(usize, usize)], params: LayoutParams) -> Layout {
        let n = positions.len();
        let mut layout = Layout::new(vec![Vec3::ZERO; n], edges, params);
        layout.pos = positions;
        layout.flat = false;
        layout
    }

    pub fn len(&self) -> usize {
        self.pos.len()
    }

    pub fn is_empty(&self) -> bool {
        self.pos.is_empty()
    }

    pub fn degree(&self, i: usize) -> u32 {
        self.degree.get(i).copied().unwrap_or(0)
    }

    pub fn alpha(&self) -> f32 {
        self.alpha
    }

    pub fn is_flat(&self) -> bool {
        self.flat
    }

    /// Passe en mode plan (`z` = 0) ou en 3D. En 3D, les nœuds libres
    /// partent de leur profondeur imposée, sinon de `initial_z(i)` ; la
    /// simulation est relancée dans les deux cas.
    pub fn set_flat(&mut self, flat: bool, initial_z: impl Fn(usize) -> f32) {
        if flat == self.flat {
            return;
        }
        self.flat = flat;
        if flat {
            self.flatten();
        } else {
            for i in 0..self.pos.len() {
                if !self.pinned[i] {
                    self.pos[i].z = self.z_target[i].unwrap_or_else(|| initial_z(i));
                }
            }
        }
        self.reheat(0.6);
    }

    fn flatten(&mut self) {
        if self.flat {
            self.pos.iter_mut().for_each(|p| p.z = 0.0);
            self.vel.iter_mut().for_each(|v| v.z = 0.0);
        }
    }

    /// La simulation a-t-elle encore besoin de pas ?
    pub fn is_running(&self) -> bool {
        self.alpha >= self.params.alpha_min || self.alpha_target > 0.0
    }

    /// Relance la simulation (au moins jusqu'à `alpha`).
    pub fn reheat(&mut self, alpha: f32) {
        self.alpha = self.alpha.max(alpha);
    }

    /// Température visée (> 0 pendant un glisser-déposer, 0 sinon).
    pub fn set_alpha_target(&mut self, target: f32) {
        self.alpha_target = target;
    }

    /// Fige l'état actuel (plus de mouvement).
    pub fn stop(&mut self) {
        self.alpha = 0.0;
        self.alpha_target = 0.0;
        self.vel.iter_mut().for_each(|v| *v = Vec3::ZERO);
    }

    /// Un pas de simulation.
    pub fn step(&mut self) {
        let n = self.pos.len();
        if n == 0 {
            return;
        }
        let p = self.params;
        self.alpha += (self.alpha_target - self.alpha) * p.alpha_decay;
        let alpha = self.alpha;

        // 1. Répulsion (Barnes-Hut).
        self.tree.build(&self.pos);
        let theta2 = p.theta * p.theta;
        for i in 0..n {
            if self.pinned[i] {
                continue;
            }
            let f = self.tree.force_on(i, self.pos[i], &self.pos, theta2);
            self.vel[i] += f * (p.charge * alpha);
        }

        // 2. Ressorts.
        for &(a, b) in &self.edges {
            let (a, b) = (a as usize, b as usize);
            let mut d = (self.pos[b] + self.vel[b]) - (self.pos[a] + self.vel[a]);
            if d.length_sq() < 1e-6 {
                d = jitter(a + b, self.flat);
            }
            let len = d.length();
            let strength = 1.0 / self.degree[a].min(self.degree[b]).max(1) as f32;
            let k = (len - p.link_distance) / len * alpha * strength;
            let delta = d * k;
            let bias = self.degree[a] as f32 / (self.degree[a] + self.degree[b]) as f32;
            if !self.pinned[b] {
                self.vel[b] -= delta * bias;
            }
            if !self.pinned[a] {
                self.vel[a] += delta * (1.0 - bias);
            }
        }

        // 3. Gravité, contraintes et intégration.
        for i in 0..n {
            if self.pinned[i] {
                self.vel[i] = Vec3::ZERO;
                continue;
            }
            self.vel[i] -= self.pos[i] * (p.gravity * alpha);
            self.vel[i] *= 1.0 - p.velocity_decay;
            if !self.vel[i].is_finite() {
                self.vel[i] = Vec3::ZERO;
            }
            self.pos[i] += self.vel[i];
            if self.flat {
                self.pos[i].z = 0.0;
                self.vel[i].z = 0.0;
            } else if let Some(z) = self.z_target[i] {
                self.pos[i].z = z;
                self.vel[i].z = 0.0;
            }
        }
    }

    /// Fait tourner la simulation jusqu'à l'arrêt (au plus `max_steps` pas).
    pub fn run(&mut self, max_steps: usize) {
        for _ in 0..max_steps {
            if !self.is_running() {
                break;
            }
            self.step();
        }
    }
}

/// Position initiale déterministe dans le plan (dérivée d'une graine, par
/// exemple un hachage de l'identifiant), dans un disque dont la taille
/// dépend du nombre de nœuds : la disposition est stable d'une ouverture à
/// l'autre.
pub fn initial_position(seed: u64, node_count: usize) -> Vec3 {
    let h1 = splitmix(seed);
    let h2 = splitmix(h1);
    let r = spread(node_count) * unit(h1).sqrt();
    let a = unit(h2) * std::f32::consts::TAU;
    Vec3::new(r * a.cos(), r * a.sin(), 0.0)
}

/// Profondeur initiale déterministe (passage en 3D), dans
/// `[-0.6 s, 0.6 s]` où `s` est le rayon de [`initial_position`].
pub fn initial_depth(seed: u64, node_count: usize) -> f32 {
    let h = splitmix(splitmix(splitmix(seed)));
    (unit(h) * 2.0 - 1.0) * spread(node_count) * 0.6
}

fn spread(node_count: usize) -> f32 {
    10.0 * (node_count.max(1) as f32).sqrt() + 40.0
}

fn unit(h: u64) -> f32 {
    (h >> 11) as f32 / (1u64 << 53) as f32
}

fn splitmix(mut x: u64) -> u64 {
    x = x.wrapping_add(0x9E37_79B9_7F4A_7C15);
    let mut z = x;
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    z ^ (z >> 31)
}

/// Petit décalage déterministe pour séparer des points confondus.
fn jitter(i: usize, flat: bool) -> Vec3 {
    let a = (i as f32 * 2.399_963) % std::f32::consts::TAU;
    let z = if flat {
        0.0
    } else {
        (i as f32 * 0.618_034).fract() - 0.5
    };
    Vec3::new(a.cos(), a.sin(), z) * 1e-3
}

const MAX_DEPTH: u32 = 24;
const NONE: u32 = u32::MAX;

#[derive(Clone, Debug)]
struct Cell {
    min: Vec3,
    size: f32,
    mass: f32,
    com_sum: Vec3,
    children: [u32; 8],
    /// Points d'une feuille (un seul sauf à profondeur maximale).
    points: Vec<u32>,
}

impl Cell {
    fn new(min: Vec3, size: f32) -> Cell {
        Cell {
            min,
            size,
            mass: 0.0,
            com_sum: Vec3::ZERO,
            children: [NONE; 8],
            points: Vec::new(),
        }
    }

    fn is_leaf(&self) -> bool {
        self.children == [NONE; 8]
    }
}

/// Octree pour l'approximation Barnes-Hut (reconstruit à chaque pas).
#[derive(Clone, Debug, Default)]
struct Octree {
    cells: Vec<Cell>,
    stack: Vec<usize>,
}

impl Octree {
    fn build(&mut self, pos: &[Vec3]) {
        self.cells.clear();
        let mut lo = Vec3::new(f32::MAX, f32::MAX, f32::MAX);
        let mut hi = Vec3::new(f32::MIN, f32::MIN, f32::MIN);
        for p in pos {
            lo = lo.min(*p);
            hi = hi.max(*p);
        }
        let size = (hi.x - lo.x).max(hi.y - lo.y).max(hi.z - lo.z).max(1.0) * 1.0001;
        self.cells.push(Cell::new(lo, size));
        for (i, p) in pos.iter().enumerate() {
            self.insert(i as u32, *p, pos);
        }
    }

    fn insert(&mut self, i: u32, p: Vec3, pos: &[Vec3]) {
        let mut c = 0usize;
        let mut depth = 0;
        loop {
            self.cells[c].mass += 1.0;
            self.cells[c].com_sum += p;
            if self.cells[c].is_leaf() {
                if self.cells[c].points.is_empty() || depth >= MAX_DEPTH {
                    self.cells[c].points.push(i);
                    return;
                }
                // Subdivision : le point existant descend d'un niveau.
                let moved = std::mem::take(&mut self.cells[c].points);
                for j in moved {
                    let q = pos[j as usize];
                    let child = self.child_for(c, q);
                    self.cells[child].mass += 1.0;
                    self.cells[child].com_sum += q;
                    self.cells[child].points.push(j);
                }
            }
            c = self.child_for(c, p);
            depth += 1;
        }
    }

    /// Enfant de `c` qui contient `p` (créé au besoin).
    fn child_for(&mut self, c: usize, p: Vec3) -> usize {
        let cell = &self.cells[c];
        let half = cell.size / 2.0;
        let right = p.x >= cell.min.x + half;
        let bottom = p.y >= cell.min.y + half;
        let back = p.z >= cell.min.z + half;
        let q = usize::from(right) + 2 * usize::from(bottom) + 4 * usize::from(back);
        if cell.children[q] != NONE {
            return cell.children[q] as usize;
        }
        let off = |b: bool| if b { half } else { 0.0 };
        let min = cell.min + Vec3::new(off(right), off(bottom), off(back));
        let idx = self.cells.len();
        self.cells.push(Cell::new(min, half));
        self.cells[c].children[q] = idx as u32;
        idx
    }

    /// Somme de (q - p) / |q - p|² sur les autres nœuds (approximée).
    fn force_on(&mut self, i: usize, p: Vec3, pos: &[Vec3], theta2: f32) -> Vec3 {
        let mut f = Vec3::ZERO;
        let mut stack = std::mem::take(&mut self.stack);
        stack.clear();
        stack.push(0);
        while let Some(c) = stack.pop() {
            let cell = &self.cells[c];
            if cell.mass == 0.0 {
                continue;
            }
            if cell.is_leaf() {
                for &j in &cell.points {
                    if j as usize != i {
                        f += pair(p, pos[j as usize], i + j as usize);
                    }
                }
                continue;
            }
            let com = cell.com_sum / cell.mass;
            let d = com - p;
            let dist2 = d.length_sq();
            if cell.size * cell.size < theta2 * dist2 {
                f += d * (cell.mass / dist2.max(1.0));
            } else {
                stack.extend(
                    cell.children
                        .iter()
                        .filter(|&&k| k != NONE)
                        .map(|&k| k as usize),
                );
            }
        }
        self.stack = stack;
        f
    }
}

fn pair(p: Vec3, q: Vec3, seed: usize) -> Vec3 {
    let mut d = q - p;
    if d.length_sq() < 1e-6 {
        d = jitter(seed, p.z == 0.0 && q.z == 0.0);
    }
    d / d.length_sq().max(1.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn exact_force(i: usize, pos: &[Vec3]) -> Vec3 {
        let mut f = Vec3::ZERO;
        for (j, q) in pos.iter().enumerate() {
            if j != i {
                f += pair(pos[i], *q, i + j);
            }
        }
        f
    }

    fn grid(n: usize) -> Vec<Vec3> {
        (0..n).map(|i| initial_position(i as u64, n)).collect()
    }

    fn space(n: usize) -> Vec<Vec3> {
        (0..n)
            .map(|i| {
                let p = initial_position(i as u64, n);
                Vec3::new(p.x, p.y, initial_depth(i as u64, n))
            })
            .collect()
    }

    #[test]
    fn barnes_hut_close_to_exact() {
        for pos in [grid(500), space(500)] {
            let mut tree = Octree::default();
            tree.build(&pos);
            for i in [0, 17, 250, 499] {
                let approx = tree.force_on(i, pos[i], &pos, 0.81);
                let exact = exact_force(i, &pos);
                let err = (approx - exact).length() / exact.length().max(1e-6);
                assert!(err < 0.1, "erreur relative {err} pour {i}");
            }
            // θ = 0 : calcul exact.
            let approx = tree.force_on(3, pos[3], &pos, 0.0);
            assert!((approx - exact_force(3, &pos)).length() < 1e-3);
        }
    }

    #[test]
    fn coincident_points_do_not_break_tree() {
        for flat in [true, false] {
            let pos = vec![Vec3::new(5.0, 5.0, 0.0); 50];
            let mut l = Layout::new(pos, &[], LayoutParams::default());
            l.set_flat(flat, |_| 0.0);
            l.run(400);
            assert!(l.pos.iter().all(|p| p.is_finite()));
            // Ils se sont séparés.
            assert!((l.pos[0] - l.pos[1]).length() > 1.0);
        }
    }

    #[test]
    fn linked_nodes_end_closer_than_unlinked() {
        // Deux groupes reliés en interne, sans lien entre eux.
        let n = 20;
        let mut edges = Vec::new();
        for i in 0..9 {
            edges.push((i, i + 1));
            edges.push((10 + i, 11 + i));
        }
        for flat in [true, false] {
            let mut l = Layout::new(space(n), &edges, LayoutParams::default());
            l.set_flat(flat, |i| initial_depth(i as u64, n));
            l.run(1000);
            assert!(!l.is_running());
            let d = |a: usize, b: usize| (l.pos[a] - l.pos[b]).length();
            let linked = edges.iter().map(|&(a, b)| d(a, b)).sum::<f32>() / edges.len() as f32;
            assert!(linked < 140.0, "longueur moyenne des arêtes {linked}");
            assert!(d(0, 15) > linked);
        }
    }

    #[test]
    fn pinned_nodes_stay_put() {
        let pos = grid(30);
        let fixed = pos[4];
        let edges: Vec<_> = (0..29).map(|i| (i, i + 1)).collect();
        let mut l = Layout::new(pos, &edges, LayoutParams::default());
        l.pinned[4] = true;
        l.run(300);
        assert_eq!(l.pos[4], fixed);
    }

    #[test]
    fn cooling_reheat_and_target() {
        let mut l = Layout::new(grid(10), &[], LayoutParams::default());
        l.run(10_000);
        assert!(!l.is_running());
        l.reheat(0.3);
        assert!(l.is_running());
        l.stop();
        l.set_alpha_target(0.3);
        l.run(50);
        assert!(l.is_running(), "reste actif tant qu'une cible est fixée");
        assert!(l.alpha() > 0.1);
    }

    #[test]
    fn duplicate_edges_and_loops_ignored() {
        let l = Layout::new(
            grid(3),
            &[(0, 1), (1, 0), (2, 2), (1, 9)],
            LayoutParams::default(),
        );
        assert_eq!(l.degree(0), 1);
        assert_eq!(l.degree(1), 1);
        assert_eq!(l.degree(2), 0);
    }

    #[test]
    fn initial_positions_deterministic_and_spread() {
        assert_eq!(initial_position(42, 100), initial_position(42, 100));
        assert_ne!(initial_position(1, 100), initial_position(2, 100));
        let r = 10.0 * 10.0 + 40.0;
        assert!((0..1000).all(|s| initial_position(s, 100).length() <= r + 1e-3));
        assert!((0..1000).all(|s| initial_position(s, 100).z == 0.0));
        assert_eq!(initial_depth(5, 100), initial_depth(5, 100));
        assert!((0..1000).all(|s| initial_depth(s, 100).abs() <= r));
    }

    #[test]
    fn flat_mode_keeps_plane_and_3d_uses_depth() {
        let n = 40;
        let edges: Vec<_> = (0..n - 1).map(|i| (i, i + 1)).collect();
        let mut l = Layout::new(space(n), &edges, LayoutParams::default());
        assert!(l.is_flat());
        assert!(l.pos.iter().all(|p| p.z == 0.0), "z aplati à la création");
        l.run(100);
        assert!(l.pos.iter().all(|p| p.z == 0.0));

        l.set_flat(false, |i| initial_depth(i as u64, n));
        assert!(l.is_running());
        l.run(500);
        assert!(
            l.pos.iter().any(|p| p.z.abs() > 5.0),
            "la 3D utilise la profondeur"
        );
        assert!(l.pos.iter().all(|p| p.is_finite()));

        l.set_flat(true, |_| 0.0);
        assert!(l.pos.iter().all(|p| p.z == 0.0));

        let l3 = Layout::new_3d(space(n), &edges, LayoutParams::default());
        assert!(!l3.is_flat());
        assert_eq!(l3.degree(1), 2);
        assert!(l3.pos.iter().any(|p| p.z != 0.0));
    }

    #[test]
    fn z_target_is_enforced_in_3d_only() {
        let n = 10;
        let mut l = Layout::new(space(n), &[(0, 1), (1, 2)], LayoutParams::default());
        l.z_target[0] = Some(120.0);
        l.z_target[1] = Some(-80.0);
        l.run(50);
        assert_eq!(l.pos[0].z, 0.0, "ignoré en mode plan");
        l.set_flat(false, |_| 3.0);
        assert_eq!(l.pos[0].z, 120.0);
        assert_eq!(l.pos[2].z, 3.0);
        l.run(300);
        assert_eq!(l.pos[0].z, 120.0);
        assert_eq!(l.pos[1].z, -80.0);
    }

    /// 2000 nœuds / 5000 arêtes : un pas doit rester bien sous 16 ms en
    /// release (borne large en debug), en 2D comme en 3D.
    #[test]
    fn step_time_2000_nodes() {
        let n = 2000;
        let edges: Vec<_> = (0..5000).map(|k| (k % n, (k * 7 + 13) % n)).collect();
        for flat in [true, false] {
            let mut l = Layout::new(space(n), &edges, LayoutParams::default());
            l.set_flat(flat, |i| initial_depth(i as u64, n));
            l.step();
            let t = std::time::Instant::now();
            for _ in 0..10 {
                l.step();
            }
            let per_step = t.elapsed() / 10;
            let mode = if flat { "2D" } else { "3D" };
            eprintln!("pas de simulation {mode} (2000 nœuds, 5000 arêtes) : {per_step:?}");
            let budget = if cfg!(debug_assertions) { 400 } else { 16 };
            assert!(per_step < std::time::Duration::from_millis(budget));
        }
    }
}
