//! The Sessions tab of a group: the project Sessions tab scoped to the group (a group
//! session sees every repository of the group and opens as `Route::GroupSession`).

use crate::screens::project::sessions::{Scope, SessionsTab};

pub fn tab(id: &str) -> SessionsTab {
    SessionsTab::new(Scope::Group(id.to_string()))
}
