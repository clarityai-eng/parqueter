use std::cell::Cell;
use std::collections::HashSet;
use std::rc::Rc;
use std::time::Duration;

use gpui::{prelude::*, *};

use crate::data::DataSource;

const ROW_NUM_WIDTH: f32 = 60.0;
const ROW_HEIGHT: f32 = 28.0;
const HEADER_HEIGHT: f32 = 58.0;
const SCROLLBAR_SIZE: f32 = 10.0;
const SCROLLBAR_MIN_THUMB: f32 = 20.0;
/// Inset around the table so rounded window corners don't clip the scrollbars or cells.
const TABLE_MARGIN: f32 = 10.0;

fn to_f32(p: Pixels) -> f32 {
    let v: f32 = p.into();
    v
}

/// Scrollbar geometry along the scroll axis, in window coordinates.
/// Written while painting, read by the mouse handlers.
#[derive(Clone, Copy)]
struct ThumbGeom {
    track_origin: f32,
    track_len: f32,
    thumb_start: f32, // relative to track_origin
    thumb_len: f32,
}

/// `visible` is the fraction of the content shown in the viewport,
/// `position` how far it is scrolled (0 = start, 1 = end).
fn thumb_geom(track_origin: f32, track_len: f32, visible: f32, position: f32) -> Option<ThumbGeom> {
    if track_len <= 0.0 || visible >= 1.0 {
        return None;
    }
    let thumb_len = (track_len * visible).max(SCROLLBAR_MIN_THUMB).min(track_len);
    let thumb_start = position.clamp(0.0, 1.0) * (track_len - thumb_len);
    Some(ThumbGeom {
        track_origin,
        track_len,
        thumb_start,
        thumb_len,
    })
}

/// Returns the offset that shows `[start, start + len)` in a view of `view_len`
/// currently at `offset`, moving as little as possible. An item that doesn't fit
/// is aligned to its start.
fn reveal(offset: f64, view_len: f64, start: f64, len: f64) -> f64 {
    if start < offset || len >= view_len {
        start
    } else if start + len > offset + view_len {
        start + len - view_len
    } else {
        offset
    }
}

/// A cursor movement requested from the keyboard.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Move {
    Up,
    Down,
    Left,
    Right,
    PageUp,
    PageDown,
    FirstColumn,
    LastColumn,
    FirstRow,
    LastRow,
}

/// The cell the cursor lands on after `mv`. `row` may be `RootView::HEADER_ROW`;
/// the cursor can leave the header downwards but never enters it.
/// Expects `rows > 0` or a header cursor, and `cols > 0`.
fn next_cell((row, col): (usize, usize), mv: Move, rows: usize, cols: usize, page: usize) -> (usize, usize) {
    let last_col = cols - 1;
    let col = match mv {
        Move::Left => col.saturating_sub(1),
        Move::Right => (col + 1).min(last_col),
        Move::FirstColumn => 0,
        Move::LastColumn => last_col,
        _ => col,
    };
    if rows == 0 {
        return (row, col);
    }
    let last_row = rows - 1;
    let row = if row == RootView::HEADER_ROW {
        match mv {
            Move::Down | Move::PageDown | Move::FirstRow => 0,
            Move::LastRow => last_row,
            _ => row,
        }
    } else {
        match mv {
            Move::Up => row.saturating_sub(1),
            Move::Down => (row + 1).min(last_row),
            Move::PageUp => row.saturating_sub(page),
            Move::PageDown => (row + page).min(last_row),
            Move::FirstRow => 0,
            Move::LastRow => last_row,
            _ => row,
        }
    };
    (row, col)
}

actions!(
    parqueter,
    [
        MoveUp,
        MoveDown,
        MoveLeft,
        MoveRight,
        PageUp,
        PageDown,
        FirstColumn,
        LastColumn,
        FirstRow,
        LastRow,
        CopyCell,
        OpenFile,
        ToggleSearch,
        DismissSearch,
        SearchNext,
        SearchPrev,
    ]
);

/// Cmd on macOS, Ctrl elsewhere. GPUI's `secondary-` prefix needs the host OS,
/// which `KeyBinding::new` doesn't pass, so it panics there.
macro_rules! secondary {
    ($key:literal) => {
        if cfg!(target_os = "macos") {
            concat!("cmd-", $key)
        } else {
            concat!("ctrl-", $key)
        }
    };
}

fn key_bindings() -> Vec<KeyBinding> {
    vec![
        KeyBinding::new("up", MoveUp, None),
        KeyBinding::new("down", MoveDown, None),
        KeyBinding::new("left", MoveLeft, None),
        KeyBinding::new("right", MoveRight, None),
        KeyBinding::new("pageup", PageUp, None),
        KeyBinding::new("pagedown", PageDown, None),
        KeyBinding::new("home", FirstColumn, None),
        KeyBinding::new("end", LastColumn, None),
        KeyBinding::new(secondary!("left"), FirstColumn, None),
        KeyBinding::new(secondary!("right"), LastColumn, None),
        KeyBinding::new(secondary!("home"), FirstRow, None),
        KeyBinding::new(secondary!("end"), LastRow, None),
        KeyBinding::new(secondary!("up"), FirstRow, None),
        KeyBinding::new(secondary!("down"), LastRow, None),
        KeyBinding::new(secondary!("c"), CopyCell, None),
        KeyBinding::new(secondary!("o"), OpenFile, None),
        // Kept on macOS too, for muscle memory from earlier versions.
        KeyBinding::new("ctrl-c", CopyCell, None),
        KeyBinding::new("ctrl-o", OpenFile, None),
        KeyBinding::new(secondary!("f"), ToggleSearch, None),
        KeyBinding::new("ctrl-f", ToggleSearch, None),
        KeyBinding::new("escape", DismissSearch, None),
    ]
}

pub fn register_actions(cx: &mut App) {
    cx.bind_keys(key_bindings());
}

pub const OPEN_SHORTCUT: &str = if cfg!(target_os = "macos") { "Cmd+O" } else { "Ctrl+O" };

pub enum AppState {
    Loaded {
        file_name: String,
        source: Rc<dyn DataSource>,
    },
    Error(String),
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum DragTarget {
    Vertical,
    Horizontal,
}

/// Column geometry derived from a data source.
struct ColumnLayout {
    row_num_width: f32,
    col_widths: Vec<f32>,
    col_positions: Vec<f32>,
    data_width: f32,
}

impl ColumnLayout {
    fn new(source: Option<&dyn DataSource>) -> Self {
        let Some(source) = source else {
            return Self {
                row_num_width: ROW_NUM_WIDTH,
                col_widths: vec![],
                col_positions: vec![],
                data_width: 0.0,
            };
        };
        let col_widths = source.column_widths();
        let mut col_positions = Vec::with_capacity(col_widths.len());
        let mut acc = 0.0f32;
        for &w in &col_widths {
            col_positions.push(acc);
            acc += w;
        }
        let digits = source.row_count().to_string().len();
        Self {
            row_num_width: (digits as f32 * 9.0 + 24.0).max(ROW_NUM_WIDTH),
            col_widths,
            col_positions,
            data_width: acc,
        }
    }

    /// Columns overlapping `[h_offset, h_offset + width)`: first, one past the last,
    /// and the x of the first column's left edge relative to the view (<= 0).
    fn visible_range(&self, h_offset: f32, width: f32) -> (usize, usize, f32) {
        let n = self.col_widths.len();
        if n == 0 {
            return (0, 0, 0.0);
        }
        let right = h_offset + width;
        // Column i ends where column i + 1 starts.
        let first = self.col_positions[1..].partition_point(|&end| end <= h_offset).min(n - 1);
        let last = self.col_positions.partition_point(|&pos| pos < right).min(n);
        (first, last, self.col_positions[first] - h_offset)
    }
}

pub struct RootView {
    state: AppState,
    layout: ColumnLayout,
    focus_handle: FocusHandle,
    // Vertical position in pixels. f64 because f32 (GPUI's Pixels) loses whole
    // pixels past ~16M px, i.e. ~600k rows, and small scroll deltas get dropped.
    v_offset: f64,
    h_offset: f32,
    // Height of the row area, measured at the last paint
    body_height: Rc<Cell<f32>>,
    // Scrollbar drag state
    dragging: Option<DragTarget>,
    drag_start_pos: f32,    // mouse position when drag started
    drag_start_offset: f64, // scroll offset when drag started
    drag_scale: f64,        // scroll pixels per mouse pixel
    drag_idle: bool,        // true when drag paused long enough to show data
    drag_generation: usize, // bumped on each mouse move, idle timer checks this
    selected_cell: Option<(usize, usize)>, // (row, col)
    toast: Option<String>,
    toast_generation: usize,
    // Scrollbar geometry from the last paint
    v_thumb: Rc<Cell<Option<ThumbGeom>>>,
    h_thumb: Rc<Cell<Option<ThumbGeom>>>,
    // Axis lock for trackpad gestures, so a vertical swipe doesn't drift sideways
    scroll_gesture: OngoingScroll,
    // Updated each render
    h_max_offset: f32,
    viewport_w: f32, // width of the data columns area, excluding row numbers
    // Search
    search_active: bool,
    search_query: String,
    search_matches: Vec<(usize, usize)>,
    search_match_set: HashSet<(usize, usize)>,
    search_index: usize,
    search_focus: FocusHandle,
}

impl RootView {
    pub fn new(state: AppState, cx: &mut Context<Self>) -> Self {
        let layout = match &state {
            AppState::Loaded { source, .. } => ColumnLayout::new(Some(source.as_ref())),
            _ => ColumnLayout::new(None),
        };
        Self {
            state,
            layout,
            focus_handle: cx.focus_handle(),
            v_offset: 0.0,
            h_offset: 0.0,
            body_height: Rc::new(Cell::new(0.0)),
            dragging: None,
            drag_start_pos: 0.0,
            drag_start_offset: 0.0,
            drag_scale: 1.0,
            drag_idle: false,
            drag_generation: 0,
            selected_cell: None,
            toast: None,
            toast_generation: 0,
            v_thumb: Rc::new(Cell::new(None)),
            h_thumb: Rc::new(Cell::new(None)),
            scroll_gesture: OngoingScroll::default(),
            h_max_offset: 0.0,
            viewport_w: 0.0,
            search_active: false,
            search_query: String::new(),
            search_matches: Vec::new(),
            search_match_set: HashSet::new(),
            search_index: 0,
            search_focus: cx.focus_handle(),
        }
    }

    fn row_count(&self) -> usize {
        match &self.state {
            AppState::Loaded { source, .. } => source.row_count(),
            _ => 0,
        }
    }

    // -- Scrolling --

    /// Max vertical offset. Leaves room below the last row for the horizontal
    /// scrollbar so it never covers data.
    fn v_max_offset(&self) -> f64 {
        let bottom_pad = if self.h_max_offset > 0.0 { SCROLLBAR_SIZE } else { 0.0 };
        let content = self.row_count() as f64 * ROW_HEIGHT as f64 + bottom_pad as f64;
        (content - self.body_height.get() as f64).max(0.0)
    }

    fn max_offset(&self, target: DragTarget) -> f64 {
        match target {
            DragTarget::Vertical => self.v_max_offset(),
            DragTarget::Horizontal => self.h_max_offset as f64,
        }
    }

    fn scroll_offset(&self, target: DragTarget) -> f64 {
        match target {
            DragTarget::Vertical => self.v_offset,
            DragTarget::Horizontal => self.h_offset as f64,
        }
    }

    /// Clamps and applies the offset. Returns true when it changed.
    fn set_scroll_offset(&mut self, target: DragTarget, offset: f64) -> bool {
        let offset = offset.clamp(0.0, self.max_offset(target));
        match target {
            DragTarget::Vertical => {
                let changed = offset != self.v_offset;
                self.v_offset = offset;
                changed
            }
            DragTarget::Horizontal => {
                let offset = offset as f32;
                let changed = offset != self.h_offset;
                self.h_offset = offset;
                changed
            }
        }
    }

    // -- Keyboard cursor --

    /// Height of the rows actually visible: the horizontal scrollbar covers the bottom.
    fn visible_rows_height(&self) -> f32 {
        let pad = if self.h_max_offset > 0.0 { SCROLLBAR_SIZE } else { 0.0 };
        (self.body_height.get() - pad).max(0.0)
    }

    /// Width of the columns actually visible: the vertical scrollbar covers the right edge.
    fn visible_cols_width(&self) -> f32 {
        (self.viewport_w - SCROLLBAR_SIZE).max(0.0)
    }

    /// Rows that fit fully in the view, at least one.
    fn page_rows(&self) -> usize {
        ((self.visible_rows_height() / ROW_HEIGHT).floor() as usize).max(1)
    }

    /// Top-left cell that is fully visible, or the nearest partly visible one.
    fn first_visible_cell(&self) -> (usize, usize) {
        let l = &self.layout;
        let row = ((self.v_offset / ROW_HEIGHT as f64).ceil() as usize).min(self.row_count().saturating_sub(1));
        let mut col = l.col_positions.partition_point(|&p| p < self.h_offset);
        // No column starts inside the view (one wide column covers it): take the
        // column the view starts in.
        if col >= l.col_positions.len() || l.col_positions[col] >= self.h_offset + self.visible_cols_width() {
            col = col.saturating_sub(1);
        }
        (row, col)
    }

    /// Scrolls the least amount that shows the whole selected cell.
    fn reveal_selection(&mut self) {
        let Some((row, col)) = self.selected_cell else { return };
        if row != Self::HEADER_ROW {
            let row_h = ROW_HEIGHT as f64;
            let offset = reveal(self.v_offset, self.visible_rows_height() as f64, row as f64 * row_h, row_h);
            self.set_scroll_offset(DragTarget::Vertical, offset);
        }
        let l = &self.layout;
        if let (Some(&start), Some(&width)) = (l.col_positions.get(col), l.col_widths.get(col)) {
            let offset = reveal(
                self.h_offset as f64,
                self.visible_cols_width() as f64,
                start as f64,
                width as f64,
            );
            self.set_scroll_offset(DragTarget::Horizontal, offset);
        }
    }

    fn move_cursor(&mut self, mv: Move, cx: &mut Context<Self>) {
        let rows = self.row_count();
        let cols = self.layout.col_widths.len();
        if cols == 0 {
            return;
        }
        match self.selected_cell {
            None => {
                // The first key press only picks a starting cell.
                if rows == 0 {
                    return;
                }
                self.selected_cell = Some(self.first_visible_cell());
            }
            Some(cell) => {
                let (row, col) = next_cell(cell, mv, rows, cols, self.page_rows());
                // Page keys scroll by the rows moved, so the cursor keeps its place
                // on screen; reveal_selection then fixes up the edges.
                if matches!(mv, Move::PageUp | Move::PageDown) && cell.0 != Self::HEADER_ROW {
                    let delta = (row as f64 - cell.0 as f64) * ROW_HEIGHT as f64;
                    self.set_scroll_offset(DragTarget::Vertical, self.v_offset + delta);
                }
                self.selected_cell = Some((row, col));
            }
        }
        self.reveal_selection();
        cx.notify();
    }

    fn move_up(&mut self, _: &MoveUp, _: &mut Window, cx: &mut Context<Self>) {
        self.move_cursor(Move::Up, cx);
    }

    fn move_down(&mut self, _: &MoveDown, _: &mut Window, cx: &mut Context<Self>) {
        self.move_cursor(Move::Down, cx);
    }

    fn move_left(&mut self, _: &MoveLeft, _: &mut Window, cx: &mut Context<Self>) {
        self.move_cursor(Move::Left, cx);
    }

    fn move_right(&mut self, _: &MoveRight, _: &mut Window, cx: &mut Context<Self>) {
        self.move_cursor(Move::Right, cx);
    }

    fn page_up(&mut self, _: &PageUp, _: &mut Window, cx: &mut Context<Self>) {
        self.move_cursor(Move::PageUp, cx);
    }

    fn page_down(&mut self, _: &PageDown, _: &mut Window, cx: &mut Context<Self>) {
        self.move_cursor(Move::PageDown, cx);
    }

    fn first_column(&mut self, _: &FirstColumn, _: &mut Window, cx: &mut Context<Self>) {
        self.move_cursor(Move::FirstColumn, cx);
    }

    fn last_column(&mut self, _: &LastColumn, _: &mut Window, cx: &mut Context<Self>) {
        self.move_cursor(Move::LastColumn, cx);
    }

    fn first_row(&mut self, _: &FirstRow, _: &mut Window, cx: &mut Context<Self>) {
        self.move_cursor(Move::FirstRow, cx);
    }

    fn last_row(&mut self, _: &LastRow, _: &mut Window, cx: &mut Context<Self>) {
        self.move_cursor(Move::LastRow, cx);
    }

    /// Wheel / trackpad scrolling on both axes. Shift+wheel arrives as an X delta
    /// on every platform.
    fn on_table_scroll(&mut self, event: &ScrollEvent, window: &mut Window, cx: &mut Context<Self>) {
        let mut delta = event.delta.pixel_delta(window.line_height());
        if event.delta.precise() {
            self.scroll_gesture.filter(&mut delta, event.touch_phase);
        }
        // Positive delta moves content right/down, i.e. scrolls towards the start.
        let dx = -to_f32(delta.x) as f64;
        let dy = -to_f32(delta.y) as f64;
        let mut changed = false;
        if dx != 0.0 {
            changed |= self.set_scroll_offset(DragTarget::Horizontal, self.h_offset as f64 + dx);
        }
        if dy != 0.0 {
            changed |= self.set_scroll_offset(DragTarget::Vertical, self.v_offset + dy);
        }
        if changed {
            cx.notify();
        }
    }

    // -- Toast, files, clipboard --

    const HEADER_ROW: usize = usize::MAX;
    const TOAST_DURATION_MS: u64 = 1500;

    fn show_toast(&mut self, msg: String, window: &mut Window, cx: &mut Context<Self>) {
        self.toast = Some(msg);
        self.toast_generation = self.toast_generation.wrapping_add(1);
        let generation = self.toast_generation;
        cx.spawn_in(window, async move |this, cx| {
            cx.background_executor()
                .timer(Duration::from_millis(RootView::TOAST_DURATION_MS))
                .await;
            this.update(cx, |view, cx| {
                if view.toast_generation == generation {
                    view.toast = None;
                    cx.notify();
                }
            })
            .ok();
        })
        .detach();
        cx.notify();
    }

    fn open_file(&mut self, _: &OpenFile, window: &mut Window, cx: &mut Context<Self>) {
        let receiver = cx.prompt_for_paths(PathPromptOptions {
            files: true,
            directories: false,
            multiple: false,
            prompt: Some("Open Parquet file".into()),
        });
        cx.spawn_in(window, async move |this, cx| {
            if let Ok(Ok(Some(paths))) = receiver.await
                && let Some(path) = paths.into_iter().next()
            {
                this.update_in(cx, |view, window, cx| view.load_file(&path, window, cx))
                    .ok();
            }
        })
        .detach();
    }

    pub(crate) fn load_file(&mut self, path: &std::path::Path, window: &mut Window, cx: &mut Context<Self>) {
        use crate::data::LazyParquetSource;

        let file_name = path
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_else(|| path.display().to_string());

        match LazyParquetSource::open(path) {
            Ok(source) => {
                let source: Rc<dyn DataSource> = Rc::new(source);
                self.layout = ColumnLayout::new(Some(source.as_ref()));
                self.state = AppState::Loaded { file_name, source };
                self.v_offset = 0.0;
                self.h_offset = 0.0;
                self.selected_cell = None;
                self.search_active = false;
                self.search_query.clear();
                self.search_matches.clear();
                self.search_match_set.clear();
                self.search_index = 0;
                self.show_toast(format!("Opened: {}", path.display()), window, cx);
            }
            Err(e) => {
                self.state = AppState::Error(e);
                cx.notify();
            }
        }
    }

    fn cell_text_for_selection(&self, row: usize, col: usize) -> Option<String> {
        let AppState::Loaded { source, .. } = &self.state else {
            return None;
        };
        if row == Self::HEADER_ROW {
            Some(source.column_name(col).to_string())
        } else {
            Some(source.cell_full(row, col))
        }
    }

    fn copy_to_clipboard(&mut self, row: usize, col: usize, window: &mut Window, cx: &mut Context<Self>) {
        let Some(text) = self.cell_text_for_selection(row, col) else { return };
        cx.write_to_clipboard(ClipboardItem::new_string(text.clone()));
        let preview = if text.chars().count() > 40 {
            format!("\"{}...\" copied", text.chars().take(37).collect::<String>())
        } else {
            format!("\"{text}\" copied")
        };
        self.show_toast(preview, window, cx);
    }

    fn copy_cell(&mut self, _: &CopyCell, window: &mut Window, cx: &mut Context<Self>) {
        if let Some((row, col)) = self.selected_cell {
            self.copy_to_clipboard(row, col, window, cx);
        }
    }

    fn select_and_maybe_copy(
        &mut self,
        row: usize,
        col: usize,
        click_count: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.selected_cell = Some((row, col));
        if click_count >= 2 {
            self.copy_to_clipboard(row, col, window, cx);
        }
        cx.notify();
    }

    // -- Search --

    fn toggle_search(&mut self, _: &ToggleSearch, window: &mut Window, cx: &mut Context<Self>) {
        if !matches!(self.state, AppState::Loaded { .. }) {
            return;
        }
        if self.search_active {
            self.search_active = false;
            self.clear_search();
            self.focus_handle.focus(window, cx);
        } else {
            self.search_active = true;
            self.search_focus.focus(window, cx);
        }
        cx.notify();
    }

    fn dismiss_search(&mut self, _: &DismissSearch, window: &mut Window, cx: &mut Context<Self>) {
        if !self.search_active {
            return;
        }
        self.search_active = false;
        self.clear_search();
        self.focus_handle.focus(window, cx);
        cx.notify();
    }

    fn clear_search(&mut self) {
        self.search_matches.clear();
        self.search_match_set.clear();
        self.search_index = 0;
    }

    fn execute_search(&mut self, cx: &mut Context<Self>) {
        self.search_matches.clear();
        self.search_match_set.clear();
        self.search_index = 0;

        let query = self.search_query.trim().to_lowercase();
        if query.is_empty() {
            cx.notify();
            return;
        }

        let AppState::Loaded { source, .. } = &self.state else {
            return;
        };

        let rows = source.row_count();
        let cols = source.col_count();

        // Search headers
        for col in 0..cols {
            if source.column_name(col).to_lowercase().contains(&query) {
                self.search_matches.push((Self::HEADER_ROW, col));
            }
        }

        // Search data rows
        for row in 0..rows {
            for col in 0..cols {
                let text = source.cell(row, col);
                if text.to_lowercase().contains(&query) {
                    self.search_matches.push((row, col));
                }
            }
        }

        self.search_match_set = self.search_matches.iter().copied().collect();

        if !self.search_matches.is_empty() {
            let (row, col) = self.search_matches[0];
            self.selected_cell = Some((row, col));
            self.reveal_selection();
        }

        cx.notify();
    }

    fn search_next(&mut self, _: &SearchNext, _window: &mut Window, cx: &mut Context<Self>) {
        if self.search_matches.is_empty() {
            return;
        }
        self.search_index = (self.search_index + 1) % self.search_matches.len();
        let (row, col) = self.search_matches[self.search_index];
        self.selected_cell = Some((row, col));
        self.reveal_selection();
        cx.notify();
    }

    fn search_prev(&mut self, _: &SearchPrev, _window: &mut Window, cx: &mut Context<Self>) {
        if self.search_matches.is_empty() {
            return;
        }
        if self.search_index == 0 {
            self.search_index = self.search_matches.len() - 1;
        } else {
            self.search_index -= 1;
        }
        let (row, col) = self.search_matches[self.search_index];
        self.selected_cell = Some((row, col));
        self.reveal_selection();
        cx.notify();
    }

    // -- Scrollbar drag handlers --

    fn on_scrollbar_mouse_down(
        &mut self,
        target: DragTarget,
        event: &MouseDownEvent,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let geom = match target {
            DragTarget::Vertical => self.v_thumb.get(),
            DragTarget::Horizontal => self.h_thumb.get(),
        };
        let Some(g) = geom else { return };
        let max = self.max_offset(target);
        let travel = g.track_len - g.thumb_len;
        if max <= 0.0 || travel <= 0.0 {
            return;
        }
        let pos = match target {
            DragTarget::Vertical => to_f32(event.position.y),
            DragTarget::Horizontal => to_f32(event.position.x),
        };
        let in_track = pos - g.track_origin;
        // Clicking the track (not the thumb) centers the thumb on the click,
        // then the drag continues from there.
        if in_track < g.thumb_start || in_track > g.thumb_start + g.thumb_len {
            let ratio = ((in_track - g.thumb_len / 2.0) / travel).clamp(0.0, 1.0);
            self.set_scroll_offset(target, ratio as f64 * max);
        }
        self.dragging = Some(target);
        self.drag_start_pos = pos;
        self.drag_start_offset = self.scroll_offset(target);
        self.drag_scale = max / travel as f64; // thumb follows the cursor 1:1
        self.drag_idle = false;
        cx.notify();
    }

    /// Called from a window-level listener, so it keeps firing when the cursor
    /// leaves the window during a drag.
    fn on_drag_move(&mut self, event: &MouseMoveEvent, window: &mut Window, cx: &mut Context<Self>) {
        let Some(target) = self.dragging else { return };
        // Button no longer held: the release happened somewhere we didn't see
        // (e.g. focus moved to another app). End the drag instead of sticking.
        if event.pressed_button != Some(MouseButton::Left) {
            self.end_drag(cx);
            return;
        }
        let pos = match target {
            DragTarget::Vertical => to_f32(event.position.y),
            DragTarget::Horizontal => to_f32(event.position.x),
        };
        let offset = self.drag_start_offset + (pos - self.drag_start_pos) as f64 * self.drag_scale;
        self.set_scroll_offset(target, offset);

        // Movement detected: reset idle, bump generation, schedule idle check
        self.drag_idle = false;
        self.drag_generation = self.drag_generation.wrapping_add(1);
        let generation = self.drag_generation;
        cx.spawn_in(window, async move |this, cx| {
            cx.background_executor().timer(Duration::from_millis(100)).await;
            this.update(cx, |view, cx| {
                if view.dragging.is_some() && view.drag_generation == generation {
                    view.drag_idle = true;
                    cx.notify();
                }
            })
            .ok();
        })
        .detach();
        cx.notify();
    }

    fn end_drag(&mut self, cx: &mut Context<Self>) {
        if self.dragging.is_some() {
            self.dragging = None;
            self.drag_idle = false;
            cx.notify();
        }
    }
}

impl Focusable for RootView {
    fn focus_handle(&self, _cx: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl Render for RootView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let content = match &self.state {
            AppState::Error(msg) => {
                let view = cx.entity().downgrade();
                div()
                    .flex()
                    .flex_col()
                    .gap_4()
                    .size_full()
                    .justify_center()
                    .items_center()
                    .child(
                        div()
                            .text_color(rgb(0xf38ba8))
                            .text_xl()
                            .child(msg.clone()),
                    )
                    .child(
                        div()
                            .id("open-file-btn")
                            .px_4()
                            .py_2()
                            .rounded_md()
                            .bg(rgb(0x45475a))
                            .text_color(rgb(0xcdd6f4))
                            .cursor_pointer()
                            .hover(|s| s.bg(rgb(0x585b70)))
                            .child(format!("Open File ({OPEN_SHORTCUT})"))
                            .on_click(move |_, window, cx| {
                                if let Some(entity) = view.upgrade() {
                                    entity.update(cx, |this, cx| {
                                        this.open_file(&OpenFile, window, cx);
                                    });
                                }
                            }),
                    )
                    .into_any_element()
            }

            AppState::Loaded { file_name, source } => {
                let source = Rc::clone(source);
                let file_name = file_name.clone();
                let col_count = source.col_count();
                let row_count = source.row_count();
                let row_num_w = self.layout.row_num_width;

                // -- Horizontal geometry --
                let window_w = to_f32(window.viewport_size().width);
                let viewport_w = (window_w - 2.0 * TABLE_MARGIN - row_num_w).max(0.0);
                // Extra SCROLLBAR_SIZE so the last column can scroll clear of the vertical scrollbar.
                self.h_max_offset = (self.layout.data_width + SCROLLBAR_SIZE - viewport_w).max(0.0);
                self.h_offset = self.h_offset.min(self.h_max_offset); // window may have grown
                self.viewport_w = viewport_w;
                let h_scrollable = self.h_max_offset > 0.0;
                let (vis_first, vis_last, spacer) = self.layout.visible_range(self.h_offset, viewport_w);
                let vis_widths: Vec<f32> = self.layout.col_widths[vis_first..vis_last].to_vec();

                // -- Vertical geometry --
                // Rows are virtualized by hand instead of with uniform_list, whose
                // f32 offset breaks down on files with millions of rows.
                let v_max = self.v_max_offset();
                self.v_offset = self.v_offset.min(v_max); // window may have grown
                let row_h = ROW_HEIGHT as f64;
                let first_row = ((self.v_offset / row_h) as usize).min(row_count);
                let row_shift = (self.v_offset - first_row as f64 * row_h) as f32;
                // The window height always covers the row area, even before it's measured.
                let rows_to_draw =
                    (to_f32(window.viewport_size().height) / ROW_HEIGHT).ceil() as usize + 2;
                let last_row = (first_row + rows_to_draw).min(row_count);

                // File info header
                let file_header = div()
                    .pb_2()
                    .px_4()
                    .pt_4()
                    .border_b_1()
                    .border_color(rgb(0x45475a))
                    .child(
                        div()
                            .text_xl()
                            .text_color(rgb(0x89b4fa))
                            .child(file_name),
                    )
                    .child(
                        div()
                            .text_sm()
                            .text_color(rgb(0x6c7086))
                            .child(format!("{col_count} columns · {row_count} rows")),
                    );

                // -- Search highlight state --
                let current_match = self.search_matches.get(self.search_index).copied();
                let search_match_set = &self.search_match_set;

                // -- Column header row --
                let header_bg = rgb(0x313244);
                let header_selected_bg = rgb(0x585b70);
                let view_entity = cx.entity().downgrade();
                let mut header_data = div().flex().ml(px(spacer));
                for (i, col) in (vis_first..vis_last).enumerate() {
                    let is_selected = self.selected_cell == Some((Self::HEADER_ROW, col));
                    let is_hdr_current = current_match == Some((Self::HEADER_ROW, col));
                    let is_hdr_match = search_match_set.contains(&(Self::HEADER_ROW, col));
                    let bg = if is_hdr_current {
                        rgb(0xf9e2af)
                    } else if is_hdr_match {
                        rgb(0x7c6f3e)
                    } else if is_selected {
                        header_selected_bg
                    } else {
                        header_bg
                    };
                    let view = view_entity.clone();
                    header_data = header_data.child(
                        div()
                            .id(ElementId::NamedInteger("hdr".into(), col as u64))
                            .debug_selector(|| format!("hdr-{col}"))
                            .flex_shrink_0()
                            .w(px(vis_widths[i]))
                            .h(px(HEADER_HEIGHT))
                            .px_2()
                            .py_1()
                            .border_r_1()
                            .border_b_1()
                            .border_color(rgb(0x45475a))
                            .bg(bg)
                            .overflow_hidden()
                            .cursor_pointer()
                            .on_click(move |event: &ClickEvent, window, cx| {
                                let count = event.click_count();
                                view.update(cx, |this, cx| {
                                    this.select_and_maybe_copy(RootView::HEADER_ROW, col, count, window, cx);
                                })
                                .ok();
                            })
                            .child(
                                div()
                                    .text_sm()
                                    .text_color(rgb(0x89b4fa))
                                    .whitespace_nowrap()
                                    .text_ellipsis()
                                    .child(source.column_name(col).to_string()),
                            )
                            .child(
                                div()
                                    .flex()
                                    .justify_between()
                                    .text_xs()
                                    .text_color(rgb(0x6c7086))
                                    .child(format!("#{}", col + 1))
                                    .child(format!("type: {}", source.column_type(col))),
                            ),
                    );
                }

                let header_row = div()
                    .flex()
                    .flex_shrink_0()
                    .h(px(HEADER_HEIGHT))
                    .child(
                        div()
                            .flex_shrink_0()
                            .w(px(row_num_w))
                            .h(px(HEADER_HEIGHT))
                            .px_2()
                            .flex()
                            .items_center()
                            .border_r_1()
                            .border_b_1()
                            .border_color(rgb(0x45475a))
                            .bg(header_bg)
                            .child(div().text_sm().text_color(rgb(0x6c7086)).child("#")),
                    )
                    .child(
                        div()
                            .flex_shrink_0()
                            .w(px(viewport_w))
                            .h(px(HEADER_HEIGHT))
                            .overflow_hidden()
                            .child(header_data),
                    );

                // -- Data rows --
                let dragging_fast = self.dragging.is_some() && !self.drag_idle;
                let mut rows = div().absolute().top(px(-row_shift)).left_0().flex().flex_col();
                for row_idx in first_row..last_row {
                    let bg = if row_idx % 2 == 0 {
                        rgb(0x1e1e2e)
                    } else {
                        rgb(0x181825)
                    };

                    let mut data_cells = div().flex().ml(px(spacer));
                    for (i, col_idx) in (vis_first..vis_last).enumerate() {
                        // During drag: show data if cached (free), empty if not (skip I/O)
                        let show_data = !dragging_fast || source.is_cached(row_idx, col_idx);
                        let is_selected = self.selected_cell == Some((row_idx, col_idx));
                        let is_current_match = current_match == Some((row_idx, col_idx));
                        let is_search_match = search_match_set.contains(&(row_idx, col_idx));
                        let cell_bg = if is_current_match {
                            rgb(0xf9e2af) // bright amber for current match
                        } else if is_search_match {
                            rgb(0x7c6f3e) // muted amber for other matches
                        } else if is_selected {
                            rgb(0x45475a)
                        } else {
                            bg
                        };
                        let cell_text_color = if is_current_match {
                            rgb(0x1e1e2e) // dark text on bright background
                        } else {
                            rgb(0xcdd6f4)
                        };
                        let mut cell = div()
                            // Scoped under the row's id, so no collisions however many columns.
                            .id(col_idx)
                            .debug_selector(|| format!("cell-{row_idx}-{col_idx}"))
                            // Fixed width: without this, flex shrinks the cells to fit
                            // the clipped row, and how much depends on h_offset.
                            .flex_shrink_0()
                            .w(px(vis_widths[i]))
                            .h(px(ROW_HEIGHT))
                            .border_r_1()
                            .border_color(rgb(0x45475a))
                            .bg(cell_bg)
                            .cursor_pointer();
                        if show_data {
                            let view = view_entity.clone();
                            cell = cell
                                .on_click(move |event: &ClickEvent, window, cx| {
                                    let count = event.click_count();
                                    view.update(cx, |this, cx| {
                                        this.select_and_maybe_copy(row_idx, col_idx, count, window, cx);
                                    })
                                    .ok();
                                })
                                .px_2()
                                .flex()
                                .items_center()
                                .overflow_hidden()
                                .child(
                                    div()
                                        .text_sm()
                                        .text_color(cell_text_color)
                                        .whitespace_nowrap()
                                        .text_ellipsis()
                                        .child(source.cell(row_idx, col_idx)),
                                );
                        }
                        data_cells = data_cells.child(cell);
                    }

                    rows = rows.child(
                        div()
                            .id(row_idx)
                            .flex()
                            .h(px(ROW_HEIGHT))
                            .bg(bg)
                            .child(
                                div()
                                    .flex_shrink_0()
                                    .w(px(row_num_w))
                                    .h(px(ROW_HEIGHT))
                                    .px_2()
                                    .flex()
                                    .items_center()
                                    .border_r_1()
                                    .border_color(rgb(0x45475a))
                                    .bg(bg)
                                    .child(
                                        div()
                                            .text_xs()
                                            .text_color(rgb(0x585b70))
                                            .child((row_idx + 1).to_string()),
                                    ),
                            )
                            .child(
                                div()
                                    .flex_shrink_0()
                                    .w(px(viewport_w))
                                    .h(px(ROW_HEIGHT))
                                    .overflow_hidden()
                                    .child(data_cells),
                            ),
                    );
                }

                // Measure the row area; re-render if it changed (first frame, resize).
                let body_height = self.body_height.clone();
                let measure_view = view_entity.clone();
                let measure = canvas(
                    move |bounds, _, cx| {
                        let h = to_f32(bounds.size.height);
                        if body_height.get() != h {
                            body_height.set(h);
                            cx.defer(move |cx| {
                                measure_view.update(cx, |_, cx| cx.notify()).ok();
                            });
                        }
                    },
                    |_, _, _, _| {},
                )
                .absolute()
                .size_full();

                let data_area = div()
                    .relative()
                    .flex_1()
                    .min_h_0()
                    .overflow_hidden()
                    .child(rows)
                    .child(measure);

                // -- Scrollbars --
                // Tracks are sized by layout; the thumb is painted from the track's
                // actual bounds, which are also stored for the mouse handlers.
                let thumb_color = |active: bool| if active { rgb(0x7f849c) } else { rgb(0x585b70) };

                let body_h = self.body_height.get() as f64;
                let v_visible = if v_max > 0.0 { (body_h / (body_h + v_max)) as f32 } else { 1.0 };
                let v_position = if v_max > 0.0 { (self.v_offset / v_max) as f32 } else { 0.0 };
                let v_thumb = self.v_thumb.clone();
                let v_color = thumb_color(self.dragging == Some(DragTarget::Vertical));
                let v_scrollbar = div()
                    .id("v-scrollbar-track")
                    .absolute()
                    .right_0()
                    .top(px(HEADER_HEIGHT))
                    .bottom(px(if h_scrollable { SCROLLBAR_SIZE } else { 0.0 }))
                    .w(px(SCROLLBAR_SIZE))
                    .block_mouse_except_scroll() // don't click the cells underneath
                    .cursor_pointer()
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(|this, e, w, cx| {
                            this.on_scrollbar_mouse_down(DragTarget::Vertical, e, w, cx)
                        }),
                    )
                    .child(
                        canvas(
                            |_, _, _| {},
                            move |bounds, _, window, _| {
                                let geom = thumb_geom(
                                    to_f32(bounds.origin.y),
                                    to_f32(bounds.size.height),
                                    v_visible,
                                    v_position,
                                );
                                v_thumb.set(geom);
                                if let Some(g) = geom {
                                    let thumb = Bounds::new(
                                        point(bounds.origin.x, px(g.track_origin + g.thumb_start)),
                                        size(bounds.size.width, px(g.thumb_len)),
                                    );
                                    window.paint_quad(
                                        fill(thumb, v_color).corner_radii(px(SCROLLBAR_SIZE / 2.0)),
                                    );
                                }
                            },
                        )
                        .size_full(),
                    );

                let h_scrollbar = h_scrollable.then(|| {
                    let h_thumb = self.h_thumb.clone();
                    let data_w = self.layout.data_width + SCROLLBAR_SIZE;
                    let h_visible = viewport_w / data_w;
                    let h_position = self.h_offset / self.h_max_offset;
                    let h_color = thumb_color(self.dragging == Some(DragTarget::Horizontal));
                    div()
                        .id("h-scrollbar-track")
                        .absolute()
                        .bottom_0()
                        .left(px(row_num_w))
                        .w(px((viewport_w - SCROLLBAR_SIZE).max(0.0)))
                        .h(px(SCROLLBAR_SIZE))
                        .block_mouse_except_scroll()
                        .cursor_pointer()
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(|this, e, w, cx| {
                                this.on_scrollbar_mouse_down(DragTarget::Horizontal, e, w, cx)
                            }),
                        )
                        .child(
                            canvas(
                                |_, _, _| {},
                                move |bounds, _, window, _| {
                                    let geom = thumb_geom(
                                        to_f32(bounds.origin.x),
                                        to_f32(bounds.size.width),
                                        h_visible,
                                        h_position,
                                    );
                                    h_thumb.set(geom);
                                    if let Some(g) = geom {
                                        let thumb = Bounds::new(
                                            point(px(g.track_origin + g.thumb_start), bounds.origin.y),
                                            size(px(g.thumb_len), bounds.size.height),
                                        );
                                        window.paint_quad(
                                            fill(thumb, h_color)
                                                .corner_radii(px(SCROLLBAR_SIZE / 2.0)),
                                        );
                                    }
                                },
                            )
                            .size_full(),
                        )
                });
                if !h_scrollable {
                    self.h_thumb.set(None);
                }

                let body = div()
                    .flex()
                    .flex_col()
                    .flex_1()
                    .min_h_0()
                    .child(header_row)
                    .child(data_area);

                let mut table_area = div()
                    .id("table-area")
                    .relative()
                    .flex()
                    .flex_col()
                    .flex_1()
                    .min_h_0()
                    .mx(px(TABLE_MARGIN))
                    .mb(px(TABLE_MARGIN))
                    .on_scroll(cx.listener(Self::on_table_scroll))
                    .child(body)
                    .child(v_scrollbar);

                if let Some(bar) = h_scrollbar {
                    table_area = table_area.child(bar);
                }

                div()
                    .flex()
                    .flex_col()
                    .size_full()
                    .child(file_header)
                    .child(table_area)
                    .into_any_element()
            }
        };

        let mut root = div()
            .id("root")
            .relative()
            .track_focus(&self.focus_handle)
            .on_action(cx.listener(Self::move_up))
            .on_action(cx.listener(Self::move_down))
            .on_action(cx.listener(Self::move_left))
            .on_action(cx.listener(Self::move_right))
            .on_action(cx.listener(Self::page_up))
            .on_action(cx.listener(Self::page_down))
            .on_action(cx.listener(Self::first_column))
            .on_action(cx.listener(Self::last_column))
            .on_action(cx.listener(Self::first_row))
            .on_action(cx.listener(Self::last_row))
            .on_action(cx.listener(Self::copy_cell))
            .on_action(cx.listener(Self::open_file))
            .on_action(cx.listener(Self::toggle_search))
            .on_action(cx.listener(Self::dismiss_search))
            .on_action(cx.listener(Self::search_next))
            .on_action(cx.listener(Self::search_prev))
            .size_full()
            .bg(rgb(0x1e1e2e))
            .child(content);

        // While dragging a scrollbar, listen at window level instead of on an element:
        // element handlers only fire while the cursor is over them, so a release outside
        // the window was missed and the drag got stuck. The OS keeps delivering move/up
        // events to the window during a drag (mouse capture on Windows, implicit grab on
        // X11/Wayland, Cocoa on macOS).
        if self.dragging.is_some() {
            let view = cx.entity().downgrade();
            root = root.child(
                canvas(
                    |_, _, _| {},
                    move |_, _, window, _| {
                        let move_view = view.clone();
                        window.on_mouse_event(move |e: &MouseMoveEvent, phase, window, cx| {
                            if phase == DispatchPhase::Bubble {
                                move_view.update(cx, |this, cx| this.on_drag_move(e, window, cx)).ok();
                            }
                        });
                        window.on_mouse_event(move |e: &MouseUpEvent, phase, _window, cx| {
                            if phase == DispatchPhase::Bubble && e.button == MouseButton::Left {
                                view.update(cx, |this, cx| this.end_drag(cx)).ok();
                            }
                        });
                    },
                )
                .absolute()
                .size_full(),
            );
        }

        // Search bar overlay
        if self.search_active {
            let query_display = if self.search_query.is_empty() {
                "Search…".to_string()
            } else {
                self.search_query.clone()
            };
            let query_is_empty = self.search_query.is_empty();
            let has_matches = !self.search_matches.is_empty();
            let counter_text = if has_matches {
                format!("{} of {}", self.search_index + 1, self.search_matches.len())
            } else if !self.search_query.is_empty() {
                "0 results".to_string()
            } else {
                String::new()
            };

            let view_prev = cx.entity().downgrade();
            let view_next = cx.entity().downgrade();
            let view_key = cx.entity().downgrade();

            root = root.child(
                div()
                    .absolute()
                    .top(px(8.0))
                    .w_full()
                    .flex()
                    .justify_center()
                    .child(
                        div()
                            .id("search-bar")
                            .track_focus(&self.search_focus)
                            .flex()
                            .items_center()
                            .gap_2()
                            .px_3()
                            .py_2()
                            .rounded_lg()
                            .bg(rgb(0x313244))
                            .border_1()
                            .border_color(rgb(0x585b70))
                            .shadow_md()
                            .on_key_down(move |event: &KeyDownEvent, _window, cx| {
                                let keystroke = &event.keystroke;
                                let key = keystroke.key.as_str();
                                let is_cmd = keystroke.modifiers.platform || keystroke.modifiers.control;

                                // Escape: dismiss search
                                if key == "escape" {
                                    view_key
                                        .update_in(cx, |this, window, cx| {
                                            this.dismiss_search(&DismissSearch, window, cx);
                                        })
                                        .ok();
                                    return;
                                }

                                // Ctrl/Cmd+F: toggle off
                                if key == "f" && is_cmd {
                                    view_key
                                        .update_in(cx, |this, window, cx| {
                                            this.toggle_search(&ToggleSearch, window, cx);
                                        })
                                        .ok();
                                    return;
                                }

                                // Enter: execute search
                                if key == "enter" {
                                    view_key
                                        .update(cx, |this, cx| {
                                            this.execute_search(cx);
                                        })
                                        .ok();
                                    return;
                                }

                                // Backspace: delete last char
                                if key == "backspace" {
                                    view_key
                                        .update(cx, |this, cx| {
                                            this.search_query.pop();
                                            cx.notify();
                                        })
                                        .ok();
                                    return;
                                }

                                // Regular character input
                                if let Some(ch) = &keystroke.key_char {
                                    let text = ch.clone();
                                    view_key
                                        .update(cx, |this, cx| {
                                            this.search_query.push_str(&text);
                                            cx.notify();
                                        })
                                        .ok();
                                } else if key.len() == 1 && !is_cmd && !keystroke.modifiers.alt {
                                    let ch = key.to_string();
                                    view_key
                                        .update(cx, |this, cx| {
                                            this.search_query.push_str(&ch);
                                            cx.notify();
                                        })
                                        .ok();
                                }
                            })
                            .child(
                                div()
                                    .flex()
                                    .items_center()
                                    .px_2()
                                    .py_1()
                                    .rounded_md()
                                    .bg(rgb(0x1e1e2e))
                                    .border_1()
                                    .border_color(rgb(0x45475a))
                                    .min_w(px(240.0))
                                    .child(
                                        div()
                                            .flex()
                                            .items_center()
                                            .text_sm()
                                            .text_color(if query_is_empty {
                                                rgb(0x585b70)
                                            } else {
                                                rgb(0xcdd6f4)
                                            })
                                            .child(query_display)
                                            .when(!query_is_empty, |el| {
                                                el.child(
                                                    div()
                                                        .text_color(rgb(0x89b4fa))
                                                        .ml(px(-1.0))
                                                        .child("|"),
                                                )
                                            }),
                                    ),
                            )
                            .child(
                                div()
                                    .text_sm()
                                    .text_color(rgb(0x6c7086))
                                    .min_w(px(70.0))
                                    .flex()
                                    .justify_center()
                                    .child(counter_text),
                            )
                            .child(
                                div()
                                    .id("search-prev-btn")
                                    .px_2()
                                    .py_1()
                                    .rounded_md()
                                    .cursor_pointer()
                                    .hover(|s| s.bg(rgb(0x45475a)))
                                    .text_sm()
                                    .text_color(rgb(0xcdd6f4))
                                    .child("▲")
                                    .on_click(move |_, window, cx| {
                                        view_prev
                                            .update(cx, |this, cx| {
                                                this.search_prev(&SearchPrev, window, cx);
                                            })
                                            .ok();
                                    }),
                            )
                            .child(
                                div()
                                    .id("search-next-btn")
                                    .px_2()
                                    .py_1()
                                    .rounded_md()
                                    .cursor_pointer()
                                    .hover(|s| s.bg(rgb(0x45475a)))
                                    .text_sm()
                                    .text_color(rgb(0xcdd6f4))
                                    .child("▼")
                                    .on_click(move |_, window, cx| {
                                        view_next
                                            .update(cx, |this, cx| {
                                                this.search_next(&SearchNext, window, cx);
                                            })
                                            .ok();
                                    }),
                            ),
                    ),
            );
        }

        // Toast notification overlay
        if let Some(msg) = &self.toast {
            root = root.child(
                div()
                    .absolute()
                    .bottom(px(24.0))
                    .w_full()
                    .flex()
                    .justify_center()
                    .child(
                        div()
                            .px_4()
                            .py_2()
                            .rounded_md()
                            .bg(rgb(0x45475a))
                            .text_sm()
                            .text_color(rgb(0xcdd6f4))
                            .child(msg.clone()),
                    ),
            );
        }

        root
    }
}

#[cfg(test)]
mod tests {
    use super::{
        AppState, ColumnLayout, DataSource, DragTarget, HEADER_HEIGHT, Move, ROW_HEIGHT, RootView, SCROLLBAR_MIN_THUMB,
        SCROLLBAR_SIZE, SearchNext, SearchPrev, TABLE_MARGIN, key_bindings, next_cell, register_actions, reveal,
        thumb_geom,
    };
    use gpui::{
        Entity, Focusable, Modifiers, MouseButton, MouseMoveEvent, Pixels, Point, ScrollDelta, ScrollEvent,
        TestAppContext, TouchPhase, VisualTestContext, point, px, size,
    };
    use std::rc::Rc;
    use std::time::Duration;

    /// `KeyBinding::new` panics on a keystroke it can't parse, at app start.
    #[test]
    fn key_bindings_parse() {
        assert!(!key_bindings().is_empty());
    }

    const H: usize = RootView::HEADER_ROW;

    #[test]
    fn reveal_moves_least() {
        // Already visible: no change.
        assert_eq!(reveal(100.0, 50.0, 110.0, 10.0), 100.0);
        // Above / left of the view: align start.
        assert_eq!(reveal(100.0, 50.0, 80.0, 10.0), 80.0);
        // Below / right of the view: align end.
        assert_eq!(reveal(100.0, 50.0, 160.0, 10.0), 120.0);
        // Partly cut at the end.
        assert_eq!(reveal(100.0, 50.0, 145.0, 10.0), 105.0);
        // Larger than the view: align start.
        assert_eq!(reveal(100.0, 50.0, 300.0, 80.0), 300.0);
    }

    #[test]
    fn next_cell_clamps_to_edges() {
        assert_eq!(next_cell((0, 0), Move::Up, 10, 3, 4), (0, 0));
        assert_eq!(next_cell((0, 0), Move::Left, 10, 3, 4), (0, 0));
        assert_eq!(next_cell((9, 2), Move::Down, 10, 3, 4), (9, 2));
        assert_eq!(next_cell((9, 2), Move::Right, 10, 3, 4), (9, 2));
        assert_eq!(next_cell((5, 1), Move::Down, 10, 3, 4), (6, 1));
        assert_eq!(next_cell((5, 1), Move::Right, 10, 3, 4), (5, 2));
    }

    #[test]
    fn next_cell_pages_and_jumps() {
        assert_eq!(next_cell((5, 1), Move::PageDown, 10, 3, 4), (9, 1));
        assert_eq!(next_cell((2, 1), Move::PageDown, 10, 3, 4), (6, 1));
        assert_eq!(next_cell((2, 1), Move::PageUp, 10, 3, 4), (0, 1));
        assert_eq!(next_cell((5, 1), Move::FirstRow, 10, 3, 4), (0, 1));
        assert_eq!(next_cell((5, 1), Move::LastRow, 10, 3, 4), (9, 1));
        assert_eq!(next_cell((5, 1), Move::FirstColumn, 10, 3, 4), (5, 0));
        assert_eq!(next_cell((5, 1), Move::LastColumn, 10, 3, 4), (5, 2));
    }

    #[test]
    fn next_cell_never_enters_header() {
        assert_eq!(next_cell((0, 1), Move::Up, 10, 3, 4), (0, 1));
        assert_eq!(next_cell((0, 1), Move::PageUp, 10, 3, 4), (0, 1));
        // Leaves the header downwards, moves along it sideways.
        assert_eq!(next_cell((H, 1), Move::Up, 10, 3, 4), (H, 1));
        assert_eq!(next_cell((H, 1), Move::Down, 10, 3, 4), (0, 1));
        assert_eq!(next_cell((H, 1), Move::PageDown, 10, 3, 4), (0, 1));
        assert_eq!(next_cell((H, 1), Move::LastRow, 10, 3, 4), (9, 1));
        assert_eq!(next_cell((H, 1), Move::Right, 10, 3, 4), (H, 2));
        // No data rows: stays in the header.
        assert_eq!(next_cell((H, 1), Move::Down, 0, 3, 4), (H, 1));
    }

    // -- Pure geometry --

    #[test]
    fn thumb_geom_sizes_and_places_the_thumb() {
        // Everything visible, or no track: no thumb.
        assert!(thumb_geom(0.0, 100.0, 1.0, 0.0).is_none());
        assert!(thumb_geom(0.0, 0.0, 0.5, 0.0).is_none());

        let g = thumb_geom(10.0, 200.0, 0.25, 0.0).unwrap();
        assert_eq!((g.track_origin, g.track_len, g.thumb_len, g.thumb_start), (10.0, 200.0, 50.0, 0.0));
        let g = thumb_geom(10.0, 200.0, 0.25, 1.0).unwrap();
        assert_eq!(g.thumb_start, 150.0); // flush with the track end
        let g = thumb_geom(10.0, 200.0, 0.25, 0.5).unwrap();
        assert_eq!(g.thumb_start, 75.0);
        // Out-of-range positions are clamped.
        assert_eq!(thumb_geom(0.0, 200.0, 0.25, 2.0).unwrap().thumb_start, 150.0);
        assert_eq!(thumb_geom(0.0, 200.0, 0.25, -1.0).unwrap().thumb_start, 0.0);
        // Huge files: the thumb keeps a minimum size, and still ends flush.
        let g = thumb_geom(0.0, 200.0, 1e-7, 1.0).unwrap();
        assert_eq!(g.thumb_len, SCROLLBAR_MIN_THUMB);
        assert_eq!(g.thumb_start + g.thumb_len, 200.0);
    }

    #[test]
    fn column_layout_accumulates_positions() {
        let source = FakeSource::new(5, &[100.0, 50.0, 200.0]);
        let l = ColumnLayout::new(Some(&source));
        assert_eq!(l.col_positions, vec![0.0, 100.0, 150.0]);
        assert_eq!(l.data_width, 350.0);

        // The row number column grows with the digits of the row count.
        let small = ColumnLayout::new(Some(&FakeSource::new(5, &[100.0])));
        let huge = ColumnLayout::new(Some(&FakeSource::new(10_000_000_000, &[100.0])));
        assert!(huge.row_num_width > small.row_num_width);

        let empty = ColumnLayout::new(None);
        assert!(empty.col_widths.is_empty());
        assert_eq!(empty.visible_range(0.0, 500.0), (0, 0, 0.0));
    }

    #[test]
    fn visible_range_covers_partly_visible_columns() {
        let l = ColumnLayout::new(Some(&FakeSource::new(5, &[100.0, 50.0, 200.0, 100.0])));
        // Columns start at 0, 100, 150, 350; data ends at 450.
        assert_eq!(l.visible_range(0.0, 120.0), (0, 2, 0.0));
        assert_eq!(l.visible_range(0.0, 100.0), (0, 1, 0.0));
        // Scrolled into the middle of column 0.
        assert_eq!(l.visible_range(40.0, 100.0), (0, 2, -40.0));
        // Exactly at a column edge: the previous column is gone.
        assert_eq!(l.visible_range(100.0, 60.0), (1, 3, 0.0));
        // Inside one wide column.
        assert_eq!(l.visible_range(200.0, 50.0), (2, 3, -50.0));
        // At the end.
        assert_eq!(l.visible_range(400.0, 100.0), (3, 4, -50.0));
    }

    // -- View harness --

    struct FakeSource {
        rows: usize,
        widths: Vec<f32>,
    }

    impl FakeSource {
        fn new(rows: usize, widths: &[f32]) -> Self {
            Self { rows, widths: widths.to_vec() }
        }
    }

    impl DataSource for FakeSource {
        fn row_count(&self) -> usize {
            self.rows
        }
        fn col_count(&self) -> usize {
            self.widths.len()
        }
        fn column_name(&self, _col: usize) -> &str {
            "col"
        }
        fn column_type(&self, _col: usize) -> &str {
            "Utf8"
        }
        fn cell(&self, row: usize, col: usize) -> String {
            format!("{row}:{col}")
        }
        fn cell_full(&self, row: usize, col: usize) -> String {
            format!("{row}:{col} full")
        }
        fn column_widths(&self) -> Vec<f32> {
            self.widths.clone()
        }
        fn is_cached(&self, _row: usize, _col: usize) -> bool {
            true
        }
    }

    const WINDOW_W: f32 = 800.0;
    const WINDOW_H: f32 = 600.0;

    fn open_view<'a>(
        cx: &'a mut TestAppContext,
        rows: usize,
        widths: &[f32],
    ) -> (Entity<RootView>, &'a mut VisualTestContext) {
        open_state(
            cx,
            AppState::Loaded {
                file_name: "test.parquet".into(),
                source: Rc::new(FakeSource::new(rows, widths)),
            },
        )
    }

    fn open_state(cx: &mut TestAppContext, state: AppState) -> (Entity<RootView>, &mut VisualTestContext) {
        cx.update(register_actions);
        let (view, cx) = cx.add_window_view(|_, cx| RootView::new(state, cx));
        cx.simulate_resize(size(px(WINDOW_W), px(WINDOW_H)));
        cx.update(|window, cx| view.focus_handle(cx).focus(window, cx));
        cx.run_until_parked();
        (view, cx)
    }

    /// Ten columns of 150 px: wider than the window, so both axes scroll.
    const WIDE: &[f32] = &[150.0; 10];

    fn read<R>(view: &Entity<RootView>, cx: &mut VisualTestContext, f: impl FnOnce(&RootView) -> R) -> R {
        view.read_with(cx, |v, _| f(v))
    }

    fn wheel(cx: &mut VisualTestContext, dx: f32, dy: f32) {
        cx.simulate_event(ScrollEvent {
            position: point(px(WINDOW_W / 2.0), px(WINDOW_H / 2.0)),
            delta: ScrollDelta::Pixels(point(px(dx), px(dy))),
            touch_phase: TouchPhase::Started,
            ..Default::default()
        });
        cx.run_until_parked();
    }

    fn mouse_move(cx: &mut VisualTestContext, pos: Point<Pixels>, button: Option<MouseButton>) {
        cx.simulate_event(MouseMoveEvent { position: pos, pressed_button: button, modifiers: Modifiers::none() });
        cx.run_until_parked();
    }

    /// Centre of the vertical thumb, in window coordinates.
    fn v_thumb_center(view: &Entity<RootView>, cx: &mut VisualTestContext) -> Point<Pixels> {
        let g = read(view, cx, |v| v.v_thumb.get()).expect("vertical thumb");
        point(
            px(WINDOW_W - TABLE_MARGIN - SCROLLBAR_SIZE / 2.0),
            px(g.track_origin + g.thumb_start + g.thumb_len / 2.0),
        )
    }

    #[gpui::test]
    fn view_measures_its_geometry(cx: &mut TestAppContext) {
        let (view, cx) = open_view(cx, 1000, WIDE);
        let (body, viewport_w, h_max) = read(&view, cx, |v| (v.body_height.get(), v.viewport_w, v.h_max_offset));
        assert!(body > 0.0 && body < WINDOW_H - HEADER_HEIGHT, "body height {body}");
        let row_num_w = read(&view, cx, |v| v.layout.row_num_width);
        assert_eq!(viewport_w, WINDOW_W - 2.0 * TABLE_MARGIN - row_num_w);
        assert_eq!(h_max, 1500.0 + SCROLLBAR_SIZE - viewport_w);
        assert!(read(&view, cx, |v| v.v_thumb.get().is_some() && v.h_thumb.get().is_some()));
    }

    // -- Wheel and trackpad --

    #[gpui::test]
    fn wheel_scrolls_and_clamps_both_axes(cx: &mut TestAppContext) {
        let (view, cx) = open_view(cx, 1000, WIDE);
        wheel(cx, 0.0, -100.0);
        assert_eq!(read(&view, cx, |v| v.v_offset), 100.0);
        wheel(cx, -70.0, 0.0);
        assert_eq!(read(&view, cx, |v| v.h_offset), 70.0);

        // Past the ends: clamped.
        wheel(cx, 0.0, -1e7);
        wheel(cx, -1e7, 0.0);
        let (v_off, v_max, h_off, h_max) =
            read(&view, cx, |v| (v.v_offset, v.v_max_offset(), v.h_offset, v.h_max_offset));
        assert_eq!((v_off, h_off), (v_max, h_max));
        // At the bottom, the last row sits right above the horizontal scrollbar.
        let visible = read(&view, cx, |v| v.visible_rows_height()) as f64;
        assert_eq!(1000.0 * ROW_HEIGHT as f64 - v_off, visible);
        // At the right, the last column ends right before the vertical scrollbar.
        let cols_w = read(&view, cx, |v| v.visible_cols_width());
        assert_eq!(1500.0 - h_off, cols_w);

        wheel(cx, 1e7, 0.0);
        wheel(cx, 0.0, 1e7);
        assert_eq!(read(&view, cx, |v| (v.v_offset, v.h_offset)), (0.0, 0.0));
    }

    #[gpui::test]
    fn trackpad_gesture_locks_to_its_main_axis(cx: &mut TestAppContext) {
        let (view, cx) = open_view(cx, 1000, WIDE);
        // Mostly vertical swipe with some sideways drift.
        wheel(cx, -3.0, -40.0);
        assert_eq!(read(&view, cx, |v| (v.v_offset, v.h_offset)), (40.0, 0.0));
    }

    #[gpui::test]
    fn small_scrolls_still_move_deep_in_huge_files(cx: &mut TestAppContext) {
        // 10M rows is ~280M px, past where f32 can hold whole pixels.
        let (view, cx) = open_view(cx, 10_000_000, WIDE);
        wheel(cx, 0.0, -1e10);
        let bottom = read(&view, cx, |v| v.v_offset);
        assert!(bottom > 2.0e8);
        wheel(cx, 0.0, 1.0);
        assert_eq!(read(&view, cx, |v| v.v_offset), bottom - 1.0);
    }

    #[gpui::test]
    fn growing_the_window_clamps_offsets(cx: &mut TestAppContext) {
        let (view, cx) = open_view(cx, 100, WIDE);
        wheel(cx, -1e7, 0.0);
        wheel(cx, 0.0, -1e7);
        assert!(read(&view, cx, |v| v.h_offset > 0.0 && v.v_offset > 0.0));
        cx.simulate_resize(size(px(2400.0), px(1800.0)));
        cx.run_until_parked();
        let (v_off, v_max, h_off, h_max) =
            read(&view, cx, |v| (v.v_offset, v.v_max_offset(), v.h_offset, v.h_max_offset));
        assert_eq!(h_max, 0.0); // all columns fit now
        assert_eq!(h_off, 0.0);
        assert!(v_off <= v_max);
    }

    #[gpui::test]
    fn columns_keep_their_width_while_scrolling_sideways(cx: &mut TestAppContext) {
        let (view, cx) = open_view(cx, 100, WIDE);
        let left = TABLE_MARGIN + read(&view, cx, |v| v.layout.row_num_width);
        for offset in [0.0, 75.0, 300.0, 333.0] {
            let current = read(&view, cx, |v| v.h_offset);
            wheel(cx, current - offset, 0.0);
            assert_eq!(read(&view, cx, |v| v.h_offset), offset);
            let (first, last, _) = read(&view, cx, |v| v.layout.visible_range(v.h_offset, v.viewport_w));
            for col in first..last {
                for key in [format!("hdr-{col}"), format!("cell-0-{col}")] {
                    let key: &'static str = Box::leak(key.into_boxed_str());
                    let b = cx.debug_bounds(key).unwrap_or_else(|| panic!("{key} not drawn at offset {offset}"));
                    assert_eq!(b.size.width, px(150.0), "{key} at offset {offset}");
                    assert_eq!(b.origin.x, px(left + 150.0 * col as f32 - offset), "{key} at offset {offset}");
                }
            }
        }
    }

    // -- Scrollbar dragging --

    #[gpui::test]
    fn dragging_the_thumb_follows_the_cursor(cx: &mut TestAppContext) {
        let (view, cx) = open_view(cx, 1000, WIDE);
        let start = v_thumb_center(&view, cx);
        let g0 = read(&view, cx, |v| v.v_thumb.get()).unwrap();

        cx.simulate_mouse_down(start, MouseButton::Left, Modifiers::none());
        cx.run_until_parked();
        // Grabbing the thumb doesn't move it.
        assert_eq!(read(&view, cx, |v| v.v_offset), 0.0);
        assert_eq!(read(&view, cx, |v| v.dragging), Some(DragTarget::Vertical));

        mouse_move(cx, start + point(px(0.0), px(50.0)), Some(MouseButton::Left));
        let g1 = read(&view, cx, |v| v.v_thumb.get()).unwrap();
        assert!((g1.thumb_start - g0.thumb_start - 50.0).abs() < 0.01, "thumb moved {}", g1.thumb_start);
        let (off, max) = read(&view, cx, |v| (v.v_offset, v.v_max_offset()));
        let travel = (g0.track_len - g0.thumb_len) as f64;
        assert!((off - 50.0 * max / travel).abs() < 0.01);

        // A click on the scrollbar doesn't select the cell underneath.
        cx.simulate_mouse_up(start, MouseButton::Left, Modifiers::none());
        cx.run_until_parked();
        assert_eq!(read(&view, cx, |v| v.selected_cell), None);
    }

    #[gpui::test]
    fn horizontal_thumb_drags_too(cx: &mut TestAppContext) {
        let (view, cx) = open_view(cx, 1000, WIDE);
        let g = read(&view, cx, |v| v.h_thumb.get()).unwrap();
        let start = point(
            px(g.track_origin + g.thumb_start + g.thumb_len / 2.0),
            px(WINDOW_H - TABLE_MARGIN - SCROLLBAR_SIZE / 2.0),
        );
        cx.simulate_mouse_down(start, MouseButton::Left, Modifiers::none());
        mouse_move(cx, start + point(px(1e5), px(0.0)), Some(MouseButton::Left));
        let (off, max) = read(&view, cx, |v| (v.h_offset, v.h_max_offset));
        assert_eq!(off, max);
    }

    #[gpui::test]
    fn release_outside_the_window_ends_the_drag(cx: &mut TestAppContext) {
        let (view, cx) = open_view(cx, 1000, WIDE);
        let start = v_thumb_center(&view, cx);
        cx.simulate_mouse_down(start, MouseButton::Left, Modifiers::none());
        mouse_move(cx, point(px(-200.0), px(-200.0)), Some(MouseButton::Left));
        cx.simulate_mouse_up(point(px(-200.0), px(-200.0)), MouseButton::Left, Modifiers::none());
        cx.run_until_parked();
        assert_eq!(read(&view, cx, |v| v.dragging), None);

        // Moving afterwards doesn't scroll.
        let off = read(&view, cx, |v| v.v_offset);
        mouse_move(cx, start + point(px(0.0), px(100.0)), None);
        assert_eq!(read(&view, cx, |v| v.v_offset), off);
    }

    #[gpui::test]
    fn missed_release_ends_the_drag_on_next_move(cx: &mut TestAppContext) {
        let (view, cx) = open_view(cx, 1000, WIDE);
        let start = v_thumb_center(&view, cx);
        cx.simulate_mouse_down(start, MouseButton::Left, Modifiers::none());
        cx.run_until_parked();
        // The release happened in another app; the next move has no button held.
        mouse_move(cx, start + point(px(0.0), px(80.0)), None);
        assert_eq!(read(&view, cx, |v| v.dragging), None);
        assert_eq!(read(&view, cx, |v| v.v_offset), 0.0);
    }

    #[gpui::test]
    fn clicking_the_track_centres_the_thumb_there(cx: &mut TestAppContext) {
        let (view, cx) = open_view(cx, 1000, WIDE);
        let g = read(&view, cx, |v| v.v_thumb.get()).unwrap();
        let x = px(WINDOW_W - TABLE_MARGIN - SCROLLBAR_SIZE / 2.0);
        // Middle of the track.
        cx.simulate_mouse_down(point(x, px(g.track_origin + g.track_len / 2.0)), MouseButton::Left, Modifiers::none());
        cx.simulate_mouse_up(point(x, px(g.track_origin + g.track_len / 2.0)), MouseButton::Left, Modifiers::none());
        cx.run_until_parked();
        let (off, max) = read(&view, cx, |v| (v.v_offset, v.v_max_offset()));
        assert!((off / max - 0.5).abs() < 0.001, "at {}", off / max);
        // Very end of the track.
        cx.simulate_mouse_down(point(x, px(g.track_origin + g.track_len - 1.0)), MouseButton::Left, Modifiers::none());
        cx.run_until_parked();
        assert_eq!(read(&view, cx, |v| v.v_offset), max);
    }

    #[gpui::test]
    fn drag_shows_data_after_a_pause(cx: &mut TestAppContext) {
        let (view, cx) = open_view(cx, 1000, WIDE);
        let start = v_thumb_center(&view, cx);
        cx.simulate_mouse_down(start, MouseButton::Left, Modifiers::none());
        mouse_move(cx, start + point(px(0.0), px(30.0)), Some(MouseButton::Left));
        assert!(!read(&view, cx, |v| v.drag_idle));
        cx.executor().advance_clock(Duration::from_millis(150));
        cx.run_until_parked();
        assert!(read(&view, cx, |v| v.drag_idle));
        // Moving again resets it.
        mouse_move(cx, start + point(px(0.0), px(40.0)), Some(MouseButton::Left));
        assert!(!read(&view, cx, |v| v.drag_idle));
    }

    // -- Keyboard --

    /// Asserts the selected cell is fully inside the visible area.
    fn assert_selection_visible(view: &Entity<RootView>, cx: &mut VisualTestContext) {
        read(view, cx, |v| {
            let (row, col) = v.selected_cell.unwrap();
            let top = row as f64 * ROW_HEIGHT as f64;
            assert!(top >= v.v_offset, "row {row} above view");
            assert!(top + ROW_HEIGHT as f64 <= v.v_offset + v.visible_rows_height() as f64, "row {row} below view");
            let (start, width) = (v.layout.col_positions[col], v.layout.col_widths[col]);
            assert!(start >= v.h_offset, "column {col} left of view");
            let fits = width <= v.visible_cols_width();
            assert!(
                if fits { start + width <= v.h_offset + v.visible_cols_width() } else { start == v.h_offset },
                "column {col} not shown"
            );
        });
    }

    #[gpui::test]
    fn first_key_selects_top_left_visible_cell(cx: &mut TestAppContext) {
        let (view, cx) = open_view(cx, 1000, WIDE);
        wheel(cx, 0.0, -42.0); // 1.5 rows down
        wheel(cx, -160.0, 0.0); // into column 1
        cx.simulate_keystrokes("right");
        // Row 1 is cut at the top, column 1 at the left: take the next whole ones.
        assert_eq!(read(&view, cx, |v| v.selected_cell), Some((2, 2)));
    }

    #[gpui::test]
    fn arrows_move_and_scroll_into_view(cx: &mut TestAppContext) {
        let (view, cx) = open_view(cx, 1000, WIDE);
        cx.simulate_keystrokes("down");
        assert_eq!(read(&view, cx, |v| v.selected_cell), Some((0, 0)));

        let page = read(&view, cx, |v| v.page_rows());
        for _ in 0..page + 3 {
            cx.simulate_keystrokes("down");
            assert_selection_visible(&view, cx);
        }
        assert_eq!(read(&view, cx, |v| v.selected_cell), Some((page + 3, 0)));
        // Scrolled just enough: the cursor row is the bottom visible one.
        read(&view, cx, |v| {
            let bottom = (page + 4) as f64 * ROW_HEIGHT as f64;
            assert_eq!(bottom, v.v_offset + v.visible_rows_height() as f64);
        });

        for _ in 0..9 {
            cx.simulate_keystrokes("right");
            assert_selection_visible(&view, cx);
        }
        assert_eq!(read(&view, cx, |v| v.selected_cell), Some((page + 3, 9)));
        cx.simulate_keystrokes("right"); // clamped at the last column
        assert_eq!(read(&view, cx, |v| v.selected_cell), Some((page + 3, 9)));

        for _ in 0..9 {
            cx.simulate_keystrokes("left");
            assert_selection_visible(&view, cx);
        }
        assert_eq!(read(&view, cx, |v| v.h_offset), 0.0);
        for _ in 0..page + 10 {
            cx.simulate_keystrokes("up");
            assert_selection_visible(&view, cx);
        }
        assert_eq!(read(&view, cx, |v| (v.selected_cell, v.v_offset)), (Some((0, 0)), 0.0));
    }

    #[gpui::test]
    fn columns_wider_than_the_view_align_left(cx: &mut TestAppContext) {
        let (view, cx) = open_view(cx, 10, &[100.0, 2000.0, 100.0]);
        cx.simulate_keystrokes("right right");
        assert_eq!(read(&view, cx, |v| v.selected_cell), Some((0, 1)));
        assert_eq!(read(&view, cx, |v| v.h_offset), 100.0);
        assert_selection_visible(&view, cx);
    }

    #[gpui::test]
    fn page_keys_keep_the_cursor_in_place_on_screen(cx: &mut TestAppContext) {
        let (view, cx) = open_view(cx, 1000, WIDE);
        cx.simulate_keystrokes("down down down down"); // select, then row 3
        assert_eq!(read(&view, cx, |v| v.selected_cell), Some((3, 0)));
        let page = read(&view, cx, |v| v.page_rows());
        assert!(page > 1);

        cx.simulate_keystrokes("pagedown");
        assert_eq!(read(&view, cx, |v| v.selected_cell), Some((3 + page, 0)));
        assert_eq!(read(&view, cx, |v| v.v_offset), (page as f64) * ROW_HEIGHT as f64);

        cx.simulate_keystrokes("pageup");
        assert_eq!(read(&view, cx, |v| (v.selected_cell, v.v_offset)), (Some((3, 0)), 0.0));

        // Paging past the end stops on the last row, still visible.
        for _ in 0..1000 / page + 2 {
            cx.simulate_keystrokes("pagedown");
        }
        assert_eq!(read(&view, cx, |v| v.selected_cell), Some((999, 0)));
        assert_selection_visible(&view, cx);
    }

    #[gpui::test]
    fn jump_keys_reach_the_edges(cx: &mut TestAppContext) {
        let (view, cx) = open_view(cx, 10_000_000, WIDE);
        cx.simulate_keystrokes("down");
        cx.simulate_keystrokes(secondary!("down"));
        assert_eq!(read(&view, cx, |v| v.selected_cell), Some((9_999_999, 0)));
        assert_eq!(read(&view, cx, |v| v.v_offset), read(&view, cx, |v| v.v_max_offset()));
        assert_selection_visible(&view, cx);

        cx.simulate_keystrokes("end");
        assert_eq!(read(&view, cx, |v| v.selected_cell), Some((9_999_999, 9)));
        assert_selection_visible(&view, cx);
        cx.simulate_keystrokes("home");
        assert_eq!(read(&view, cx, |v| v.selected_cell), Some((9_999_999, 0)));
        cx.simulate_keystrokes(secondary!("right"));
        assert_eq!(read(&view, cx, |v| v.selected_cell), Some((9_999_999, 9)));
        cx.simulate_keystrokes(secondary!("left"));
        assert_eq!(read(&view, cx, |v| v.selected_cell), Some((9_999_999, 0)));

        cx.simulate_keystrokes(secondary!("up"));
        assert_eq!(read(&view, cx, |v| (v.selected_cell, v.v_offset)), (Some((0, 0)), 0.0));
        cx.simulate_keystrokes(secondary!("end"));
        assert_eq!(read(&view, cx, |v| v.selected_cell), Some((9_999_999, 0)));
        cx.simulate_keystrokes(secondary!("home"));
        assert_eq!(read(&view, cx, |v| v.selected_cell), Some((0, 0)));
    }

    #[gpui::test]
    fn header_cursor_moves_down_into_rows_only(cx: &mut TestAppContext) {
        let (view, cx) = open_view(cx, 100, WIDE);
        view.update(cx, |v, _| v.selected_cell = Some((RootView::HEADER_ROW, 4)));
        cx.simulate_keystrokes("up");
        assert_eq!(read(&view, cx, |v| v.selected_cell), Some((RootView::HEADER_ROW, 4)));
        cx.simulate_keystrokes("right");
        assert_eq!(read(&view, cx, |v| v.selected_cell), Some((RootView::HEADER_ROW, 5)));
        cx.simulate_keystrokes("down");
        assert_eq!(read(&view, cx, |v| v.selected_cell), Some((0, 5)));
        cx.simulate_keystrokes("up");
        assert_eq!(read(&view, cx, |v| v.selected_cell), Some((0, 5)));
    }

    #[gpui::test]
    fn keys_do_nothing_without_data(cx: &mut TestAppContext) {
        let (view, cx) = open_view(cx, 0, WIDE);
        cx.simulate_keystrokes("down right pagedown end");
        cx.simulate_keystrokes(secondary!("down"));
        assert_eq!(read(&view, cx, |v| v.selected_cell), None);
    }

    #[gpui::test]
    fn keys_do_nothing_on_the_error_screen(cx: &mut TestAppContext) {
        let (view, cx) = open_state(cx, AppState::Error("x".into()));
        cx.simulate_keystrokes("down right pagedown");
        assert_eq!(read(&view, cx, |v| v.selected_cell), None);
    }

    #[gpui::test]
    fn copy_shortcuts_copy_the_full_value(cx: &mut TestAppContext) {
        let (view, cx) = open_view(cx, 100, WIDE);
        cx.simulate_keystrokes("down down right");
        assert_eq!(read(&view, cx, |v| v.selected_cell), Some((1, 1)));
        cx.simulate_keystrokes(secondary!("c"));
        assert_eq!(cx.read_from_clipboard().and_then(|c| c.text()).as_deref(), Some("1:1 full"));
        assert!(read(&view, cx, |v| v.toast.is_some()));

        cx.simulate_keystrokes("down ctrl-c");
        assert_eq!(cx.read_from_clipboard().and_then(|c| c.text()).as_deref(), Some("2:1 full"));
    }

    // -- Search --

    #[gpui::test]
    fn search_finds_matching_cells(cx: &mut TestAppContext) {
        // FakeSource: cell(r,c) = "{r}:{c}", column_name = "col"
        let (view, cx) = open_view(cx, 10, &[150.0; 3]);
        view.update(cx, |v, cx| {
            v.search_active = true;
            // Search for "0:1" — only cell (0,1) matches
            v.search_query = "0:1".into();
            v.execute_search(cx);
        });
        let (matches, idx, sel) = read(&view, cx, |v| {
            (v.search_matches.clone(), v.search_index, v.selected_cell)
        });
        assert_eq!(matches, vec![(0, 1)]);
        assert_eq!(idx, 0);
        assert_eq!(sel, Some((0, 1)));
    }

    #[gpui::test]
    fn search_is_case_insensitive(cx: &mut TestAppContext) {
        let (view, cx) = open_view(cx, 10, &[150.0; 3]);
        view.update(cx, |v, cx| {
            v.search_active = true;
            // "COL" should match headers (column_name returns "col")
            v.search_query = "COL".into();
            v.execute_search(cx);
        });
        let matches = read(&view, cx, |v| v.search_matches.clone());
        // All 3 headers match
        assert_eq!(matches.len(), 3);
        assert!(matches.iter().all(|&(r, _)| r == RootView::HEADER_ROW));
    }

    #[gpui::test]
    fn search_empty_query_clears_matches(cx: &mut TestAppContext) {
        let (view, cx) = open_view(cx, 10, &[150.0; 3]);
        view.update(cx, |v, cx| {
            v.search_active = true;
            v.search_query = "0:0".into();
            v.execute_search(cx);
        });
        assert!(!read(&view, cx, |v| v.search_matches.is_empty()));
        // Now search with empty query
        view.update(cx, |v, cx| {
            v.search_query = "".into();
            v.execute_search(cx);
        });
        assert!(read(&view, cx, |v| v.search_matches.is_empty()));
    }

    #[gpui::test]
    fn search_no_matches(cx: &mut TestAppContext) {
        let (view, cx) = open_view(cx, 10, &[150.0; 3]);
        view.update(cx, |v, cx| {
            v.search_active = true;
            v.search_query = "zzzzz_no_match".into();
            v.execute_search(cx);
        });
        assert!(read(&view, cx, |v| v.search_matches.is_empty()));
    }

    #[gpui::test]
    fn search_navigation_wraps_around(cx: &mut TestAppContext) {
        let (view, cx) = open_view(cx, 10, &[150.0; 3]);
        view.update(cx, |v, cx| {
            v.search_active = true;
            // "5:" matches 3 cells: (5,0), (5,1), (5,2)
            v.search_query = "5:".into();
            v.execute_search(cx);
        });
        let n = read(&view, cx, |v| v.search_matches.len());
        assert_eq!(n, 3);

        // Navigate forward through all, then wrap
        view.update_in(cx, |v, window, cx| v.search_next(&SearchNext, window, cx));
        assert_eq!(read(&view, cx, |v| v.search_index), 1);
        view.update_in(cx, |v, window, cx| v.search_next(&SearchNext, window, cx));
        assert_eq!(read(&view, cx, |v| v.search_index), 2);
        view.update_in(cx, |v, window, cx| v.search_next(&SearchNext, window, cx));
        // Wrapped to 0
        assert_eq!(read(&view, cx, |v| v.search_index), 0);

        // Navigate backward, wrap
        view.update_in(cx, |v, window, cx| v.search_prev(&SearchPrev, window, cx));
        // Wrapped to last
        assert_eq!(read(&view, cx, |v| v.search_index), 2);
    }

    #[gpui::test]
    fn toggle_and_dismiss_search(cx: &mut TestAppContext) {
        let (view, cx) = open_view(cx, 10, &[150.0; 3]);
        assert!(!read(&view, cx, |v| v.search_active));

        // Toggle on
        cx.simulate_keystrokes(secondary!("f"));
        assert!(read(&view, cx, |v| v.search_active));

        // Execute a search, then dismiss
        view.update(cx, |v, cx| {
            v.search_query = "0:0".into();
            v.execute_search(cx);
        });
        assert!(!read(&view, cx, |v| v.search_matches.is_empty()));
        let sel = read(&view, cx, |v| v.selected_cell);

        cx.simulate_keystrokes("escape");
        assert!(!read(&view, cx, |v| v.search_active));
        assert!(read(&view, cx, |v| v.search_matches.is_empty()));
        // Selection persists on the last navigated match
        assert_eq!(read(&view, cx, |v| v.selected_cell), sel);
    }
}
