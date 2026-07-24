use std::fmt::Debug;
use std::path::Path;

use humansize::{DECIMAL, format_size};
use ratatui::layout::{Position, Rect};

use thousands::Separable;
use tui_tree_widget::TreeState;
use typed_builder::TypedBuilder;
use typed_path::TypedPathBuf;

use crate::core::{Entry, EntryInfo, Forest, path_from_std};

#[derive(Clone, Copy, Debug, Default)]
pub enum AppMode {
    #[default]
    Normal,
    Xray,
}

#[derive(Clone, Debug, Default)]
pub enum AppAction {
    #[default]
    Noop,
    SwitchMode(AppMode),
    Deflate,
    Expand,
    ForceRescan,
}

#[derive(TypedBuilder)]
pub struct AppState {
    #[cfg(feature = "clipboard")]
    #[builder(default)]
    pub clipboard: Option<arboard::Clipboard>,

    pub root: TypedPathBuf,

    #[builder(default)]
    pub mode: AppMode,
    #[builder(default)]
    pub action: AppAction,
    #[builder(default)]
    pub diagnostic: bool,

    #[builder(default)]
    pub view_info: Option<EntryInfo>,
    #[builder(default)]
    pub title: Option<String>,
    #[builder(default)]
    pub skip_view: Vec<usize>,
    #[builder(default)]
    pub tree_state: TreeState<usize>,
    #[builder(default)]
    pub tag: Option<Vec<u8>>,

    #[builder(default)]
    pub click_pos: Option<Position>,
    #[builder(default)]
    pub click_area: Rect,
    #[builder(default)]
    pub click_addr: Vec<usize>,
}

impl AppState {
    pub fn new(path: impl AsRef<Path>, mode: AppMode) -> Self {
        let this = Self::builder().root(path_from_std(path)).mode(mode);

        #[cfg(feature = "clipboard")]
        let this = this.clipboard(arboard::Clipboard::new().ok());

        this.build()
    }

    /// Address of selection qualified with current view
    pub fn qual_select(&self) -> Vec<usize> {
        let mut selection = self.skip_view.clone();
        selection.extend(self.tree_state.selected());
        selection
    }

    pub fn show_selection(&mut self, addr: &[usize]) {
        for i in 0..addr.len() {
            self.tree_state.open(addr[..i].to_vec());
        }

        self.tree_state.select(addr.to_vec());
    }
}

#[ouroboros::self_referencing(pub_extras)]
pub struct TreeFocus {
    pub tree: Forest,

    #[borrows(tree)]
    #[covariant]
    pub focus: Option<&'this Entry>,
}

impl Default for TreeFocus {
    fn default() -> Self {
        TreeFocusBuilder {
            tree: Default::default(),
            focus_builder: |_| None,
        }
        .build()
    }
}

impl TreeFocus {
    pub fn select(&mut self, selection: &[usize]) {
        self.with_mut(|fields| {
            *fields.focus = get_selection(selection, fields.tree);
        });
    }
}

pub fn get_title(state: &AppState, info: &EntryInfo) -> String {
    let mut title = if info.tag.is_none() {
        info.path.to_string_lossy().into_owned()
    } else if let Some(tag) = &info.tag {
        info.path
            .join("**")
            .with_extension(tag)
            .to_string_lossy()
            .into_owned()
    } else {
        state.root.to_string_lossy().into_owned()
    };

    title.push_str(&format!(
        " | {} ({} files)",
        format_size(info.size, DECIMAL),
        info.nfiles.separate_with_commas()
    ));
    title
}

pub fn get_selection<'a>(
    mut selection: &[usize],
    mut level: &'a [(usize, Entry)],
) -> Option<&'a Entry> {
    while let Some(id) = selection.first()
        && let Ok(idx) = level.binary_search_by_key(id, |(k, _)| *k)
    {
        let entry = &level[idx].1;
        if selection.len() == 1 {
            return Some(entry);
        }

        selection = &selection[1..];
        level = &entry.subtree;
    }

    None
}
