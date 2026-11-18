use crate::tmux;

use super::{SidebarPosition, configured_width};

pub(crate) fn maintain_sidebar_width(pane: &str) -> Result<(), String> {
    let geometry = tmux::run_tmux_capture(&[
        "display-message",
        "-t",
        pane,
        "-p",
        "#{window_id}|#{window_layout}|#{pane_width}|#{window_width}|#{window_zoomed_flag}|#{@sidebar_width}|#{pane_left}",
    ])?;
    let fields: Vec<&str> = geometry.split('|').collect();
    let [
        window,
        layout,
        pane_width,
        window_width,
        zoomed,
        setting,
        left,
    ] = fields.as_slice()
    else {
        return Err("invalid sidebar geometry".into());
    };
    if *zoomed == "1" {
        return Ok(());
    }
    let current = pane_width
        .parse::<u32>()
        .map_err(|error| format!("invalid pane width: {error}"))?;
    let total = window_width
        .parse::<u32>()
        .map_err(|error| format!("invalid window width: {error}"))?;
    let target = configured_width(setting, total)?;
    // psmux stores split sizes as integer percentages, losing up to one percent.
    if current.abs_diff(target) <= total.div_ceil(100) || current == total {
        return Ok(());
    }
    let position = if *left == "0" {
        SidebarPosition::Left
    } else {
        SidebarPosition::Right
    };
    let restored = restore_sidebar_width(layout, target, position, pane)?;
    tmux::run_tmux_capture(&["select-layout", "-t", window, &restored]).map(|_| ())
}

fn restore_sidebar_width(
    layout: &str,
    width: u32,
    position: SidebarPosition,
    pane: &str,
) -> Result<String, String> {
    let (_, body) = layout.split_once(',').ok_or("missing layout checksum")?;
    let (root, children) = body
        .split_once('{')
        .ok_or("sidebar has no outer horizontal split")?;
    if root.contains('[') {
        return Err("sidebar is nested inside a vertical split".into());
    }
    let children = children.strip_suffix('}').ok_or("unclosed sidebar split")?;
    let first_end = layout_cell_length(children)?;
    let (first, rest) = children.split_at(first_end);
    let second = rest
        .strip_prefix(',')
        .ok_or("sidebar split has no second child")?;
    if layout_cell_length(second)? != second.len() {
        return Err("sidebar split has more than two children".into());
    }
    let (dimensions, _) = root.split_once(',').ok_or("missing window dimensions")?;
    let (total, _) = dimensions
        .split_once('x')
        .ok_or("invalid window dimensions")?;
    let total = total
        .parse::<u32>()
        .map_err(|error| format!("invalid layout width: {error}"))?;
    let main_width = total
        .checked_sub(width)
        .and_then(|remaining| remaining.checked_sub(1))
        .filter(|remaining| *remaining >= 2)
        .ok_or("sidebar does not fit layout")?;
    let (sidebar, main) = match position {
        SidebarPosition::Left => (first, second),
        SidebarPosition::Right => (second, first),
    };
    if sidebar.contains(['{', '[']) {
        return Err("sidebar is not an outer leaf".into());
    }
    let (_, sidebar_id) = sidebar.rsplit_once(',').ok_or("missing sidebar pane id")?;
    let (_, pane_id) = pane.rsplit_once('%').ok_or("invalid sidebar pane target")?;
    if sidebar_id != pane_id {
        return Err("outer layout leaf does not match sidebar pane".into());
    }
    let sidebar = resize_cell_width(sidebar, width)?;
    let main = resize_cell_width(main, main_width)?;
    let children = match position {
        SidebarPosition::Left => format!("{sidebar},{main}"),
        SidebarPosition::Right => format!("{main},{sidebar}"),
    };
    Ok(checksummed_layout(&format!("{root}{{{children}}}")))
}

fn resize_cell_width(cell: &str, width: u32) -> Result<String, String> {
    let (dimensions, rest) = cell.split_once(',').ok_or("missing cell dimensions")?;
    let (_, height) = dimensions
        .split_once('x')
        .ok_or("invalid cell dimensions")?;
    Ok(format!("{width}x{height},{rest}"))
}

fn layout_cell_length(cell: &str) -> Result<usize, String> {
    let mut depth = 0_u32;
    let mut commas = 0;
    for (index, character) in cell.char_indices() {
        match character {
            '{' | '[' => depth += 1,
            '}' | ']' => {
                depth = depth.checked_sub(1).ok_or("unexpected layout delimiter")?;
                if depth == 0 {
                    return Ok(index + 1);
                }
            }
            ',' if depth == 0 => {
                commas += 1;
                if commas == 4 {
                    return Ok(index);
                }
            }
            _ => {}
        }
    }
    if depth == 0 && commas == 3 {
        Ok(cell.len())
    } else {
        Err("incomplete layout cell".into())
    }
}

fn checksummed_layout(body: &str) -> String {
    let checksum = body.bytes().fold(0_u16, |sum, byte| {
        sum.rotate_right(1).wrapping_add(u16::from(byte))
    });
    format!("{checksum:04x},{body}")
}

pub(super) fn create_sidebar(
    window: &str,
    width: u32,
    position: SidebarPosition,
    geometry: &str,
    directory: &str,
    binary: &str,
) -> Result<String, String> {
    let original = tmux::display_message(window, "#{window_layout}");
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
    Ok(checksummed_layout(&body))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn closing_a_main_pane_restores_the_left_sidebar_without_reordering_panes() {
        let restored = restore_sidebar_width(
            "0000,187x40,0,0{93x40,0,0,4,93x40,94,0,1}",
            42,
            SidebarPosition::Left,
            "scratch:@1.%4",
        )
        .expect("restorable outer split");
        assert_eq!(
            restored.split_once(',').expect("checksum").1,
            "187x40,0,0{42x40,0,0,4,144x40,94,0,1}"
        );
    }

    #[test]
    fn width_restore_keeps_nested_main_pane_proportions_and_right_sidebar() {
        let restored = restore_sidebar_width(
            "0000,187x40,0,0{93x40,0,0[93x10,0,0,1,93x29,0,11{30x29,0,11,2,62x29,31,11,3}],93x40,94,0,4}",
            42, SidebarPosition::Right, "scratch:@1.%4",
        ).expect("restorable nested layout");
        assert_eq!(
            restored.split_once(',').expect("checksum").1,
            "187x40,0,0{144x40,0,0[93x10,0,0,1,93x29,0,11{30x29,0,11,2,62x29,31,11,3}],42x40,94,0,4}"
        );
    }

    #[test]
    fn width_restore_rejects_unsupported_layouts_and_wrong_sidebar_ids() {
        for layout in [
            "",
            "0000,187x40,0,0,4",
            "0000,187x40,0,0{93x40,0,0,5,93x40,94,0,1}",
            "0000,187x40,0,0{60x40,0,0,4,60x40,61,0,1,65x40,122,0,2}",
            "0000,187x40,0,0{93x40,0,0[93x20,0,0,4,93x19,0,21,3],93x40,94,0,1}",
        ] {
            assert!(restore_sidebar_width(layout, 42, SidebarPosition::Left, "%4").is_err());
        }
    }

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
