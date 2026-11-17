//! `enigma-graph` : disposition en forces, caméra, projection et sélection
//! pour le graphe des fiches. Aucune dépendance à l'interface.
//!
//! - [`layout`] : simulation de forces 2D (Barnes-Hut).
//! - [`camera`] : caméra 2D (déplacement, zoom, cadrage).
//! - [`pick`] : sélection à la souris et voisinage d'un nœud.

pub mod camera;
pub mod layout;
pub mod pick;
pub mod vec2;

pub use camera::Camera2D;
pub use layout::{Layout, LayoutParams, initial_position};
pub use vec2::Vec2;
