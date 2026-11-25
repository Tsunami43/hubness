//! A tab that is not built yet (replaced by its real screen).

use crossterm::event::KeyEvent;
use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::widgets::Paragraph;

use crate::core::{Ctx, Handled, Screen};
use crate::ui::block;

pub struct Todo(pub &'static str);

impl Screen for Todo {
    fn title(&self) -> String {
        self.0.into()
    }
    fn draw(&mut self, f: &mut Frame, area: Rect, _ctx: &Ctx) {
        f.render_widget(Paragraph::new("Not built yet in the TUI — use the web app for now.").block(block(self.0, false)), area);
    }
    fn on_key(&mut self, _key: KeyEvent, _ctx: &mut Ctx) -> Handled {
        Handled::No
    }
}
