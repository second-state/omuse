//! The web shell: the UI's static files served from memory (`app/sentinel/assets.py` → [`assets`]), the Tera
//! templates for the page shell (`web/index.html`) and the login page ([`templates`]), and the login gate
//! (`deploy/docker/gate.py` → [`gate`]) as Axum middleware.

pub mod assets;
pub mod gate;
pub mod templates;

pub use assets::Assets;
pub use gate::Gate;
pub use templates::Templates;
