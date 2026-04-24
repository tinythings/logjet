//! Interactive `ljx view` command UI.

use std::io::{self, IsTerminal};

use crossterm::execute;
use crossterm::terminal::{EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode};
use ratatui::Terminal;
use ratatui::backend::CrosstermBackend;

mod app;
mod detail;
mod render;
mod scan;
mod text;
mod types;
mod ui;

use crate::cli::ViewArgs;
use crate::error::{Error, Result};

#[cfg(test)]
pub(crate) use self::detail::{extract_otlp_log_message, format_summary, parse_export_selection, render_modal_info_entries, render_modal_message};
#[cfg(test)]
pub(crate) use self::scan::{create_temp_path, open_temp_spool_pair, read_spool_record, write_export_selection_to_temp_logjet, write_spool_record};
#[cfg(test)]
pub(crate) use self::text::text_preview;
#[cfg(test)]
pub(crate) use self::types::{
    DedupUpdate, DetailRecord, EntryMeta, ExportField, ExportFormatChoice, Focus, MODAL_ATTR_ENTRY_LIMIT_PER_KIND, ViewApp,
};

pub fn run(args: ViewArgs) -> Result<()> {
    if !io::stdin().is_terminal() || !io::stdout().is_terminal() {
        return Err(Error::Usage("ljx view needs an interactive terminal; pipe-oriented output belongs in `ljx filter`".to_string()));
    }

    let mut stdout = io::stdout();
    enable_raw_mode()?;
    execute!(stdout, EnterAlternateScreen)?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;
    let mut app = self::types::ViewApp::new(args)?;
    app.apply_filter()?;
    let outcome = app.run(&mut terminal);

    disable_raw_mode()?;
    execute!(terminal.backend_mut(), LeaveAlternateScreen)?;
    terminal.show_cursor()?;

    outcome
}

#[cfg(test)]
#[path = "../../../tests/unit/commands/view_ut.rs"]
mod view_ut;
