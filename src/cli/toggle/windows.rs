use crate::tmux;

use super::SidebarPosition;

pub(super) fn create_sidebar(
    window: &str,
    width: &str,
    position: SidebarPosition,
    geometry: &str,
    directory: &str,
    binary: &str,
) -> Result<String, String> {
    let original = tmux::display_message(window, "#{window_layout}");
    let width = width
        .parse::<u32>()
        .map_err(|error| format!("invalid sidebar width: {error}"))?;
    let layout = sidebar_layout(&original, width, position)?;
    // psmux consumes layout leaves in tree order and ignores pane ids.
    let original_panes: Vec<&str> = geometry
        .lines()
        .filter_map(|line| line.split_whitespace().last())
        .collect();
    let target = original_panes
        .last()
        .ok_or("cannot find the last psmux pane")?;
    let pane = tmux::run_tmux_capture(&[
        "split-window",
        "-h",
        "-d",
        "-p",
        "20",
        "-t",
        target,
        "-c",
        directory,
        "-P",
        "-F",
        "#{pane_id}",
        binary,
    ])?;
    if pane.is_empty() || original_panes.contains(&pane.as_str()) {
        return Err("psmux did not create a new sidebar pane".into());
    }
    tmux::set_pane_option(&pane, tmux::PANE_ROLE, "sidebar");
    if let Err(error) = place_sidebar(window, &layout, position) {
        // remove only the new sidebar before restoring the original tree.
        super::close_sidebar(&pane)
            .map_err(|cleanup| format!("{error}; cannot close new sidebar {pane}: {cleanup}"))?;
        tmux::run_tmux_capture(&["select-layout", "-t", window, &original])
            .map_err(|restore| format!("{error}; cannot restore original layout: {restore}"))?;
        return Err(error);
    }
    Ok(pane)
}

fn place_sidebar(window: &str, layout: &str, position: SidebarPosition) -> Result<(), String> {
    // flatten before rotating so only the new leaf moves ahead of the old tree.
    tmux::run_tmux_capture(&["select-layout", "-t", window, "even-horizontal"])?;
    if position == SidebarPosition::Left {
        tmux::run_tmux_capture(&["rotate-window", "-t", window])?;
    }
    tmux::run_tmux_capture(&["select-layout", "-t", window, layout])?;
    Ok(())
}

fn sidebar_layout(
    original: &str,
    sidebar_width: u32,
    position: SidebarPosition,
) -> Result<String, String> {
    let (_, body) = original
        .split_once(',')
        .ok_or("psmux did not return a window layout")?;
    let (dimensions, tree) = body.split_once(',').ok_or("missing layout dimensions")?;
    let (width, height) = dimensions
        .split_once('x')
        .ok_or("invalid layout dimensions")?;
    let width = width
        .parse::<u32>()
        .map_err(|error| format!("invalid window width: {error}"))?;
    let height = height
        .parse::<u32>()
        .map_err(|error| format!("invalid window height: {error}"))?;
    if sidebar_width == 0
        || width
            .checked_sub(sidebar_width)
            .is_none_or(|remaining| remaining <= 2)
    {
        return Err(format!(
            "sidebar width {sidebar_width} does not fit window width {width}"
        ));
    }
    let main_width = width - sidebar_width - 1;
    // psmux restores nested split proportions from dimensions, ignoring offsets.
    let main = format!("{main_width}x{height},{tree}");
    let sidebar = format!("{sidebar_width}x{height},0,0,0");
    let children = match position {
        SidebarPosition::Left => format!("{sidebar},{main}"),
        SidebarPosition::Right => format!("{main},{sidebar}"),
    };
    let body = format!("{width}x{height},0,0{{{children}}}");
    let checksum = body.bytes().fold(0_u16, |sum, byte| {
        sum.rotate_right(1).wrapping_add(u16::from(byte))
    });
    Ok(format!("{checksum:04x},{body}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn left_sidebar_keeps_nested_main_tree() {
        let layout = sidebar_layout(
            "0000,187x40,0,0[187x20,0,0,1,187x19,0,21{93x19,0,21,3,93x19,94,21,4}]",
            42,
            SidebarPosition::Left,
        )
        .expect("valid layout");
        assert_eq!(
            layout.split_once(',').expect("checksum").1,
            "187x40,0,0{42x40,0,0,0,144x40,0,0[187x20,0,0,1,187x19,0,21{93x19,0,21,3,93x19,94,21,4}]}"
        );
    }

    #[test]
    fn right_sidebar_follows_main_tree() {
        let layout =
            sidebar_layout("0000,120x30,0,0,1", 30, SidebarPosition::Right).expect("valid layout");
        assert_eq!(
            layout.split_once(',').expect("checksum").1,
            "120x30,0,0{89x30,0,0,1,30x30,0,0,0}"
        );
    }

    #[test]
    fn invalid_layouts_fail_before_creating_a_pane() {
        for original in ["", "0000,invalid", "0000,20x40,0,0,1"] {
            assert!(sidebar_layout(original, 42, SidebarPosition::Left).is_err());
        }
    }
}
