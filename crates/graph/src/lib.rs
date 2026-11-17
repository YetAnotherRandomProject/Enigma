//! `enigma-graph` : disposition en forces, caméras, projection et sélection
//! pour le graphe des fiches. Aucune dépendance à l'interface.
//!
//! - [`layout`] : simulation de forces 2D/3D (Barnes-Hut, octree).
//! - [`camera`] : caméra 2D (déplacement, zoom, cadrage).
//! - [`camera3d`] : caméra orbitale en perspective.
//! - [`pick`] : sélection à la souris et voisinage d'un nœud.

pub mod camera;
pub mod camera3d;
pub mod layout;
pub mod pick;
pub mod vec2;
pub mod vec3;

pub use camera::Camera2D;
pub use camera3d::{OrbitCamera, Projected, Projector};
pub use layout::{Layout, LayoutParams, initial_depth, initial_position};
pub use vec2::Vec2;
pub use vec3::Vec3;
