use std::io;
use std::sync::atomic::AtomicBool;
#[cfg(unix)]
use std::sync::atomic::Ordering;

use crossterm::{
    event::{DisableMouseCapture, EnableMouseCapture},
    execute,
    terminal::{EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode},
};
use ratatui::{Terminal, backend::CrosstermBackend};
use tmux_agent_sidebar::{app, tmux};

static NEEDS_REFRESH: AtomicBool = AtomicBool::new(false);

struct TuiSession {
    entered_alt_screen: bool,
}

impl TuiSession {
    fn enter(stdout: &mut io::Stdout) -> io::Result<Self> {
        enable_raw_mode()?;
        if let Err(err) = execute!(stdout, EnterAlternateScreen, EnableMouseCapture) {
            let _ = disable_raw_mode();
            return Err(err);
        }
        Ok(Self {
            entered_alt_screen: true,
        })
    }
}

impl Drop for TuiSession {
    fn drop(&mut self) {
        let _ = disable_raw_mode();
        if self.entered_alt_screen {
            let mut stdout = io::stdout();
            let _ = execute!(stdout, LeaveAlternateScreen, DisableMouseCapture);
        }
    }
}

fn main() -> io::Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if let Some(code) = tmux_agent_sidebar::cli::run(&args) {
        std::process::exit(code);
    }

    let tmux_pane = tmux::current_pane_target();
    if tmux_pane.is_empty() {
        eprintln!("TMUX_PANE not set");
        std::process::exit(1);
    }

    #[cfg(unix)]
    unsafe {
        let mut sa: libc::sigaction = std::mem::zeroed();
        sa.sa_sigaction = sigusr1_handler as *const () as libc::sighandler_t;
        sa.sa_flags = libc::SA_RESTART;
        libc::sigaction(libc::SIGUSR1, &sa, std::ptr::null_mut());
    }

    let pid = std::process::id();
    #[cfg(windows)]
    tmux::unset_pane_option(&tmux_pane, tmux::SIDEBAR_CLOSE_REQUEST);
    tmux::set_pane_option(&tmux_pane, tmux::SIDEBAR_PID, &pid.to_string());
    tmux::set_pane_option(&tmux_pane, tmux::PANE_ROLE, "sidebar");

    let mut stdout = io::stdout();
    let _tui_session = TuiSession::enter(&mut stdout)?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;

    app::run(&mut terminal, tmux_pane, &NEEDS_REFRESH)
}

#[cfg(unix)]
extern "C" fn sigusr1_handler(_: libc::c_int) {
    NEEDS_REFRESH.store(true, Ordering::Relaxed);
}
