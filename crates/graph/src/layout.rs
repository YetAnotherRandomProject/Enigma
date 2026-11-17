//! Disposition en forces 2D, dans l'esprit de d3-force :
//! - répulsion entre tous les nœuds (approximation Barnes-Hut, O(n log n)) ;
//! - ressorts le long des arêtes (longueur cible, force ∝ 1/degré) ;
//! - légère gravité vers l'origine ;
//! - amortissement des vitesses et « température » `alpha` qui décroît
//!   jusqu'à l'arrêt et qu'on relance lors des interactions.
//!
//! Les nœuds épinglés ne bougent pas mais exercent leurs forces.

use crate::vec2::Vec2;

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
    pub pos: Vec<Vec2>,
    vel: Vec<Vec2>,
    pub pinned: Vec<bool>,
    edges: Vec<(u32, u32)>,
    degree: Vec<u32>,
    alpha: f32,
    alpha_target: f32,
    params: LayoutParams,
    tree: QuadTree,
}

impl Layout {
    /// `edges` : paires d'indices ; boucles et doublons sont ignorés.
    pub fn new(positions: Vec<Vec2>, edges: &[(usize, usize)], params: LayoutParams) -> Layout {
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
        Layout {
            vel: vec![Vec2::ZERO; n],
            pinned: vec![false; n],
            pos: positions,
            edges: uniq,
            degree,
            alpha: 1.0,
            alpha_target: 0.0,
            params,
            tree: QuadTree::default(),
        }
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
        self.vel.iter_mut().for_each(|v| *v = Vec2::ZERO);
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
                d = jitter(a + b);
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

        // 3. Gravité et intégration.
        for i in 0..n {
            if self.pinned[i] {
                self.vel[i] = Vec2::ZERO;
                continue;
            }
            self.vel[i] -= self.pos[i] * (p.gravity * alpha);
            self.vel[i] *= 1.0 - p.velocity_decay;
            if !self.vel[i].is_finite() {
                self.vel[i] = Vec2::ZERO;
            }
            self.pos[i] += self.vel[i];
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

/// Position initiale déterministe (dérivée d'une graine, par exemple un
/// hachage de l'identifiant), dans un disque dont la taille dépend du nombre
/// de nœuds : la disposition est stable d'une ouverture à l'autre.
pub fn initial_position(seed: u64, node_count: usize) -> Vec2 {
    let h1 = splitmix(seed);
    let h2 = splitmix(h1);
    let u = (h1 >> 11) as f32 / (1u64 << 53) as f32;
    let v = (h2 >> 11) as f32 / (1u64 << 53) as f32;
    let radius = 10.0 * (node_count.max(1) as f32).sqrt() + 40.0;
    let r = radius * u.sqrt();
    let a = v * std::f32::consts::TAU;
    Vec2::new(r * a.cos(), r * a.sin())
}

fn splitmix(mut x: u64) -> u64 {
    x = x.wrapping_add(0x9E37_79B9_7F4A_7C15);
    let mut z = x;
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    z ^ (z >> 31)
}

/// Petit décalage déterministe pour séparer des points confondus.
fn jitter(i: usize) -> Vec2 {
    let a = (i as f32 * 2.399_963) % std::f32::consts::TAU;
    Vec2::new(a.cos(), a.sin()) * 1e-3
}

const MAX_DEPTH: u32 = 24;
const NONE: u32 = u32::MAX;

#[derive(Clone, Debug)]
struct Cell {
    min: Vec2,
    size: f32,
    mass: f32,
    com_sum: Vec2,
    children: [u32; 4],
    /// Points d'une feuille (un seul sauf à profondeur maximale).
    points: Vec<u32>,
}

impl Cell {
    fn new(min: Vec2, size: f32) -> Cell {
        Cell {
            min,
            size,
            mass: 0.0,
            com_sum: Vec2::ZERO,
            children: [NONE; 4],
            points: Vec::new(),
        }
    }

    fn is_leaf(&self) -> bool {
        self.children == [NONE; 4]
    }
}

/// Quadtree pour l'approximation Barnes-Hut (reconstruit à chaque pas).
#[derive(Clone, Debug, Default)]
struct QuadTree {
    cells: Vec<Cell>,
}

impl QuadTree {
    fn build(&mut self, pos: &[Vec2]) {
        self.cells.clear();
        let (mut lo, mut hi) = (Vec2::new(f32::MAX, f32::MAX), Vec2::new(f32::MIN, f32::MIN));
        for p in pos {
            lo = Vec2::new(lo.x.min(p.x), lo.y.min(p.y));
            hi = Vec2::new(hi.x.max(p.x), hi.y.max(p.y));
        }
        let size = (hi.x - lo.x).max(hi.y - lo.y).max(1.0) * 1.0001;
        self.cells.push(Cell::new(lo, size));
        for (i, p) in pos.iter().enumerate() {
            self.insert(i as u32, *p, pos);
        }
    }

    fn insert(&mut self, i: u32, p: Vec2, pos: &[Vec2]) {
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
    fn child_for(&mut self, c: usize, p: Vec2) -> usize {
        let cell = &self.cells[c];
        let half = cell.size / 2.0;
        let right = p.x >= cell.min.x + half;
        let bottom = p.y >= cell.min.y + half;
        let q = usize::from(right) + 2 * usize::from(bottom);
        if cell.children[q] != NONE {
            return cell.children[q] as usize;
        }
        let min = Vec2::new(
            cell.min.x + if right { half } else { 0.0 },
            cell.min.y + if bottom { half } else { 0.0 },
        );
        let idx = self.cells.len();
        self.cells.push(Cell::new(min, half));
        self.cells[c].children[q] = idx as u32;
        idx
    }

    /// Somme de (q - p) / |q - p|² sur les autres nœuds (approximée).
    fn force_on(&self, i: usize, p: Vec2, pos: &[Vec2], theta2: f32) -> Vec2 {
        let mut f = Vec2::ZERO;
        let mut stack = vec![0usize];
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
        f
    }
}

fn pair(p: Vec2, q: Vec2, seed: usize) -> Vec2 {
    let mut d = q - p;
    if d.length_sq() < 1e-6 {
        d = jitter(seed);
    }
    d / d.length_sq().max(1.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn exact_force(i: usize, pos: &[Vec2]) -> Vec2 {
        let mut f = Vec2::ZERO;
        for (j, q) in pos.iter().enumerate() {
            if j != i {
                f += pair(pos[i], *q, i + j);
            }
        }
        f
    }

    fn grid(n: usize) -> Vec<Vec2> {
        (0..n).map(|i| initial_position(i as u64, n)).collect()
    }

    #[test]
    fn barnes_hut_close_to_exact() {
        let pos = grid(500);
        let mut tree = QuadTree::default();
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

    #[test]
    fn coincident_points_do_not_break_tree() {
        let pos = vec![Vec2::new(5.0, 5.0); 50];
        let mut l = Layout::new(pos, &[], LayoutParams::default());
        l.run(400);
        assert!(l.pos.iter().all(|p| p.is_finite()));
        // Ils se sont séparés.
        assert!((l.pos[0] - l.pos[1]).length() > 1.0);
    }

    #[test]
    fn linked_nodes_end_closer_than_unlinked() {
        // Deux groupes reliés en interne, sans lien entre eux.
        let n = 20;
        let pos = grid(n);
        let mut edges = Vec::new();
        for i in 0..9 {
            edges.push((i, i + 1));
            edges.push((10 + i, 11 + i));
        }
        let mut l = Layout::new(pos, &edges, LayoutParams::default());
        l.run(1000);
        assert!(!l.is_running());
        let d = |a: usize, b: usize| (l.pos[a] - l.pos[b]).length();
        let linked = edges.iter().map(|&(a, b)| d(a, b)).sum::<f32>() / edges.len() as f32;
        assert!(linked < 140.0, "longueur moyenne des arêtes {linked}");
        assert!(d(0, 15) > linked);
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
    }

    /// 2000 nœuds / 5000 arêtes : un pas doit rester bien sous 16 ms en
    /// release (borne large en debug).
    #[test]
    fn step_time_2000_nodes() {
        let n = 2000;
        let pos = grid(n);
        let edges: Vec<_> = (0..5000).map(|k| (k % n, (k * 7 + 13) % n)).collect();
        let mut l = Layout::new(pos, &edges, LayoutParams::default());
        l.step();
        let t = std::time::Instant::now();
        for _ in 0..10 {
            l.step();
        }
        let per_step = t.elapsed() / 10;
        eprintln!("pas de simulation (2000 nœuds, 5000 arêtes) : {per_step:?}");
        let budget = if cfg!(debug_assertions) { 400 } else { 16 };
        assert!(per_step < std::time::Duration::from_millis(budget));
    }
}
