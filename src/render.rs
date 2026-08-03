use humansize::{DECIMAL, format_size};
use ratatui::{
    Frame,
    buffer::Buffer,
    layout::{Constraint, Direction, Layout, Rect},
    style::{Color, Style, Stylize},
    symbols,
    text::{Line, ToLine as _},
    widgets::{Block, BorderType, Clear, Fill, Paragraph, Widget},
};
use thousands::Separable;

use crate::state::AppState;
use crate::{
    config::Config,
    core::{Entry, EntryInfo, MaybePair, StackAddr, TreeSlice},
};
use crate::{
    config::DirStyle,
    forest::{key_range, partition},
};

pub fn render_subtree(
    config: &Config,
    state: &mut AppState,
    addr: &StackAddr,
    area: Rect,
    buf: &mut Buffer,
    tree: TreeSlice,
    selection: &[usize],
) {
    if tree.is_empty() {
        return;
    }

    // Can't display useful information if area is too small
    if tree.len() > 1 && (area.height < 2 || area.width <= 2) {
        let head = selection.first();
        let color = tree.first().map(|(_, it)| it.color).unwrap_or_default();
        let style = Style::from(color);
        if tree.iter().any(|(k, _)| Some(k) == head) {
            Fill::new("▓").style(style).render(area, buf);
        } else {
            Fill::new(symbols::DOT).style(style).render(area, buf);
        }

        let addr = addr.push(tree.first().unwrap().0);

        if let Some(click) = &state.click_pos
            && area.contains(*click)
            && state.click_area.intersection(area) == area
        {
            state.click_area = area;
            state.click_addr.clear();
            for id in &addr {
                state.click_addr.push(id)
            }

            state.click_addr.reverse();
        }

        return;
    }

    if tree.len() == 1 {
        let (key, entry) = &tree[0];

        let addr = addr.push(*key);
        render_entry(config, state, &addr, area, buf, entry, selection);

        return;
    }

    match partition(tree) {
        MaybePair::One(entries) => {
            render_subtree(config, state, addr, area, buf, entries, selection);
            // Paragraph::new(format!("{entries:?}"))
            //     .centered()
            //     .render(area, buf);
        }
        MaybePair::Two(left, right) => {
            let l = key_range(left).map(|r| (r.end - r.start) as f32).unwrap();
            let r = key_range(right).map(|r| (r.end - r.start) as f32).unwrap();

            // Must interpolate multi-gigabytes down to u16 range
            let lr = (l + r) / 1E5;
            let l = (l / lr) as u16;
            let r = (r / lr) as u16;

            let direction = if area.width > area.height * 2 {
                Direction::Horizontal
            } else {
                Direction::Vertical
            };

            let mut layout = Layout::default()
                .direction(direction)
                .constraints(vec![Constraint::Fill(l), Constraint::Fill(r)])
                .split(area);

            // Ensure tiny left-overs are always represented even if it skews proportions
            if layout[1].width == 0 || layout[1].height == 0 {
                layout = Layout::default()
                    .direction(direction)
                    .constraints(vec![Constraint::Percentage(100), Constraint::Min(1)])
                    .split(area);
            }

            render_subtree(config, state, addr, layout[0], buf, left, selection);
            render_subtree(config, state, addr, layout[1], buf, right, selection);
        }
    }
}

pub fn render_entry(
    config: &Config,
    state: &mut AppState,
    addr: &StackAddr,
    area: Rect,
    buf: &mut Buffer,
    entry: &Entry,
    selection: &[usize],
) {
    let Entry {
        path,
        size,
        subtree,
        is_group,
        nfiles,
        ..
    } = entry;

    let title = path.file_name().unwrap_or_default();
    let display = title.display();

    if let Some(click) = &state.click_pos
        && area.contains(*click)
        && state.click_area.intersection(area) == area
    {
        state.click_area = area;
        state.click_addr.clear();
        for id in addr {
            state.click_addr.push(id)
        }

        state.click_addr.reverse();
    }

    let (selected, selection) = if selection.first() == addr.head() {
        (true, &selection[1..])
    } else {
        (false, [].as_slice())
    };

    let style = Style::from(entry.color);

    let mut block = Block::bordered()
        .title(display.to_line())
        .border_style(style);

    if config.dir_style == DirStyle::Thick {
        block = block.border_type(BorderType::Thick);
    }

    if area.height > 1 {
        // let mut a = addr.collect_vec();
        // a.reverse();
        // block = block.title_bottom(format!("{a:?}"));
        block = block.title_bottom(format_size(*size, DECIMAL));
    }

    if selected {
        block = block.border_type(BorderType::QuadrantInside);
    } else if *is_group {
        block = block.border_type(BorderType::Double);
    } else if subtree.is_empty() && *nfiles == 1 {
        block = block.border_type(BorderType::LightDoubleDashed);
    }

    let inner = block.inner(area);
    block.render(area, buf);
    if subtree.is_empty() {
        Fill::new(if selected { "▓" } else { "▒" })
            .style(style)
            .render(inner, buf);
    } else if inner.height > 2 || inner.width > 2 {
        render_subtree(config, state, addr, inner, buf, subtree, selection);
    }
}

pub fn render_delete_modal(frame: &mut Frame, info: &EntryInfo) {
    let popup_area = frame.area().centered(
        Constraint::Length(65),
        Constraint::Length(10),
    );

    frame.render_widget(Clear, popup_area);

    let is_dir = info.path.is_dir();
    let item_type = if is_dir { "Directory" } else { "File" };
    let path_str = info.path.display().to_string();

    let text = vec![
        Line::from(vec![
            "REMOVE ".red().bold(),
            item_type.into(),
            "?".into(),
        ]),
        Line::raw(""),
        Line::from(vec![
            "Path: ".yellow(),
            path_str.into(),
        ]),
        Line::from(vec![
            "Size: ".yellow(),
            format_size(info.size, DECIMAL).bold(),
            format!(" ({} files)", info.nfiles.separate_with_commas()).into(),
        ]),
        Line::raw(""),
        Line::from(vec![
            "Press ".into(),
            "y".red().bold(),
            " / ".into(),
            "Enter".red().bold(),
            " to confirm, ".into(),
            "n".green().bold(),
            " / ".into(),
            "Esc".green().bold(),
            " to cancel".into(),
        ]),
    ];

    let block = Block::bordered()
        .title(" Confirm Deletion ".bold().red())
        .title_alignment(ratatui::layout::Alignment::Center)
        .border_style(Style::default().fg(Color::Red));

    let paragraph = Paragraph::new(text)
        .block(block)
        .centered();

    frame.render_widget(paragraph, popup_area);
}
