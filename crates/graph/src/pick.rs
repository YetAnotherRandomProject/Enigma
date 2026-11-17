//! Sélection à la souris (en coordonnées du monde) et voisinage d'un nœud.

use std::collections::VecDeque;

use crate::vec2::Vec2;

/// Nœud le plus proche de `p` dont le disque (rayon `radius(i)`, plus une
/// tolérance) contient `p`. Les nœuds non `visible` sont ignorés.
pub fn pick_node(
    positions: &[Vec2],
    p: Vec2,
    tolerance: f32,
    radius: impl Fn(usize) -> f32,
    visible: impl Fn(usize) -> bool,
) -> Option<usize> {
    positions
        .iter()
        .enumerate()
        .filter(|(i, _)| visible(*i))
        .filter_map(|(i, q)| {
            let d = (*q - p).length();
            (d <= radius(i) + tolerance).then_some((i, d))
        })
        .min_by(|a, b| a.1.total_cmp(&b.1))
        .map(|(i, _)| i)
}

/// Distance de `p` au segment `[a, b]`.
pub fn distance_to_segment(p: Vec2, a: Vec2, b: Vec2) -> f32 {
    let ab = b - a;
    let len2 = ab.length_sq();
    if len2 <= f32::EPSILON {
        return (p - a).length();
    }
    let t = ((p - a).dot(ab) / len2).clamp(0.0, 1.0);
    (p - (a + ab * t)).length()
}

/// Segment le plus proche de `p` à moins de `tolerance`.
pub fn pick_segment(
    segments: impl IntoIterator<Item = (usize, Vec2, Vec2)>,
    p: Vec2,
    tolerance: f32,
) -> Option<usize> {
    segments
        .into_iter()
        .map(|(k, a, b)| (k, distance_to_segment(p, a, b)))
        .filter(|(_, d)| *d <= tolerance)
        .min_by(|a, b| a.1.total_cmp(&b.1))
        .map(|(k, _)| k)
}

/// Distance (en nombre d'arêtes, sans tenir compte du sens) de chaque nœud
/// à `start`, jusqu'à `max_depth` ; `None` au-delà ou si non relié.
pub fn neighborhood(
    node_count: usize,
    edges: &[(usize, usize)],
    start: usize,
    max_depth: u32,
) -> Vec<Option<u32>> {
    let mut adjacency = vec![Vec::new(); node_count];
    for &(a, b) in edges {
        if a < node_count && b < node_count {
            adjacency[a].push(b);
            adjacency[b].push(a);
        }
    }
    let mut depth = vec![None; node_count];
    if start >= node_count {
        return depth;
    }
    depth[start] = Some(0);
    let mut queue = VecDeque::from([start]);
    while let Some(i) = queue.pop_front() {
        let d = depth[i].unwrap_or(0);
        if d >= max_depth {
            continue;
        }
        for &j in &adjacency[i] {
            if depth[j].is_none() {
                depth[j] = Some(d + 1);
                queue.push_back(j);
            }
        }
    }
    depth
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn picks_closest_visible_node() {
        let pos = [
            Vec2::new(0.0, 0.0),
            Vec2::new(10.0, 0.0),
            Vec2::new(4.0, 0.0),
        ];
        let r = |_| 5.0;
        assert_eq!(
            pick_node(&pos, Vec2::new(5.0, 0.0), 0.0, r, |_| true),
            Some(2)
        );
        // Nœud 2 masqué : égalité entre 0 et 1, le premier l'emporte.
        assert_eq!(
            pick_node(&pos, Vec2::new(5.0, 0.0), 0.0, r, |i| i != 2),
            Some(0)
        );
        assert_eq!(
            pick_node(&pos, Vec2::new(50.0, 0.0), 2.0, r, |_| true),
            None
        );
    }

    #[test]
    fn segment_distance_and_pick() {
        let a = Vec2::new(0.0, 0.0);
        let b = Vec2::new(10.0, 0.0);
        assert_eq!(distance_to_segment(Vec2::new(5.0, 3.0), a, b), 3.0);
        assert_eq!(distance_to_segment(Vec2::new(-4.0, 3.0), a, b), 5.0);
        assert_eq!(distance_to_segment(Vec2::new(1.0, 1.0), a, a), 2f32.sqrt());
        let segs = [(7, a, b), (8, Vec2::new(0.0, 2.0), Vec2::new(10.0, 2.0))];
        assert_eq!(pick_segment(segs, Vec2::new(5.0, 1.5), 1.0), Some(8));
        assert_eq!(pick_segment(segs, Vec2::new(5.0, 9.0), 1.0), None);
    }

    #[test]
    fn neighborhood_depths() {
        // 0 - 1 - 2 - 3, 4 isolé
        let edges = [(0, 1), (2, 1), (2, 3)];
        let d = neighborhood(5, &edges, 1, 1);
        assert_eq!(d, [Some(1), Some(0), Some(1), None, None]);
        let d = neighborhood(5, &edges, 0, 5);
        assert_eq!(d, [Some(0), Some(1), Some(2), Some(3), None]);
        assert!(neighborhood(5, &edges, 9, 1).iter().all(Option::is_none));
    }
}
