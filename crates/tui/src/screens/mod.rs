//! Screens of the TUI, one per web app route. `make` builds the screen for a route.

pub mod composer;
pub mod feed;
pub mod group;
pub mod home;
pub mod project;
pub mod projects;
pub mod session;
pub mod tabs;

use crossterm::event::KeyEvent;
use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::widgets::Paragraph;

use crate::core::{Ctx, Handled, Route, Screen};
use crate::ui::block;

pub fn make(route: &Route) -> Box<dyn Screen> {
    match route {
        Route::Home => Box::new(home::Home::default()),
        Route::Chat(id) => Box::new(session::SessionView::new(id)),
        Route::ProjectSession { session, .. } | Route::GroupSession { session, .. } => Box::new(session::SessionView::new(session)),
        Route::Projects => Box::new(projects::Projects::default()),
        Route::Project(id) => Box::new(project::ProjectScreen::new(id)),
        Route::Group(id) => Box::new(group::GroupScreen::new(id)),
        other => Box::new(Placeholder(format!("{other:?}"))),
    }
}

/// A screen that is not built yet.
struct Placeholder(String);

impl Screen for Placeholder {
    fn title(&self) -> String {
        self.0.clone()
    }
    fn draw(&mut self, f: &mut Frame, area: Rect, _ctx: &Ctx) {
        f.render_widget(Paragraph::new("Coming soon in the TUI — use the web app for now.").block(block(self.0.clone(), true)), area);
    }
    fn on_key(&mut self, _key: KeyEvent, _ctx: &mut Ctx) -> Handled {
        Handled::No
    }
}
