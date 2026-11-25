//! Screens of the TUI, one per web app route. `make` builds the screen for a route.

pub mod composer;
pub mod feed;
pub mod group;
pub mod harness;
pub mod harness_ext;
pub mod harness_mcp;
pub mod harness_skills;
pub mod home;
pub mod kit;
pub mod project;
pub mod projects;
pub mod session;
pub mod settings;
pub mod system;
pub mod tabs;

use crate::core::{Route, Screen};

pub fn make(route: &Route) -> Box<dyn Screen> {
    match route {
        Route::Home => Box::new(home::Home::default()),
        Route::Chat(id) => Box::new(session::SessionView::new(id)),
        Route::ProjectSession { session, .. } | Route::GroupSession { session, .. } => Box::new(session::SessionView::new(session)),
        Route::Projects => Box::new(projects::Projects::default()),
        Route::Project(id) => Box::new(project::ProjectScreen::new(id)),
        Route::Group(id) => Box::new(group::GroupScreen::new(id)),
        Route::System(section) => system::screen(section.as_deref()),
        Route::Harness(kind) => Box::new(harness::HarnessScreen::new(kind.as_deref())),
        Route::Settings(section) => settings::screen(section.as_deref()),
    }
}
